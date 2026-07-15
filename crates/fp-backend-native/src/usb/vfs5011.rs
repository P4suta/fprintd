// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validity VFS5011 device constants and init/deinit sequences.
//!
//! ## Provenance (read before editing)
//!
//! Everything here is **original code stating interoperability facts** — the USB vendor/product
//! identity, image geometry, endpoint addresses, and the *shape* of the bring-up/teardown handshake.
//! It is written from spec and observation, **not** transliterated from libfprint's LGPL
//! `vfs5011.c`; copying that file's expression would make this a derivative work of LGPL-2.1+ and is
//! exactly what `ARCHITECTURE.md` forbids for the permissive core.
//!
//! Interoperability *facts* (a USB VID/PID, an endpoint number) are not copyrightable and may be
//! matched freely. But most concrete VFS5011 handshake bytes are **not yet confirmed here**: those
//! values are marked "HW-verified: required" and carry deliberate placeholders, so this module never
//! asserts as fact a byte it has not observed. The init/deinit sequences below are structurally
//! plausible (reset → configure → start; stop → reset) but must be finalized against a physical
//! sensor before any real capture will succeed.

// The USB identity below is this driver's canonical, HW-verifiable device match. No enumerator
// consumes it yet (USB device discovery is a later stage), so it is `allow(dead_code)` today; it is
// part of the driver's definition and is asserted by this module's tests. Kept as named constants
// rather than deleted so the identity lives with the driver, not scattered in a future enumerator.

/// USB vendor id: Validity Sensors, Inc.
///
/// Interoperability fact (the assigned USB vendor id), safe to match.
#[allow(dead_code)] // Consumed by USB enumeration (a later stage); see the note above.
pub const VENDOR_ID: u16 = 0x138a;

/// USB product id of a VFS5011 unit.
///
/// Interoperability fact for the VFS5011 family; several product ids share this driver
/// (0x0005/0x0007/0x0008/0x000c/0x0011…). One representative id is recorded here; enumeration should
/// accept the whole family. HW-verified: required to confirm the exact set for a given unit.
#[allow(dead_code)] // Consumed by USB enumeration (a later stage); see the note above.
pub const PRODUCT_ID: u16 = 0x0011;

/// Bulk-in endpoint address (device-to-host image stream).
///
/// HW-verified: required. Placeholder endpoint address; confirm against the sensor's descriptors.
pub const EP_IN: u8 = 0x81;

/// Bulk-out endpoint address (host-to-device commands).
///
/// HW-verified: required. Placeholder endpoint address; confirm against the sensor's descriptors.
pub const EP_OUT: u8 = 0x02;

/// Nominal captured image width in pixels.
///
/// The VFS5011 is a swipe sensor with a fixed-width scan line. `WIDTH * HEIGHT` is used by the
/// driver as an upper bound on a single frame's payload, so a garbled header cannot request an
/// absurd bulk-in. HW-verified: required to confirm the exact width.
pub const WIDTH: usize = 256;

/// Nominal captured image height in pixels.
///
/// A swipe sensor accumulates a variable number of lines; this is a nominal assembled height whose
/// product with [`WIDTH`] bounds a single frame's payload (see [`WIDTH`]). HW-verified: required.
pub const HEIGHT: usize = 256;

/// Upper bound on a single assembled frame's payload, in bytes (`WIDTH * HEIGHT`).
///
/// The driver rejects any frame header claiming more than this, so a corrupt or hostile header
/// cannot drive an unbounded bulk-in read.
pub const MAX_FRAME_BYTES: usize = WIDTH * HEIGHT;

/// Scan resolution in pixels-per-inch recorded on every captured [`crate::Frame`].
///
/// HW-verified: required. 500 ppi is the NBIS reference resolution and a sane placeholder; the real
/// VFS5011 resolution must be confirmed so MINDTCT's resolution-relative thresholds are honest.
pub const PPI: u16 = 500;

/// One step of a device bring-up or teardown handshake.
///
/// A sequence of these is what [`init_sequence`] / [`deinit_sequence`] describe and what
/// `crate::usb::UsbFrameSource` replays through a [`crate::UsbTransport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitStep {
    /// A control transfer: `bmRequestType`, `bRequest`, `wValue`, `wIndex`, then the payload.
    Control {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: Vec<u8>,
    },
    /// A command written to the bulk-out endpoint.
    BulkOut(Vec<u8>),
}

/// The device bring-up handshake, replayed by `crate::usb::UsbFrameSource::arm`.
///
/// HW-verified: required. The *structure* — a vendor control reset, then a bulk-out configure —
/// is plausible for this family, but the concrete bytes are placeholders to be confirmed on
/// hardware. Written originally from that structural spec, not copied from `vfs5011.c`.
#[must_use]
pub fn init_sequence() -> Vec<InitStep> {
    vec![
        // Vendor reset (device recipient, host-to-device). Placeholder request/bytes.
        InitStep::Control {
            request_type: 0x40, // OUT | Vendor | Device
            request: 0x01,
            value: 0x0000,
            index: 0x0000,
            data: Vec::new(),
        },
        // Configure/prepare the imager. Placeholder command byte.
        InitStep::BulkOut(vec![0x01]),
    ]
}

/// The device teardown handshake, replayed by `crate::usb::UsbFrameSource::disarm`.
///
/// HW-verified: required. Placeholder stop-then-reset; confirm on hardware.
#[must_use]
pub fn deinit_sequence() -> Vec<InitStep> {
    vec![
        // Stop imaging. Placeholder command byte.
        InitStep::BulkOut(vec![0x00]),
        // Vendor reset back to idle. Placeholder request.
        InitStep::Control {
            request_type: 0x40, // OUT | Vendor | Device
            request: 0x01,
            value: 0x0000,
            index: 0x0000,
            data: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_constants_are_the_documented_interop_facts() {
        assert_eq!(VENDOR_ID, 0x138a);
        assert_eq!(PRODUCT_ID, 0x0011);
    }

    #[test]
    fn sequences_are_non_empty_and_structured() {
        assert!(!init_sequence().is_empty());
        assert!(!deinit_sequence().is_empty());
    }
}
