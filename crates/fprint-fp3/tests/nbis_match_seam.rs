// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end proof of the host-image slice, hardware-free: a real minutiae [`Print`] survives the
//! **FP3 on-disk round-trip** and still scores as a match under the real **BOZORTH3** matcher.
//!
//! This ties three crates together the way a live host-image driver would: `fprint-core` (the domain
//! `Print`/`Template::Nbis`), `fprint-fp3` (the wire codec), and `fprint-bozorth3` (the matcher, reached via
//! `fprint-backend-native`'s `nbis_match_score` seam). It exercises what the virtual device's
//! deterministic stub deliberately does not — genuine fuzzy minutiae matching — without any sensor:
//! enroll capture A, persist it as FP3, read it back, then match a *different* capture B of the same
//! synthetic finger (small jitter) and confirm it clears a threshold, while an unrelated finger does
//! not.

use fprint_backend_native::{nbis_identify, nbis_match_score};
use fprint_core::{Minutia, Print, Template};

/// A conventional BOZORTH3 accept threshold; a same-finger recapture clears it comfortably while an
/// unrelated finger scores near zero.
const THRESHOLD: u32 = 40;

/// Deterministic minutiae set of `n` points with unique coordinates (a tiny LCG — no RNG crate).
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

/// A small per-minutia jitter (simulates recapturing the same finger).
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

fn nbis_print(samples: Vec<Vec<Minutia>>) -> Print {
    Print {
        template: Template::Nbis(samples),
        finger: Some(fprint_core::Finger::LeftIndex),
        driver: Some(fprint_core::DriverId("virtual_image".into())),
        device_id: Some(fprint_core::DeviceId("seam-test".into())),
        ..Print::default()
    }
}

#[test]
fn enrolled_nbis_print_survives_fp3_and_matches_via_bozorth3() {
    let finger_a = synth(40, 2026);
    let recapture_a = jitter(&finger_a, 99); // same finger, different capture
    let finger_b = synth(40, 777); // an unrelated finger

    // Enroll capture A and persist it as FP3, exactly as the daemon's store would.
    let enrolled = nbis_print(vec![finger_a.clone()]);
    let bytes = fprint_fp3::to_bytes(&enrolled).expect("serialize FP3");
    assert!(bytes.starts_with(fprint_fp3::MAGIC));
    let back = fprint_fp3::from_bytes(&bytes).expect("deserialize FP3");
    assert_eq!(enrolled, back, "FP3 round-trip must be exact");

    // The read-back enrolled template matches a fresh capture of the same finger...
    let same = nbis_match_score(&back.template, &Template::Nbis(vec![recapture_a]));
    assert!(
        same >= THRESHOLD,
        "same-finger recapture should match (score {same} < {THRESHOLD})"
    );

    // ...but not an unrelated finger.
    let different = nbis_match_score(&back.template, &Template::Nbis(vec![finger_b]));
    assert!(
        different < THRESHOLD,
        "unrelated finger should not match (score {different} >= {THRESHOLD})"
    );

    assert!(
        same > different,
        "self ({same}) must beat cross ({different})"
    );
}

#[test]
fn identify_picks_the_matching_gallery_entry() {
    // A gallery of three unrelated enrolled fingers, plus a probe that is a jittered recapture of
    // the middle one. Identify must return index 1 (and None when the probe is a fourth finger).
    let gallery: Vec<Template> = [11, 22, 33]
        .iter()
        .map(|&seed| Template::Nbis(vec![synth(40, seed)]))
        .collect();

    let probe_of_1 = Template::Nbis(vec![jitter(&synth(40, 22), 5)]);
    assert_eq!(
        nbis_identify(&probe_of_1, &gallery, THRESHOLD),
        Some(1),
        "identify should find the recaptured finger at index 1"
    );

    let stranger = Template::Nbis(vec![synth(40, 999)]);
    assert_eq!(
        nbis_identify(&stranger, &gallery, THRESHOLD),
        None,
        "an unenrolled finger must identify to nobody"
    );
}

#[test]
fn raw_moc_template_is_never_host_matched() {
    // A match-on-chip Raw handle is opaque; host-side scoring must decline (score 0), regardless.
    let raw = Template::Raw(b"opaque".to_vec());
    let nbis = Template::Nbis(vec![synth(20, 1)]);
    assert_eq!(nbis_match_score(&raw, &nbis), 0);
    assert_eq!(nbis_match_score(&nbis, &raw), 0);
    assert_eq!(nbis_match_score(&raw, &raw), 0);
}
