// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`UsbTransport`]: the minimal async bulk/control seam the USB driver drives.
//!
//! The driver ([`super::source::UsbFrameSource`]) speaks only these three methods; what carries the
//! bytes is a parameter. The real carrier is [`NusbTransport`] over [`nusb`], compiled only under
//! `#[cfg(feature = "usb")]` so a default build has no USB dependency; a scripted mock over the same
//! trait exercises the driver offline (see the `mock_tests` module).
//!
//! **Hardware verification.** [`NusbTransport`]'s method bodies map onto `nusb` 0.1's transfer API,
//! but no byte of real I/O — nor the precise `nusb` call shapes — can be confirmed without a
//! physical Validity sensor. The whole `NusbTransport` block is therefore marked "HW-verified:
//! required": it is the best compilable rendering of the intended calls, to be reconciled against
//! hardware. Nothing above it (the trait, the protocol, the driver) depends on that reconciliation.

use fprint_core::Result;

/// An async USB bulk/control transport.
///
/// `async fn` in this trait is deliberate and mirrors [`fprint_core::Device`] and
/// [`crate::FrameSource`]: the driver is generic over a concrete transport (static dispatch), so no
/// caller ever needs a `+ Send` bound and the desugared return type never has to be named. The
/// `async_fn_in_trait` lint's `Send`-bound caveat therefore does not apply here.
#[allow(async_fn_in_trait)] // Static dispatch (no `+ Send` needed), same rationale as `fprint_core::Device`.
pub trait UsbTransport {
    /// Write `data` to bulk-out endpoint `ep`.
    async fn bulk_out(&mut self, ep: u8, data: &[u8]) -> Result<()>;

    /// Read up to `len` bytes from bulk-in endpoint `ep`, returning what arrived.
    async fn bulk_in(&mut self, ep: u8, len: usize) -> Result<Vec<u8>>;

    /// Issue a control transfer. `request_type` is the raw USB `bmRequestType` byte; its direction
    /// bit selects an IN or OUT transfer. For an OUT transfer `data` is the payload and the returned
    /// vector is empty; for an IN transfer `data.len()` is the number of bytes requested and the
    /// returned vector is the response.
    async fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>>;
}

// --- Live bus enumeration (feature-gated) --------------------------------------------------------

/// A USB device present on the bus, as [`list_usb_devices`] reports it.
///
/// Enough to identify and label a device — its [`UsbId`](super::UsbId) plus the manufacturer/product
/// descriptor strings when the OS exposes them — without opening it. Unlike the transfer path, plain
/// enumeration touches no endpoint, so this is verifiable on any host with USB devices attached.
#[cfg(feature = "usb")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbDeviceInfo {
    /// The device's `(vid, pid)`.
    pub id: super::UsbId,
    /// The USB manufacturer-descriptor string, when the OS exposes it.
    pub manufacturer: Option<String>,
    /// The USB product-descriptor string, when the OS exposes it.
    pub product: Option<String>,
}

/// Enumerate the USB devices currently attached to the bus.
///
/// A read-only listing over [`nusb::list_devices`] — it opens nothing and streams no bytes, so it
/// needs no specific sensor and runs on any host. `fpdev probe` uses it to classify every attached
/// device against the interoperability database.
///
/// # Errors
/// [`fprint_core::Error::Transport`] if the platform USB backend cannot enumerate the bus.
#[cfg(feature = "usb")]
pub fn list_usb_devices() -> Result<Vec<UsbDeviceInfo>> {
    let devices = nusb::list_devices()
        .map_err(|e| fprint_core::Error::Transport(format!("list USB devices: {e}")))?;
    Ok(devices
        .map(|d| UsbDeviceInfo {
            id: super::UsbId {
                vid: d.vendor_id(),
                pid: d.product_id(),
            },
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
        })
        .collect())
}

// --- Real nusb-backed transport (feature-gated; hardware-only verification) ----------------------

/// A [`UsbTransport`] over a claimed [`nusb::Interface`].
///
/// HW-verified: required for the entire `impl` below. The bodies render the intended `nusb` 0.1
/// transfer calls, but the actual endpoint behaviour — and any drift in `nusb`'s exact API — can
/// only be confirmed against a physical Validity VFS5011. This type exists so
/// `ImageDevice<UsbFrameSource<NusbTransport>>` type-checks and links; it is not exercised by the
/// offline test suite.
#[cfg(feature = "usb")]
pub struct NusbTransport {
    iface: nusb::Interface,
}

#[cfg(feature = "usb")]
impl NusbTransport {
    /// Wrap an already-claimed interface.
    pub fn new(iface: nusb::Interface) -> Self {
        NusbTransport { iface }
    }

