// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stage 3: clustering and the final match score (`bz_match_score`, `bz_sift`, `bz_final_loop`).
//!
//! This is a faithful port of stock NBIS's core matcher: it consumes the sorted compatibility list
//! from stage 2 and grows **consistent clusters** of compatible edge pairs, returning the largest
//! total edge count over a mutually-compatible cluster set. The reference uses fixed global arrays;
//! here they live in [`Bz`] (heap `Vec`s sized to the input, per-cluster data grown as clusters
//! form). Two deliberate, behaviour-preserving deviations from the C, each marked `// PORT:` inline:
//!
//! * C relies on `&&` short-circuit so a value comparison runs *before* its bound check, performing
//!   a benign out-of-bounds read whose result never changes control flow. We reorder to bound-first,
//!   which is provably identical for the sorted/dedup sets involved and avoids the UB.
//! * The `colp[l-2]` head-of-run walk-back is guarded with `l >= 2` (the C reads `colp[-1]` when
//!   `l == 1`; the guarded form stops at the array head, the only sane deterministic behaviour).
//!
//! ## `QQ_OVERFLOW_SCORE` is promised and unreached
//!
//! [`crate::match_score`] documents `4000` ([`QQ_OVERFLOW_SCORE`]) when the `qq[]` work queue
//! overflows, and [`Bz::sift`] returns it. **No input reaches it, and no test asserts it — but it is
//! not far out of reach, and nothing here proves it unreachable.**
//!
//! `qh` grows on two routes. One is a probe endpoint newly paired into the current path, and
//! `xyt::prepare` caps a print at 150 minutiae, so that route stops at 151. The other is an edge
//! first stamped into the path (`sift` case B), bounded only by stage 2's 19999-row table — and that
//! route is the live one: a dense 150-minutia self-match drives `qh` to **3680** against a
//! [`QQ_SIZE`] of 4000, the nearest approach over the generated corpus and the grid and lattice
//! families. `qh` there tracks the score, which is `3679`.
//!
//! Two things follow, both worth knowing before touching this file:
//!
//! * The guard is reproduced and left unexercised. Faking a test for it needs a hand-built `colp`
//!   that stage 2 cannot emit, and `#[ignore]` would hide it, so this note is the record. The
//!   constant relation `QQ_OVERFLOW_SCORE == QQ_SIZE` is pinned in [`crate::consts`] — the part of
//!   the promise that can be checked.
//! * The sentinel shares its range with real scores. `4000` is 9% above the largest score seen, so a
//!   caller cannot read `4000` as "overflow" rather than "a very strong match". That is the
//!   reference's design, reproduced, not a defect introduced here.

use crate::consts::{
    iangle180, round_half_away, sense, CTXS, MIN_COMPUTABLE_BOZORTH_MINUTIAE, MMSTR, MSTR,
    QQ_OVERFLOW_SCORE, QQ_SIZE, TK, TXS, WWIM, ZERO_MATCH_SCORE,
};
use crate::inter::ColpRow;
use crate::xyt::Prepared;

/// Public entry: the BOZORTH3 match score for a compatibility list.
pub(crate) fn match_score(colp: &[ColpRow], probe: &Prepared, gallery: &Prepared) -> i32 {
    // Too few minutiae in either print → not computable (mirrors the `min_computable` guard).
    if probe.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE
        || gallery.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE
    {
        return ZERO_MATCH_SCORE;
    }

    let np = colp.len() as i32;
    // Append a zero sentinel row: the reference reads `colp[np]` (and one past the last written row)
    // and depends on it being all-zero to terminate several look-ahead loops.
    let mut c: Vec<ColpRow> = Vec::with_capacity(colp.len() + 1);
    c.extend_from_slice(colp);
    c.push([0; 5]);

    let mut bz = Bz::new(np, probe.nrows.max(gallery.nrows));
    bz.run(&c, np, probe, gallery)
}

