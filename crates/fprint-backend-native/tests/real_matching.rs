// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Genuine BOZORTH3 matching through the whole [`fprint_core::Device`] trait, hardware-free.
//!
//! The virtual device's default matcher is a non-biometric byte-equality stub. Built with
//! [`VirtualDeviceBuilder::bozorth3_matching`] and fed real minutiae via
//! [`Scenario::enroll_real`] / [`Scenario::present_real`], it instead runs the real matcher — so
//! `enroll` → `verify` / `identify` exercises actual fingerprint matching across the same async
//! seam the fprintd daemon drives, with no sensor. A same-finger recapture matches; an unrelated
//! finger does not; identify picks the right gallery slot.

use fprint_testkit::block_on;

use fprint_backend_native::{Scenario, VirtualDeviceBuilder};
use fprint_core::{Device, Finger, Minutia, Print, Template};

const THRESHOLD: u32 = 40;

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
        if used.insert((x, y)) {
            pts.push(Minutia {
                x,
                y,
                theta: next().rem_euclid(360),
            });
        }
    }
    pts
}

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

fn nbis(samples: Vec<Vec<Minutia>>) -> Template {
    Template::Nbis(samples)
}

/// Enroll a finger, then verify against a scan of `present` — returns whether it matched.
fn enroll_then_verify(enroll: Vec<Minutia>, present: Vec<Minutia>) -> bool {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .bozorth3_matching(THRESHOLD)
        .scenario(
            Scenario::new()
                .enroll_real(nbis(vec![enroll]))
                .present_real(nbis(vec![present])),
        )
        .build();
    block_on(dev.open()).unwrap();
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |_p| {})).unwrap();
    assert!(matches!(enrolled.template, Template::Nbis(_)));
    block_on(dev.verify(&enrolled)).unwrap().matched
}

#[test]
fn same_finger_recapture_matches_over_the_device_trait() {
    let finger = synth(40, 2026);
    let recapture = jitter(&finger, 7);
    assert!(
        enroll_then_verify(finger, recapture),
        "a recapture of the enrolled finger must verify"
    );
}

#[test]
fn unrelated_finger_does_not_match() {
    let finger = synth(40, 2026);
    let stranger = synth(40, 555);
    assert!(
        !enroll_then_verify(finger, stranger),
        "an unrelated finger must not verify"
    );
}

#[test]
fn identify_selects_the_matching_gallery_entry() {
    let finger = synth(40, 2026);
    let recapture = jitter(&finger, 7);

    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .bozorth3_matching(THRESHOLD)
        .scenario(Scenario::new().present_real(nbis(vec![recapture])))
        .build();
    block_on(dev.open()).unwrap();

    // Gallery: two strangers around the true finger at index 1.
    let gallery = vec![
        Print {
            template: nbis(vec![synth(40, 111)]),
            ..Print::default()
        },
        Print {
            template: nbis(vec![finger]),
            ..Print::default()
        },
        Print {
            template: nbis(vec![synth(40, 333)]),
            ..Print::default()
        },
    ];
    let outcome = block_on(dev.identify(&gallery)).unwrap();
    assert_eq!(outcome.match_index, Some(1), "identify should hit index 1");
}

#[test]
fn synthetic_stub_is_unaffected_by_default() {
    // Without `bozorth3_matching`, matching stays the deterministic byte-equality stub: an NBIS
    // template only matches a byte-identical one, so two different real captures do NOT match.
    let finger = synth(40, 2026);
    let recapture = jitter(&finger, 7);
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new()
                .enroll_real(nbis(vec![finger]))
                .present_real(nbis(vec![recapture])),
        )
        .build();
    block_on(dev.open()).unwrap();
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |_p| {})).unwrap();
    assert!(
        !block_on(dev.verify(&enrolled)).unwrap().matched,
        "the stub matcher must not fuzzy-match distinct captures"
    );
}