    /// Open the first bus device matching `id` and claim its interface 0.
    ///
    /// HW-verified: required. The enumeration and claim shapes render `nusb` 0.1's API, and
    /// interface 0 is the assumed control/bulk interface; the real interface number, and that any
    /// bytes flow at all, can only be confirmed against a physical sensor. This is the live seam
    /// `fpdev shell` drives; nothing offline depends on it.
    ///
    /// # Errors
    /// [`fprint_core::Error::Transport`] if no matching device is present, or the OS refuses the
    /// open or the interface claim.
    pub fn open(id: super::UsbId) -> Result<Self> {
        let info = nusb::list_devices()
            .map_err(|e| fprint_core::Error::Transport(format!("list USB devices: {e}")))?
            .find(|d| d.vendor_id() == id.vid && d.product_id() == id.pid)
            .ok_or_else(|| {
                fprint_core::Error::Transport(format!(
                    "no device {:04x}:{:04x} on the bus",
                    id.vid, id.pid
                ))
            })?;
        let device = info.open().map_err(|e| {
            fprint_core::Error::Transport(format!("open {:04x}:{:04x}: {e}", id.vid, id.pid))
        })?;
        let iface = device
            .claim_interface(0)
            .map_err(|e| fprint_core::Error::Transport(format!("claim interface 0: {e}")))?;
        Ok(NusbTransport { iface })
    }
}

#[cfg(feature = "usb")]
impl UsbTransport for NusbTransport {
    async fn bulk_out(&mut self, ep: u8, data: &[u8]) -> Result<()> {
        // HW-verified: required. `nusb` 0.1 takes an owned buffer for an OUT transfer and returns a
        // `Completion`; `into_result` surfaces the transfer status.
        let completion = self.iface.bulk_out(ep, data.to_vec()).await;
        completion
            .into_result()
            .map(|_| ())
            .map_err(|e| fprint_core::Error::Transport(format!("bulk_out ep {ep:#04x}: {e}")))
    }

    async fn bulk_in(&mut self, ep: u8, len: usize) -> Result<Vec<u8>> {
        // HW-verified: required. A `RequestBuffer` sizes the read; the completion yields the bytes.
        let completion = self
            .iface
            .bulk_in(ep, nusb::transfer::RequestBuffer::new(len))
            .await;
        completion
            .into_result()
            .map_err(|e| fprint_core::Error::Transport(format!("bulk_in ep {ep:#04x}: {e}")))
    }

    async fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>> {
        use nusb::transfer::{ControlIn, ControlOut};

        // HW-verified: required. Decode the raw bmRequestType into nusb's typed fields, then issue
        // an IN or OUT control transfer per its direction bit.
        let control_type = decode_control_type(request_type);
        let recipient = decode_recipient(request_type);

        if request_type & 0x80 != 0 {
            // Device-to-host: `data.len()` is the number of bytes to read.
            let length = u16::try_from(data.len()).map_err(|_| {
                fprint_core::Error::Transport("control IN length exceeds u16".to_string())
            })?;
            let completion = self
                .iface
                .control_in(ControlIn {
                    control_type,
                    recipient,
                    request,
                    value,
                    index,
                    length,
                })
                .await;
            completion.into_result().map_err(|e| {
                fprint_core::Error::Transport(format!("control_in req {request:#04x}: {e}"))
            })
        } else {
            let completion = self
                .iface
                .control_out(ControlOut {
                    control_type,
                    recipient,
                    request,
                    value,
                    index,
                    data,
                })
                .await;
            completion.into_result().map(|_| Vec::new()).map_err(|e| {
                fprint_core::Error::Transport(format!("control_out req {request:#04x}: {e}"))
            })
        }
    }
}

/// Decode the type field (bits 6..5) of a USB `bmRequestType` byte.
#[cfg(feature = "usb")]
fn decode_control_type(request_type: u8) -> nusb::transfer::ControlType {
    use nusb::transfer::ControlType;
    match (request_type >> 5) & 0b11 {
        0 => ControlType::Standard,
        1 => ControlType::Class,
        _ => ControlType::Vendor, // 2 = Vendor; 3 (Reserved) has no nusb variant, fold to Vendor.
    }
}

/// Decode the recipient field (bits 4..0) of a USB `bmRequestType` byte.
#[cfg(feature = "usb")]
fn decode_recipient(request_type: u8) -> nusb::transfer::Recipient {
    use nusb::transfer::Recipient;
    match request_type & 0b1_1111 {
        0 => Recipient::Device,
        1 => Recipient::Interface,
        2 => Recipient::Endpoint,
        _ => Recipient::Other,
    }
}