/// The reference's global work arrays, sized to one match.
struct Bz {
    // Edge/endpoint-indexed scratch (the reference's 20000-element globals).
    sc: Vec<i32>,
    cp: Vec<i32>,
    rp: Vec<i32>,
    tq: Vec<i32>,
    rq: Vec<i32>,
    zz: Vec<i32>,
    rk: Vec<i32>,
    y: Vec<i32>,
    qq: Vec<i32>,
    // Endpoint-conflict groups (indexed by group id).
    rx: Vec<i32>,
    mm: Vec<i32>,
    nn: Vec<i32>,
    rr: Vec<i32>,
    cf: Vec<Vec<i32>>,
    rf: Vec<Vec<i32>>,
    // Per-cluster data, grown as clusters form (index = cluster id `tp`).
    avv: Vec<[i32; 5]>,
    ct: Vec<i32>,
    gct: Vec<i32>,
    ctt: Vec<i32>,
    ctp: Vec<Vec<i32>>,
    /// `yy[tp][side]` — the sorted, deduplicated endpoint set of cluster `tp` on the probe (0) /
    /// gallery (1) side. `yl[side][tp]` in the reference is simply this vector's length.
    yy: Vec<[Vec<i32>; 2]>,
}

impl Bz {
    fn new(np: i32, nend: usize) -> Self {
        // Edge-indexed arrays must also cover cluster ids (`tp ≤ np/3`) reused as indices in
        // `bz_final_loop`; endpoint arrays only need `nend`, but sizing them the same is harmless.
        let big = (np.max(nend as i32) + 2).max(2) as usize;
        let groups = 64usize; // ww is capped near WWIM (10); 64 is comfortably safe.
        let alts = nend + 2; // cf/rf alternative-endpoint lists ≤ minutiae count.
        Bz {
            sc: vec![0; big],
            cp: vec![0; big],
            rp: vec![0; big],
            tq: vec![0; big],
            rq: vec![0; big],
            zz: vec![1000; big], // reference: zz[] initialised to 1000
            rk: vec![0; big],
            y: vec![0; big],
            qq: vec![0; (QQ_SIZE as usize) + 2],
            rx: vec![0; groups],
            mm: vec![0; groups],
            nn: vec![0; groups],
            rr: vec![0; groups],
            cf: vec![vec![0; alts]; groups],
            rf: vec![vec![0; alts]; groups],
            avv: Vec::new(),
            ct: Vec::new(),
            gct: Vec::new(),
            ctt: Vec::new(),
            ctp: Vec::new(),
            yy: Vec::new(),
        }
    }

    /// `bz_sift`: link a lookahead edge into the current path, or record a pairing conflict.
    ///
    /// Returns `true` on `qq[]` overflow (the caller then returns `QQ_OVERFLOW_SCORE`).
    #[expect(clippy::too_many_arguments)] // mirrors the reference signature; splitting would obscure the port
    fn sift(
        &mut self,
        ww: &mut i32,
        kz: i32,
        qh: &mut i32,
        l: i32,
        kx: i32,
        ftt: i32,
        tot: &mut i32,
    ) -> bool {
        let n = self.tq[(kz - 1) as usize];
        let t = self.rq[(l - 1) as usize];

        // Case A — both endpoints free: accept the edge and record the new pairing.
        if n == 0 && t == 0 {
            if self.sc[(kx - 1) as usize] != ftt {
                self.y[*tot as usize] = kx;
                *tot += 1;
                self.rk[(kx - 1) as usize] = self.sc[(kx - 1) as usize];
                self.sc[(kx - 1) as usize] = ftt;
            }
            if *qh >= QQ_SIZE {
                return true;
            }
            self.qq[*qh as usize] = kz;
            self.zz[(kz - 1) as usize] = *qh;
            *qh += 1;
            self.tq[(kz - 1) as usize] = l;
            self.rq[(l - 1) as usize] = kz;
            return false;
        }

        // Case B — already consistently paired: re-accept without a new pairing.
        if n == l {
            if self.sc[(kx - 1) as usize] != ftt {
                // PORT: the reference tests zz[kx-1] (edge space) but writes zz[kz-1] (endpoint
                // space) — an original NIST quirk, reproduced verbatim.
                if self.zz[(kx - 1) as usize] == 1000 {
                    if *qh >= QQ_SIZE {
                        return true;
                    }
                    self.qq[*qh as usize] = kz;
                    self.zz[(kz - 1) as usize] = *qh;
                    *qh += 1;
                }
                self.y[*tot as usize] = kx;
                *tot += 1;
                self.rk[(kx - 1) as usize] = self.sc[(kx - 1) as usize];
                self.sc[(kx - 1) as usize] = ftt;
            }
            return false;
        }

        // Case C — conflict: record the alternative pairing(s), but do not extend the path.
        if *ww >= WWIM {
            return false;
        }
        if n != 0 {
            let mut b = self.cp[(kz - 1) as usize];
            if b == 0 {
                *ww += 1;
                b = *ww;
                let bi = (b - 1) as usize;
                self.cp[(kz - 1) as usize] = b;
                self.cf[bi][0] = n;
                self.mm[bi] = 1;
                self.nn[bi] = 1;
                self.rx[bi] = kz;
            }
            let bi = (b - 1) as usize;
            let lim = self.mm[bi];
            let mut found_at = lim; // index at which to append if not found
            for i in 0..lim {
                if self.cf[bi][i as usize] == l {
                    found_at = -1;
                    break;
                }
            }
            if found_at != -1 {
                self.cf[bi][lim as usize] = l;
                self.mm[bi] += 1;
            }
        }
        if t != 0 {
            let mut b = self.rp[(l - 1) as usize];
            if b == 0 {
                *ww += 1;
                b = *ww;
                let bi = (b - 1) as usize;
                self.rp[(l - 1) as usize] = b;
                self.rf[bi][0] = t;
                self.mm[bi] = 1;
                self.nn[bi] = 1;
                self.rx[bi] = -l;
            }
            let bi = (b - 1) as usize;
            let lim = self.mm[bi];
            let mut notfound = true;
            for i in 0..lim {
                if self.rf[bi][i as usize] == kz {
                    notfound = false;
                    break;
                }
            }
            if notfound {
                self.rf[bi][lim as usize] = kz;
                self.mm[bi] += 1;
            }
        }
        false
    }

