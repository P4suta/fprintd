// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The whole host-image loop in code: detect minutiae from a frame, then enroll a finger, verify the
//! same finger (PASS), and present a different finger (REJECT).
//!
//! Run it with `cargo run -p fprint-pipeline --example enroll_verify`. It needs no hardware and no
//! Docker: the detection half runs the real MINDTCT extractor over a procedural frame, and the
//! matching half scores deterministic synthetic minutiae so the PASS/REJECT verdicts are
//! reproducible. A real driver would feed captured frames to both halves.

use fprint_pipeline::fprint_core::{Minutia, Template};
use fprint_pipeline::{extract_minutiae, nbis_identify, nbis_match_score, nbis_verify, GrayImage};

/// A conventional BOZORTH3 accept threshold: a same-finger recapture clears it comfortably while an
/// unrelated finger scores near zero.
const THRESHOLD: u32 = 40;

fn main() {
    println!("== fprint-pipeline: enroll / verify / reject ==\n");

    detection_half();
    let ok = matching_half();

    if ok {
        println!("\nAll verdicts as expected.");
    } else {
        eprintln!("\nA verdict did not hold — see above.");
        std::process::exit(1);
    }
}

/// The front half: a procedural frame → minutiae, through the real MINDTCT extractor.
fn detection_half() {
    let (w, h) = (128usize, 128usize);
    let frame: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let on_ridge = (y % 8) < 4;
            // Cut a gap into every other ridge; a gap ends a ridge, and a ridge ending is a minutia.
            let gap = (48..80).contains(&x) && (y / 8) % 2 == 0;
            if on_ridge && !gap {
                32
            } else {
                224
            }
        })
        .collect();

    let img = GrayImage::new(&frame, w, h, 500).expect("buffer holds the image");
    let minutiae = extract_minutiae(img);
    println!(
        "detect: MINDTCT found {} minutiae in a {w}x{h} procedural frame",
        minutiae.len()
    );
}

/// The back half: enroll a finger, verify the same finger (PASS), reject a different one (REJECT),
/// and identify the right gallery entry. Returns whether every verdict held.
fn matching_half() -> bool {
    let finger_a = synth(40, 2026);
    let recapture_a = jitter(&finger_a, 99); // the same finger, a fresh capture
    let finger_b = synth(40, 777); // an unrelated finger

    let enrolled = Template::Nbis(vec![finger_a]);
    let same = Template::Nbis(vec![recapture_a]);
    let other = Template::Nbis(vec![finger_b]);

    // The 1:1 decision goes through `nbis_verify`; `nbis_match_score` reads the raw score to print.
    let pass = nbis_verify(&enrolled, &same, THRESHOLD);
    let reject = !nbis_verify(&enrolled, &other, THRESHOLD);
    println!(
        "verify: same finger scored {:?} -> {}",
        nbis_match_score(&enrolled, &same).score(),
        verdict(pass, "PASS", "unexpected REJECT")
    );
    println!(
        "verify: other finger scored {:?} -> {}",
        nbis_match_score(&enrolled, &other).score(),
        verdict(reject, "REJECT", "unexpected PASS")
    );

    // 1:N identify over a small gallery: the probe is a recapture of entry 1.
    let gallery = vec![enrolled.clone(), same.clone(), other.clone()];
    let found = nbis_identify(&same, &gallery, THRESHOLD);
    let identified = found == Some(0) || found == Some(1);
    println!(
        "identify: probe matched gallery index {found:?} -> {}",
        verdict(identified, "OK", "unexpected miss")
    );

    pass && reject && identified
}

fn verdict(ok: bool, yes: &str, no: &str) -> String {
    if ok {
        yes.to_string()
    } else {
        format!("{no}!")
    }
}

/// A deterministic minutiae set of `n` points with unique coordinates (a tiny LCG — no RNG crate).
fn synth(n: usize, seed: u64) -> Vec<Minutia> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (s >> 33) as i32
    };
    let mut pts = Vec::new();
    let mut used = std::collections::HashSet::new();
    while pts.len() < n {
        let x = 40 + next().rem_euclid(420);
        let y = 40 + next().rem_euclid(420);
        if !used.insert((x, y)) {
            continue;
        }
        let theta = next().rem_euclid(360);
        pts.push(Minutia { x, y, theta });
    }
    pts
}

/// A small per-minutia jitter, simulating recapturing the same finger.
fn jitter(pts: &[Minutia], seed: u64) -> Vec<Minutia> {
    let mut s = seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(1);
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (s >> 33) as i32
    };
    let mut used = std::collections::HashSet::new();
    pts.iter()
        .map(|m| {
            let mut x = m.x + next().rem_euclid(5) - 2;
            let y = m.y + next().rem_euclid(5) - 2;
            while !used.insert((x, y)) {
                x += 1;
            }
            Minutia {
                x,
                y,
                theta: (m.theta + next().rem_euclid(5) - 2).rem_euclid(360),
            }
        })
        .collect()
}
