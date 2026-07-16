# Re-capturing USB traffic for `fpdev import`

`fpdev import` turns a captured USB trace of a vendor stack (or libfprint) talking to a
fingerprint sensor into a `.cassette` — a replayable [`Session`] of bulk and control transfers —
so a driver author can iterate with no hardware attached.

This chapter is the factual layout of the capture formats the importer reads. The USB
pseudo-header layouts below are an **interoperability fact** — a fixed field arrangement the
capture tools emit — not copyrightable expression. They are reproduced from the format
documentation, not from any capture tool's source.

## What the importer produces

A URB (USB Request Block) is captured twice: a **submit** going down toward the device, and a
**complete** coming back up. The payload lives on whichever half moves the data:

- an OUT write (host to device) carries its bytes on the **submit**;
- an IN read (device to host) carries its bytes on the **complete**.

The importer pairs the two halves by URB id and emits one transfer when its bytes are known,
which is the wire order a replay depends on:

- a control transfer becomes `UsbTransfer::Control` (request type/request/value/index from the
  8-byte setup packet, plus the data stage);
- a bulk or interrupt OUT becomes `UsbTransfer::BulkOut`;
- a bulk or interrupt IN becomes `UsbTransfer::BulkIn`.

Interrupt transfers map onto the bulk variants — the endpoint address (with its `0x80` IN bit)
distinguishes them, and the recorded bytes are what a replay needs. Isochronous transfers are
skipped.

A capture keys devices by **bus number and device address**, not by vendor/product id. `fpdev
import` isolates one device with `--bus`/`--addr` directly, or with `--vid`/`--pid` when the
capture contains that device's `GET_DESCRIPTOR(DEVICE)` response — the importer reads `idVendor`
and `idProduct` out of the descriptor to map the pair back to a bus/address.

## Container detection

The `--format auto` default reads the file's leading bytes:

| Leading magic (big-endian read) | Container |
| --- | --- |
| `0x0A0D0D0A` | pcapng |
| `0xA1B2C3D4`, `0xA1B23C4D`, `0xD4C3B2A1`, `0x4D3CB2A1` | classic pcap |
| printable text whose first line's third token is `S`/`C`/`E` | usbmon text |

A pcapng or classic pcap carries a **link type** per interface that selects the USB
pseudo-header decoder:

| Link type | Code | Decoder |
| --- | --- | --- |
| `DLT_USB_LINUX` | 189 | usbmon binary, 48-byte header |
| `DLT_USB_LINUX_MMAPPED` | 220 | usbmon binary, 64-byte header |
| `DLT_USBPCAP` | 249 | USBPcap |

Binary pseudo-header fields are read in the container's own byte order (the pcapng section
header's endianness, or the classic-pcap magic), because the capturing host wrote both.

## Linux usbmon binary pseudo-header

`DLT_USB_LINUX` is a 48-byte header; `DLT_USB_LINUX_MMAPPED` extends it to 64 bytes. The fields
the importer reads share offsets across both — the extra `DLT_USB_LINUX_MMAPPED` fields sit past
the setup packet.

| Offset | Size | Field | Meaning |
| --- | --- | --- | --- |
| 0 | 8 | `id` | URB id; pairs a submit with its completion |
| 8 | 1 | `type` | `S` submit, `C` complete, `E` error |
| 9 | 1 | `xfer_type` | 0 ISO, 1 interrupt, 2 control, 3 bulk |
| 10 | 1 | `epnum` | endpoint address, `0x80` bit set for IN |
| 11 | 1 | `devnum` | device address |
| 12 | 2 | `busnum` | bus number |
| 14 | 1 | `flag_setup` | `0` when the setup field is present |
| 15 | 1 | `flag_data` | data-presence marker |
| 16 | 8 | `ts_sec` | timestamp seconds |
| 24 | 4 | `ts_usec` | timestamp microseconds |
| 28 | 4 | `status` | URB status |
| 32 | 4 | `length` | requested/actual data length |
| 36 | 4 | `len_cap` | captured data length |
| 40 | 8 | `setup` | 8-byte control setup packet |

Offsets 48–63 (`interval`, `start_frame`, `xfer_flags`, `ndesc`) exist only in the 64-byte
`DLT_USB_LINUX_MMAPPED` header. The captured data (`len_cap` bytes) follows the header.

## Linux usbmon text format

The `/sys/kernel/debug/usb/usbmon/<n>u` log is one URB event per line, whitespace separated:

```
<id> <timestamp> <S|C|E> <TDb:bus:dev:ep> <setup-or-status> <length> [= <hex words>]
```

- `<id>` is a hex URB tag; the same tag ties a submit to its completion.
- The address word is a type letter (`C` control, `Z` isochronous, `I` interrupt, `B` bulk), a
  direction letter (`i` in, `o` out), then `:bus:device:endpoint` in decimal.
- A control submission has the setup packet here: `s` then `bmRequestType bRequest wValue wIndex
  wLength`. `wValue`/`wIndex`/`wLength` are printed as logical 16-bit values. Any other line has
  its URB status here instead.
- `<length>` is the data length in decimal.
- `= <hex words>` is the captured data, grouped into 4-byte words; a `<` or `>` (or nothing) marks
  data that was not captured.

Example — a device descriptor read on endpoint 0:

```
ffff88003d2b8000 100 S Ci:1:005:0 s 80 06 0100 0000 0012 18 <
ffff88003d2b8000 200 C Ci:1:005:0 0 18 = 12010002 00000040 8a131100 00010102 0301
```

## Windows USBPcap pseudo-header

USBPcap prepends a packed, little-endian header; the captured data follows it. `headerLen` is the
offset to that data, which absorbs the extra stage byte a control transfer's header carries.

| Offset | Size | Field | Meaning |
| --- | --- | --- | --- |
| 0 | 2 | `headerLen` | length of this header (27, or 28 for control) |
| 2 | 8 | `irpId` | IRP id; pairs a submit with its completion |
| 10 | 4 | `status` | USBD status |
| 14 | 2 | `function` | URB function |
| 16 | 1 | `info` | bit 0 (`PDO_TO_FDO`) set on a completion |
| 17 | 2 | `bus` | bus number |
| 19 | 2 | `device` | device address |
| 21 | 1 | `endpoint` | endpoint address, `0x80` bit set for IN |
| 22 | 1 | `transfer` | 0 ISO, 1 interrupt, 2 control, 3 bulk |
| 23 | 4 | `dataLength` | captured data length |
| 27 | 1 | `stage` | control transfers only |

A control submission's data opens with the 8-byte setup packet; the remaining bytes are the OUT
data stage. A completion carries only the returned data.

[`Session`]: https://docs.rs/fprint-backend-native