    /// `bz_match_score`: the outer seed loop.
    fn run(&mut self, colp: &[ColpRow], np: i32, probe: &Prepared, gallery: &Prepared) -> i32 {
        let mut tp: i32 = 0;
        let mut match_score = 0i32;
        let mut ftt = 0i32;

        for k in 0..(np - 1).max(0) {
            let k = k as usize;
            if self.sc[k] != 0 {
                continue;
            }

            let seed_i = colp[k][1];
            let seed_t = colp[k][3];
            self.qq[0] = seed_i;
            self.rq[(seed_t - 1) as usize] = seed_i;
            self.tq[(seed_i - 1) as usize] = seed_t;

            let mut ww = 0i32;
            let mut dw = 0i32;

            // Endpoint-group loop: enumerate alternative pairings via the mixed-radix odometer.
            loop {
                ftt += 1;
                let mut tot = 0i32;
                let mut qh = 1i32;
                let mut kx = k as i32;

                // (5a) initial run of consecutive same-start edges
                loop {
                    let kz = colp[kx as usize][2];
                    let l = colp[kx as usize][4];
                    kx += 1;
                    if self.sift(&mut ww, kz, &mut qh, l, kx, ftt, &mut tot) {
                        return QQ_OVERFLOW_SCORE;
                    }
                    if !(colp[kx as usize][3] == colp[k][3] && colp[kx as usize][1] == colp[k][1]) {
                        break;
                    }
                }
                let kq = kx;

                // (5b) BFS expansion over discovered endpoints
                let mut j = 1i32;
                while j < qh {
                    // linear scan
                    for i in (kq as usize)..(np as usize) {
                        let mut z = 1i32;
                        let mut p1 = 0i32;
                        while z < 3 {
                            if z == 1 {
                                if (j + 1) > QQ_SIZE {
                                    return QQ_OVERFLOW_SCORE;
                                }
                                p1 = self.qq[j as usize];
                            } else {
                                p1 = self.tq[(p1 - 1) as usize];
                            }
                            if colp[i][(2 * z) as usize] != p1 {
                                break;
                            }
                            z += 1;
                        }
                        if z == 3 {
                            let zc = colp[i][1];
                            let lc = colp[i][3];
                            if zc != colp[k][1] && lc != colp[k][3] {
                                kx = i as i32 + 1;
                                if self.sift(&mut ww, zc, &mut qh, lc, kx, ftt, &mut tot) {
                                    return QQ_OVERFLOW_SCORE;
                                }
                            }
                        }
                    }

                    // binary search for qq[j] over colp on cols (1,3)
                    let mut l;
                    let mut t = np + 1;
                    let mut b = kq;
                    let mut n = -1i32;
                    let mut p2 = 0i32;
                    while t - b > 1 {
                        l = (b + t) / 2;
                        let mut p1 = 0i32;
                        let mut i = 1i32;
                        while i < 3 {
                            if i == 1 {
                                if (j + 1) > QQ_SIZE {
                                    return QQ_OVERFLOW_SCORE;
                                }
                                p1 = self.qq[j as usize];
                            } else {
                                p1 = self.tq[(p1 - 1) as usize];
                            }
                            p2 = colp[(l - 1) as usize][(i * 2 - 1) as usize];
                            n = sense(p1, p2);
                            if n < 0 {
                                t = l;
                                break;
                            }
                            if n > 0 {
                                b = l;
                                break;
                            }
                            i += 1;
                        }
                        if n == 0 {
                            // walk back to the head of the equal-key run
                            // PORT: guard l >= 2 (the reference reads colp[-1] at l == 1).
                            while l >= 2
                                && colp[(l - 2) as usize][3] == p2
                                && colp[(l - 2) as usize][1] == colp[(l - 1) as usize][1]
                            {
                                l -= 1;
                            }
                            kx = l - 1;
                            loop {
                                let kz = colp[kx as usize][2];
                                let ll = colp[kx as usize][4];
                                kx += 1;
                                if self.sift(&mut ww, kz, &mut qh, ll, kx, ftt, &mut tot) {
                                    return QQ_OVERFLOW_SCORE;
                                }
                                if !(colp[kx as usize][3] == p2
                                    && colp[kx as usize][1] == colp[(kx - 1) as usize][1])
                                {
                                    break;
                                }
                            }
                            break;
                        }
                    }
                    j += 1;
                }

                // (6a) rotation-average + consistency prune
                if tot >= MSTR {
                    let (mut jj, mut kk, mut n, mut l) = (0i32, 0i32, 0i32, 0i32);
                    for i in 0..tot {
                        let v = colp[(self.y[i as usize] - 1) as usize][0];
                        if v < 0 {
                            kk += v;
                            n += 1;
                        } else {
                            jj += v;
                            l += 1;
                        }
                    }
                    if n == 0 {
                        n = 1;
                    } else if l == 0 {
                        l = 1;
                    }
                    let mut fi = jj as f32 / l as f32 - kk as f32 / n as f32;
                    if fi > 180.0 {
                        fi = (jj + kk + n * 360) as f32 / tot as f32;
                        if fi > 180.0 {
                            fi -= 360.0;
                        }
                    } else {
                        fi = (jj + kk) as f32 / tot as f32;
                    }
                    let mut mean = round_half_away(fi);
                    if mean <= -180 {
                        mean += 360;
                    }
                    // prune out-of-tolerance edges, compacting y[] in place
                    kk = 0;
                    for i in 0..tot {
                        let diff = colp[(self.y[i as usize] - 1) as usize][0] - mean;
                        let sq = diff * diff;
                        if sq > TXS && sq < CTXS {
                            kk += 1;
                        } else {
                            self.y[(i - kk) as usize] = self.y[i as usize];
                        }
                    }
                    tot -= kk;
                }

                if tot < MSTR {
                    // (6b) path too short → roll back the stamps
                    for i in (0..tot).rev() {
                        let idx = (self.y[i as usize] - 1) as usize;
                        self.sc[idx] = if self.rk[idx] == 0 { -1 } else { self.rk[idx] };
                    }
                    ftt -= 1;
                } else {
                    // (6c) form cluster `tp`
                    self.build_cluster(colp, tp, tot, probe, gallery, &mut match_score);
                    tp += 1;
                }

                // (7) teardown + odometer
                if qh > QQ_SIZE {
                    return QQ_OVERFLOW_SCORE;
                }
                for i in (1..qh).rev() {
                    let n = self.qq[i as usize] - 1;
                    if self.tq[n as usize] > 0 {
                        self.rq[(self.tq[n as usize] - 1) as usize] = 0;
                        self.tq[n as usize] = 0;
                        self.zz[n as usize] = 1000;
                    }
                }
                for i in (0..dw).rev() {
                    let n = self.rr[i as usize] - 1;
                    if self.tq[n as usize] != 0 {
                        self.rq[(self.tq[n as usize] - 1) as usize] = 0;
                        self.tq[n as usize] = 0;
                    }
                }

                let more = self.odometer(&mut ww);
                if tp > 1999 {
                    break;
                }
                dw = ww;
                if !more {
                    break;
                }
            }

            if tp > 1999 {
                break;
            }

            // per-seed cleanup
            let n = self.qq[0] - 1;
            if self.tq[n as usize] > 0 {
                self.rq[(self.tq[n as usize] - 1) as usize] = 0;
                self.tq[n as usize] = 0;
            }
            for i in (0..ww).rev() {
                let n = self.rx[i as usize];
                if n < 0 {
                    self.rp[(-n - 1) as usize] = 0;
                } else {
                    self.cp[(n - 1) as usize] = 0;
                }
            }
        }

        if match_score < MMSTR {
            return match_score;
        }
        self.final_loop(tp)
    }

