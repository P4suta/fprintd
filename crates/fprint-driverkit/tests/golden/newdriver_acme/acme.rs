// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Acme device constants and init/deinit sequences.
//!
//! ## Provenance (read before editing)
//!
//! Everything here is **original code stating interoperability facts** — the USB vendor/product
//! identity, image geometry, endpoint addresses, and the *shape* of the bring-up/teardown handshake.
//! It is written from spec and observation, not transliterated from any libfprint driver; copying
//! that expression would make this a derivative work and break the permissive core (see
//! `ARCHITECTURE.md` §Provenance & licensing).
//!
//! A USB VID/PID and an endpoint number are interoperability facts, safe to match. Most concrete
//! handshake bytes below are **not yet confirmed**: they are marked `HW-verified: required` and carry
//! deliberate placeholders, so this module never asserts as fact a byte it has not observed. The
//! sequences are structurally plausible (reset → configure → start; stop → reset) but must be
//! finalized against a physical sensor before any real capture will succeed.

use crate::usb::wire::UsbTransfer;

// The USB identity below is this driver's canonical, HW-verifiable device match, asserted by this
// module's tests. It is the match a future enumerator keys on.

/// USB vendor id claimed by this driver.
///
/// HW-verified: required. Interoperability fact (the assigned USB vendor id); confirm it matches the
/// sensor's descriptors.
pub const VENDOR_ID: u16 = 0x1c7a;

/// USB product id claimed by this driver.
///
/// HW-verified: required. Interoperability fact; confirm it matches the sensor's descriptors, and
/// widen to the whole product-id family the driver should accept.
pub const PRODUCT_ID: u16 = 0x0570;

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
/// `WIDTH * HEIGHT` bounds a single frame's payload, so a garbled header cannot request an absurd
/// bulk-in. HW-verified: required to confirm the exact width.
pub const WIDTH: usize = 256;

/// Nominal captured image height in pixels.
///
/// The product with [`WIDTH`] bounds a single frame's payload (see [`WIDTH`]). HW-verified: required.
pub const HEIGHT: usize = 256;

/// Upper bound on a single assembled frame's payload, in bytes (`WIDTH * HEIGHT`).
///
/// The driver rejects any frame header claiming more than this, so a corrupt or hostile header
/// cannot drive an unbounded bulk-in read.
pub const MAX_FRAME_BYTES: usize = WIDTH * HEIGHT;

/// Scan resolution in pixels-per-inch stamped on every captured [`crate::Frame`].
///
/// HW-verified: required. 500 ppi is the NBIS reference resolution and a sane placeholder; the real
/// Acme resolution must be confirmed, because MINDTCT's thresholds are relative to it.
pub const PPI: u16 = 500;

/// The device bring-up handshake, replayed by the frame source's `arm`.
///
/// A bring-up step is a host-to-device [`UsbTransfer`] (a control transfer or a bulk-out write), so
/// the handshake shares the one wire vocabulary the rest of the driver records and replays; a
/// [`UsbTransfer::BulkIn`] never appears here because arm/disarm only write.
///
/// HW-verified: required. The *structure* — a vendor control reset, then a bulk-out configure — is
/// plausible, but the concrete bytes are placeholders to confirm on hardware.
#[must_use]
pub fn init_sequence() -> Vec<UsbTransfer> {
    vec![
        // HW-verified: required. Vendor reset (device recipient, host-to-device). Placeholder bytes.
        UsbTransfer::Control {
            request_type: 0x40, // OUT | Vendor | Device
            request: 0x01,
            value: 0x0000,
            index: 0x0000,
            data: Vec::new(),
        },
        // HW-verified: required. Configure/prepare the imager. Placeholder command byte.
        UsbTransfer::BulkOut {
            ep: EP_OUT,
            data: vec![0x01],
        },
    ]
}

/// The device teardown handshake, replayed by the frame source's `disarm`.
///
/// HW-verified: required. Placeholder stop-then-reset; confirm on hardware.
#[must_use]
pub fn deinit_sequence() -> Vec<UsbTransfer> {
    vec![
        // HW-verified: required. Stop imaging. Placeholder command byte.
        UsbTransfer::BulkOut {
            ep: EP_OUT,
            data: vec![0x00],
        },
        // HW-verified: required. Vendor reset back to idle. Placeholder request.
        UsbTransfer::Control {
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
        assert_eq!(VENDOR_ID, 0x1c7a);
        assert_eq!(PRODUCT_ID, 0x0570);
    }

    #[test]
    fn sequences_are_non_empty_and_structured() {
        assert!(!init_sequence().is_empty());
        assert!(!deinit_sequence().is_empty());
    }
}
