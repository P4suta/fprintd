// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Offline import tests over hand-authored fixtures.
//!
//! Three encodings of one bring-up exchange — a pcapng USBPcap trace, a classic-pcap usbmon trace,
//! and a usbmon text log — decode to the same [`Session`], proving each parser independent of any
//! sensor. `HW-verified: required` covers only the claim that a real device emits these exact bytes;
//! that the parsers read the bytes correctly is what these assert. A busy two-device log exercises
//! the vendor/product and bus/address device filters, and a full import round-trips through the
//! cassette writer.

use super::*;

use fprint_backend_native::UsbTransfer;

/// Path to a committed fixture under `tests/fixtures/`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Import arguments over a fixture with no device filter and a chosen format.
fn args(name: &str, format: ImportFormat) -> ImportArgs {
    ImportArgs {
        input: fixture(name),
        vid: None,
        pid: None,
        bus: None,
        addr: None,
        out: None,
        format,
    }
}

/// The device descriptor the fixtures return: idVendor 0x138a at offset 8, idProduct 0x0011 at
/// offset 10.
fn descriptor() -> Vec<u8> {
    vec![
        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x8a, 0x13, 0x11, 0x00, 0x00, 0x01, 0x01,
        0x02, 0x03, 0x01,
    ]
}

/// The session every single-device fixture must import to: the descriptor read, a capture command,
/// then a frame header and its pixels as two bulk-in reads, all tagged to device 138a:0011.
fn expected_session() -> Session {
    let mut session = Session::for_device(UsbId {
        vid: 0x138a,
        pid: 0x0011,
    });
    session
        .push(UsbTransfer::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0x0000,
            data: descriptor(),
        })
        .push(UsbTransfer::BulkOut {
            ep: 0x02,
            data: vec![0x50],
        })
        .push(UsbTransfer::BulkIn {
            ep: 0x81,
            data: vec![0x01, 0xfe, 0x04, 0x00, 0x02, 0x00],
        })
        .push(UsbTransfer::BulkIn {
            ep: 0x81,
            data: vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
        });
    session
}

#[test]
fn usbpcap_pcapng_imports_to_the_expected_session() {
    let session = import(&args("usbpcap.pcapng", ImportFormat::Auto)).unwrap();
    assert_eq!(session, expected_session());
}

#[test]
fn usbmon_binary_pcap_imports_to_the_expected_session() {
    let session = import(&args("usbmon.pcap", ImportFormat::Auto)).unwrap();
    assert_eq!(session, expected_session());
}

#[test]
fn usbmon_text_imports_to_the_expected_session() {
    let session = import(&args("usbmon.txt", ImportFormat::Auto)).unwrap();
    assert_eq!(session, expected_session());
}

#[test]
fn every_format_agrees() {
    let a = import(&args("usbpcap.pcapng", ImportFormat::Auto)).unwrap();
    let b = import(&args("usbmon.pcap", ImportFormat::Auto)).unwrap();
    let c = import(&args("usbmon.txt", ImportFormat::Auto)).unwrap();
    assert_eq!(a, b);
    assert_eq!(b, c);
}

#[test]
fn explicit_format_matches_auto_detection() {
    assert_eq!(
        import(&args("usbpcap.pcapng", ImportFormat::Usbpcap)).unwrap(),
        import(&args("usbpcap.pcapng", ImportFormat::Auto)).unwrap()
    );
    assert_eq!(
        import(&args("usbmon.pcap", ImportFormat::Usbmon)).unwrap(),
        import(&args("usbmon.pcap", ImportFormat::Auto)).unwrap()
    );
    assert_eq!(
        import(&args("usbmon.txt", ImportFormat::Usbmon)).unwrap(),
        import(&args("usbmon.txt", ImportFormat::Auto)).unwrap()
    );
}

#[test]
fn pcapng_format_rejects_a_text_log() {
    let err = import(&args("usbmon.txt", ImportFormat::Pcapng)).unwrap_err();
    assert!(matches!(err, ImportError::Pcap(_)));
}

#[test]
fn busy_log_imports_both_devices_untagged() {
    let session = import(&args("usbmon_busy.txt", ImportFormat::Auto)).unwrap();
    // Two devices seen, so no single identity is assumed, but every transfer is kept.
    assert_eq!(session.device, None);
    assert_eq!(session.transfers.len(), 7);
}

#[test]
fn vid_pid_filter_isolates_one_device_from_a_busy_log() {
    let mut a = args("usbmon_busy.txt", ImportFormat::Auto);
    a.vid = Some("138a".into());
    a.pid = Some("0011".into());
    let session = import(&a).unwrap();
    assert_eq!(session, expected_session());
}

#[test]
fn bus_addr_filter_isolates_the_other_device() {
    let mut a = args("usbmon_busy.txt", ImportFormat::Auto);
    a.bus = Some("1".into());
    a.addr = Some("9".into());
    let session = import(&a).unwrap();

    let mut expected = Session::for_device(UsbId {
        vid: 0x1111,
        pid: 0x2222,
    });
    expected
        .push(UsbTransfer::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0x0000,
            data: vec![
                0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x11, 0x11, 0x22, 0x22, 0x00, 0x01,
                0x01, 0x02, 0x03, 0x01,
            ],
        })
        .push(UsbTransfer::BulkOut {
            ep: 0x03,
            data: vec![0x0d, 0xfe],
        })
        .push(UsbTransfer::BulkIn {
            ep: 0x84,
            data: vec![0xaa, 0xbb, 0xcc],
        });
    assert_eq!(session, expected);
}

#[test]
fn vid_pid_filter_without_a_captured_descriptor_errors() {
    let mut a = args("usbmon_busy.txt", ImportFormat::Auto);
    a.vid = Some("dead".into());
    a.pid = Some("beef".into());
    assert!(matches!(import(&a).unwrap_err(), ImportError::Format(_)));
}

#[test]
fn a_half_given_device_filter_is_rejected() {
    let mut a = args("usbmon.txt", ImportFormat::Auto);
    a.vid = Some("138a".into());
    assert!(matches!(import(&a).unwrap_err(), ImportError::Format(_)));
}

#[test]
fn import_round_trips_through_the_cassette_writer() {
    let session = import(&args("usbpcap.pcapng", ImportFormat::Auto)).unwrap();
    let path = std::env::temp_dir().join(format!("fpdev-import-{}.cassette", std::process::id()));
    cassette::save(&session, &path).unwrap();
    let back = cassette::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(session, back);
    assert_eq!(back, expected_session());
}

#[test]
fn default_out_swaps_the_extension() {
    let out = default_out(Path::new("/tmp/capture.pcapng"));
    assert_eq!(out.file_name().unwrap(), "capture.cassette");
}

#[test]
fn a_truncated_usbpcap_header_is_skipped_not_panicked() {
    // A packet shorter than the minimum header decodes to nothing.
    assert!(usbpcap::decode(&[0x00, 0x01, 0x02], Endian::Little).is_none());
}

#[test]
fn a_usbmon_text_line_round_trips_its_fields() {
    let events = usbmon::parse_text("0000000000000011 100 S Bo:2:005:2 -115 1 = 50\n");
    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.endpoint, 0x02);
    assert_eq!(ev.key.bus, 2);
    assert_eq!(ev.key.address, 5);
    assert_eq!(ev.data, vec![0x50]);
}