    /// (6c) Build cluster `tp` from the path collected in `y[0..tot]`, then cross-check it against
    /// every prior cluster and merge group totals where compatible and endpoint-disjoint.
    fn build_cluster(
        &mut self,
        colp: &[ColpRow],
        tp: i32,
        tot: i32,
        probe: &Prepared,
        gallery: &Prepared,
        match_score: &mut i32,
    ) {
        let tpu = tp as usize;
        // create per-cluster storage for `tp`
        self.yy.push([Vec::new(), Vec::new()]);

        let mut avn = [0i32; 5];
        let (mut pa, mut pb, mut pc, mut pd) = (0i32, 0i32, 0i32, 0i32);

        for i in 0..tot {
            let idx = (self.y[i as usize] - 1) as usize;
            for ii in 1..4 {
                let kk = (ii * ii - ii + 2) / 2 - 1;
                let jj = colp[idx][kk as usize];
                match ii {
                    1 => {
                        if colp[idx][0] < 0 {
                            pd += colp[idx][0];
                            pb += 1;
                        } else {
                            pa += colp[idx][0];
                            pc += 1;
                        }
                    }
                    2 => {
                        // `wrapping_add`, matching the reference's `int` centroid accumulation: an
                        // out-of-contract extreme coordinate wraps as the C does instead of
                        // panicking under overflow-checks; in-contract image coordinates never wrap.
                        avn[1] = avn[1].wrapping_add(probe.x[(jj - 1) as usize]);
                        avn[2] = avn[2].wrapping_add(probe.y[(jj - 1) as usize]);
                    }
                    _ => {
                        avn[3] = avn[3].wrapping_add(gallery.x[(jj - 1) as usize]);
                        avn[4] = avn[4].wrapping_add(gallery.y[(jj - 1) as usize]);
                    }
                }
            }
            // build the two sorted, deduplicated endpoint sets
            for ii in 0..2usize {
                for jj in 1..3usize {
                    let p1 = colp[idx][2 * ii + jj];
                    let set = &mut self.yy[tpu][ii];
                    if let Err(pos) = set.binary_search(&p1) {
                        set.insert(pos, p1);
                    }
                }
            }
        }

        if pb == 0 {
            pb = 1;
        } else if pc == 0 {
            pc = 1;
        }
        let mut fi = pa as f32 / pc as f32 - pd as f32 / pb as f32;
        if fi > 180.0 {
            fi = (pa + pd + pb * 360) as f32 / tot as f32;
            if fi > 180.0 {
                fi -= 360.0;
            }
        } else {
            fi = (pa + pd) as f32 / tot as f32;
        }
        let mut rot = round_half_away(fi);
        if rot <= -180 {
            rot += 360;
        }

        let avv_tp = [rot, avn[1] / tot, avn[2] / tot, avn[3] / tot, avn[4] / tot];
        self.avv.push(avv_tp);
        self.ct.push(tot);
        self.gct.push(tot);
        if tot > *match_score {
            *match_score = tot;
        }
        self.ctt.push(0);
        self.ctp.push(vec![tp]);

        for ii in 0..tp {
            let iiu = ii as usize;
            // rotation gate
            let diff = self.avv[tpu][0] - self.avv[iiu][0];
            if diff * diff > TXS && diff * diff < CTXS {
                continue;
            }
            // centroid distance-ratio gate. `wrapping_sub` matching the reference `int`: two
            // centroids from out-of-contract extreme coordinates can differ by more than i32 spans,
            // and the square below is taken in i64; for in-contract coordinates nothing wraps.
            let cl = self.avv[tpu][1].wrapping_sub(self.avv[iiu][1]);
            let cj = self.avv[tpu][2].wrapping_sub(self.avv[iiu][2]);
            let ck = self.avv[tpu][3].wrapping_sub(self.avv[iiu][3]);
            let cji = self.avv[tpu][4].wrapping_sub(self.avv[iiu][4]);
            // Squared in i64 so far-apart centroids cannot overflow the multiply; the sum feeds an
            // f32, and for in-range coordinates the widened value is the same integer.
            let tt = (i64::from(cl) * i64::from(cl) + i64::from(cj) * i64::from(cj)) as f32;
            let ai = (i64::from(cji) * i64::from(cji) + i64::from(ck) * i64::from(ck)) as f32;
            let fi2 = (2.0_f32 * TK) * (tt + ai);
            let dz = tt - ai;
            if dz * dz > fi2 * fi2 {
                continue;
            }
            // connecting-vector angle consistency
            let ang_p = vector_angle(cj, cl);
            let ang_g = vector_angle(cji, ck);
            let (mut ppa, mut ppb, mut ppc, mut ppd) = (0i32, 0i32, 0i32, 0i32);
            if self.avv[tpu][0] < 0 {
                ppd += self.avv[tpu][0];
                ppb += 1;
            } else {
                ppa += self.avv[tpu][0];
                ppc += 1;
            }
            if self.avv[iiu][0] < 0 {
                ppd += self.avv[iiu][0];
                ppb += 1;
            } else {
                ppa += self.avv[iiu][0];
                ppc += 1;
            }
            if ppb == 0 {
                ppb = 1;
            } else if ppc == 0 {
                ppc = 1;
            }
            let mut fi3 = ppa as f32 / ppc as f32 - ppd as f32 / ppb as f32;
            if fi3 > 180.0 {
                fi3 = (ppa + ppd + ppb * 360) as f32 / 2.0;
                if fi3 > 180.0 {
                    fi3 -= 360.0;
                }
            } else {
                fi3 = (ppa + ppd) as f32 / 2.0;
            }
            let mut pbr = round_half_away(fi3);
            if pbr <= -180 {
                pbr += 360;
            }
            let par = iangle180(ang_p - ang_g);
            let kk = (pbr - par) * (pbr - par);
            // PORT: the committed reference form is `kk > TXS && kk < CTXS` (a source comment notes
            // the `SQUARED(kk)` was a typo); reproduced as committed.
            if kk > TXS && kk < CTXS {
                continue;
            }
            // endpoint-disjointness: merge only if the clusters share no endpoint on either side
            let mut found = false;
            for side in 0..2usize {
                if sorted_sets_intersect(&self.yy[iiu][side], &self.yy[tpu][side]) {
                    found = true;
                    break;
                }
            }
            if !found {
                self.gct[iiu] += self.ct[tpu];
                if self.gct[iiu] > *match_score {
                    *match_score = self.gct[iiu];
                }
                self.ctt[iiu] += 1;
                self.ctp[iiu].push(tp);
            }
        }
    }

    /// (7) Advance the mixed-radix odometer to the next endpoint-pairing combination.
    ///
    /// Returns `true` if a further combination remains for this seed (the reference's `j >= 0`
    /// loop condition), applying the selected pairings into `tq`/`rq` as it goes.
    fn odometer(&mut self, ww: &mut i32) -> bool {
        let mut i = 0i32;
        let mut j = *ww - 1;
        while i >= 0 && j >= 0 {
            if self.nn[j as usize] < self.mm[j as usize] {
                self.nn[j as usize] += 1;
                i = *ww - 1;
                while i >= 0 {
                    let rt = self.rx[i as usize];
                    if rt < 0 {
                        let rt = -rt - 1;
                        let z = self.rf[i as usize][(self.nn[i as usize] - 1) as usize] - 1;
                        if (self.tq[z as usize] != rt + 1 && self.tq[z as usize] != 0)
                            || (self.rq[rt as usize] != z + 1 && self.rq[rt as usize] != 0)
                        {
                            break;
                        }
                        self.tq[z as usize] = rt + 1;
                        self.rq[rt as usize] = z + 1;
                        self.rr[i as usize] = z + 1;
                    } else {
                        let rt = rt - 1;
                        let z = self.cf[i as usize][(self.nn[i as usize] - 1) as usize] - 1;
                        if (self.tq[rt as usize] != z + 1 && self.tq[rt as usize] != 0)
                            || (self.rq[z as usize] != rt + 1 && self.rq[z as usize] != 0)
                        {
                            break;
                        }
                        self.tq[rt as usize] = z + 1;
                        self.rq[z as usize] = rt + 1;
                        self.rr[i as usize] = rt + 1;
                    }
                    i -= 1;
                }
                if i >= 0 {
                    for z in (i + 1)..*ww {
                        let n = self.rr[z as usize] - 1;
                        if self.tq[n as usize] > 0 {
                            self.rq[(self.tq[n as usize] - 1) as usize] = 0;
                            self.tq[n as usize] = 0;
                        }
                    }
                    j = *ww - 1;
                }
            } else {
                self.nn[j as usize] = 1;
                j -= 1;
            }
        }
        j >= 0
    }

    /// `bz_final_loop`: the maximum total edge count over a mutually-compatible cluster set.
    fn final_loop(&mut self, tp: i32) -> i32 {
        // sct[member][depth]; depth ≤ chain length ≤ tp, member ≤ tp.
        let cap = (tp + 2).max(2) as usize;
        let mut sct = vec![vec![0i32; cap]; cap];

        let mut match_score = 0i32;
        for ii in 0..tp as usize {
            if match_score >= self.gct[ii] {
                continue;
            }
            let lim0 = self.ctt[ii] + 1;
            for i in 0..lim0 {
                sct[i as usize][0] = self.ctp[ii][i as usize];
            }

            let mut t = 0i32;
            self.y[0] = lim0;
            self.cp[0] = 1;
            let mut b = 0i32;
            let mut n = 1i32;
            loop {
                if self.y[t as usize] - self.cp[t as usize] > 1 {
                    let k = sct[self.cp[t as usize] as usize][t as usize];
                    let jlen = self.ctt[k as usize] + 1;
                    for i in 0..jlen {
                        self.rp[i as usize] = self.ctp[k as usize][i as usize];
                    }
                    // sorted intersection of sct[cp[t]..y[t]][t] with rp[0..jlen]
                    let mut kout = 0i32;
                    let mut kk = self.cp[t as usize];
                    let mut jj = 0i32;
                    loop {
                        // PORT: bound-first reordering of the reference's short-circuited walk.
                        while jj < jlen && self.rp[jj as usize] < sct[kk as usize][t as usize] {
                            jj += 1;
                        }
                        while kk < self.y[t as usize]
                            && self.rp[jj as usize] > sct[kk as usize][t as usize]
                        {
                            kk += 1;
                        }
                        while kk < self.y[t as usize]
                            && jj < jlen
                            && self.rp[jj as usize] == sct[kk as usize][t as usize]
                        {
                            sct[kout as usize][(t + 1) as usize] = sct[kk as usize][t as usize];
                            kout += 1;
                            kk += 1;
                            jj += 1;
                        }
                        if !(kk < self.y[t as usize] && jj < jlen) {
                            break;
                        }
                    }
                    t += 1;
                    self.cp[t as usize] = 1;
                    self.y[t as usize] = kout;
                    b = t;
                    n = 1;
                } else {
                    let mut tot = 0i32;
                    let lim = self.y[t as usize];
                    for i in (n - 1)..lim {
                        tot += self.ct[sct[i as usize][t as usize] as usize];
                    }
                    for i in 0..b {
                        tot += self.ct[sct[0][i as usize] as usize];
                    }
                    if tot > match_score {
                        match_score = tot;
                        for i in 0..b {
                            self.rk[i as usize] = sct[0][i as usize];
                        }
                        let mut rk_index = b;
                        let lim2 = self.y[t as usize];
                        let mut i = n - 1;
                        while i < lim2 {
                            self.rk[rk_index as usize] = sct[i as usize][t as usize];
                            rk_index += 1;
                            i += 1;
                        }
                    }
                    b = t;
                    t -= 1;
                    if t >= 0 {
                        self.cp[t as usize] += 1;
                        n = self.y[t as usize];
                    }
                }
                if t < 0 {
                    break;
                }
            }
        }
        match_score
    }
}

/// The reference's connecting-vector angle: `(180/PI)*atanf(num/den)` with the sign/offset
/// correction folded to `(-180, 180]`, or the `±90` degenerate case when `den == 0`. Default
/// (non-`m1`) sign convention.
fn vector_angle(num: i32, den: i32) -> i32 {
    if den != 0 {
        let mut fi = (180.0_f32 / core::f32::consts::PI) * ((num as f32) / (den as f32)).atan();
        if fi < 0.0 {
            fi += if den < 0 { 180.5 } else { -0.5 };
        } else {
            fi += if den < 0 { -180.5 } else { 0.5 };
        }
        let mut a = fi as i32;
        if a <= -180 {
            a += 360;
        }
        a
    } else if num > 0 {
        90
    } else {
        -90
    }
}

/// Whether two sorted, deduplicated sets share any element (equivalent to the reference's
/// two-pointer endpoint-overlap walk).
fn sorted_sets_intersect(a: &[i32], b: &[i32]) -> bool {
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            core::cmp::Ordering::Less => i += 1,
            core::cmp::Ordering::Greater => j += 1,
            core::cmp::Ordering::Equal => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`vector_angle`] is `atan2(num, den)` in whole degrees, folded to `(-180, 180]` — with two
    /// details that the name does not carry, both pinned below: the `den == 0` degenerate answers
    /// `-90` for `num == 0` as well as for `num < 0`, and `(0, negative)` reaches `180` only via the
    /// `a <= -180` wrap.
    #[test]
    fn vector_angle_is_atan2_in_whole_degrees() {
        for (num, den, want) in [
            // den == 0: the two degenerate arms. `num > 0` is the only route to +90, so the
            // origin — no direction at all — answers -90 rather than 0.
            (1, 0, 90),
            (1000, 0, 90),
            (0, 0, -90),
            (-1, 0, -90),
            // den > 0: ROUND(atan) straight through, both signs of num.
            (0, 10, 0),
            (10, 10, 45),
            (-10, 10, -45),
            (10, 1, 84),
            (-10, 1, -84),
            // den < 0: ±180.5 folds the answer into the correct half-plane.
            (10, -10, 135),
            (-10, -10, -135),
            (10, -1, 96),
            (-10, -1, -96),
            // The `a <= -180` wrap: -180.5 truncates to -180, which is out of the half-open range
            // and is corrected to +180. Without the wrap this pair alone would be -180.
            (0, -10, 180),
        ] {
            assert_eq!(vector_angle(num, den), want, "vector_angle({num}, {den})");
        }
    }

    /// The whole range, against an independent `atan2` oracle: the reference's `atan`-plus-offset
    /// ladder computes the same angle as `atan2` on every direction, the origin excepted.
    ///
    /// A total statement over the directions, so it is worth more than the table above — but the
    /// table stays, because this oracle rounds the same way by construction and would not catch a
    /// changed rounding rule.
    #[test]
    fn vector_angle_agrees_with_atan2_on_every_direction() {
        for num in -60..=60 {
            for den in -60..=60 {
                if num == 0 && den == 0 {
                    continue; // no direction; the table above pins the -90 answer.
                }
                let exact = f64::from(num).atan2(f64::from(den)).to_degrees();
                let mut want = round_half_away(exact as f32);
                if want <= -180 {
                    want += 360;
                }
                assert_eq!(
                    vector_angle(num, den),
                    want,
                    "vector_angle({num}, {den}): atan2 says {exact}"
                );
            }
        }
    }

    /// The centroid at line 548 — `avn[k] / tot` — is an `i32` division, so it **truncates toward
    /// zero**, not toward negative infinity.
    ///
    /// This is the one fact `tests/properties.rs` needs to be honest about translation invariance:
    /// shifting a print by `d` shifts its centroid by exactly `d` only while the coordinate sum
    /// keeps its sign. Where the sum crosses zero, truncation flips from `ceil` to `floor` and the
    /// centroid moves by `d ± 1` — so that test scopes itself to non-negative coordinates and
    /// non-negative shifts, and says why.
    #[test]
    fn centroid_division_truncates_toward_zero() {
        assert_eq!(-7 / 2, -3, "toward zero; floor would be -4");
        // The shift identity translation invariance rests on, and the sign change that breaks it.
        assert_eq!(
            (7 + 2 * 5) / 2,
            7 / 2 + 5,
            "a sum that stays non-negative shifts exactly"
        );
        assert_ne!(
            (-7 + 2 * 5) / 2,
            -7 / 2 + 5,
            "a sum that crosses zero does not: truncation changes direction with the sign"
        );
    }

    /// Two sets share an element, or they do not. The empty set shares nothing — including with
    /// itself, which is what makes a cluster with no endpoints on one side mergeable.
    #[test]
    fn sorted_sets_intersect_finds_a_shared_endpoint() {
        assert!(sorted_sets_intersect(&[1, 5, 9], &[5]));
        assert!(sorted_sets_intersect(&[5], &[1, 5, 9]));
        assert!(sorted_sets_intersect(&[1, 2, 3], &[3, 4, 5]), "at the tail");
        assert!(sorted_sets_intersect(&[3, 4, 5], &[1, 2, 3]), "at the head");
        assert!(
            !sorted_sets_intersect(&[1, 3, 5], &[2, 4, 6]),
            "interleaved but disjoint"
        );
        assert!(!sorted_sets_intersect(&[], &[]));
        assert!(!sorted_sets_intersect(&[1, 2], &[]));
        assert!(!sorted_sets_intersect(&[], &[1, 2]));
    }
}
