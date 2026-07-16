// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Container readers: pull the per-packet link-layer payloads out of a pcapng or a classic pcap
//! file, tagged with the link type that says which USB pseudo-header decoder to run.
//!
//! The `pcap-file` crate handles the block/record framing and the file endianness; this layer keeps
//! only what `fpdev import` needs — the link type and the raw bytes — and hands the byte order on so
//! the pseudo-header decoders read multi-byte fields the way the capturing host wrote them.

use pcap_file::pcap::PcapReader;
use pcap_file::pcapng::{Block, PcapNgReader};
use pcap_file::Endianness;

use crate::capture::event::Endian;
use crate::capture::ImportError;

/// One captured packet: its link type as a raw code, and the link-layer bytes (a USB pseudo-header
/// and its data).
pub(crate) struct Packet {
    pub(crate) linktype: u32,
    pub(crate) data: Vec<u8>,
}

fn endian_of(endianness: Endianness) -> Endian {
    if endianness.is_little() {
        Endian::Little
    } else {
        Endian::Big
    }
}

/// Read a pcapng capture into its packets and the section's byte order.
///
/// Each Enhanced/Simple/legacy Packet block is paired with the link type of the interface it names,
/// so a file mixing link types (or one whose interface is not the first block) still decodes.
pub(crate) fn read_pcapng(bytes: &[u8]) -> Result<(Endian, Vec<Packet>), ImportError> {
    let mut reader = PcapNgReader::new(bytes)?;
    let endian = endian_of(reader.section().endianness);
    let mut interfaces: Vec<u32> = Vec::new();
    let mut packets = Vec::new();

    while let Some(block) = reader.next_block() {
        match block? {
            Block::InterfaceDescription(idb) => interfaces.push(idb.linktype.into()),
            Block::EnhancedPacket(epb) => {
                let linktype = interfaces.get(epb.interface_id as usize).copied().ok_or(
                    ImportError::Format(
                        "pcapng packet names an interface with no description block".into(),
                    ),
                )?;
                packets.push(Packet {
                    linktype,
                    data: epb.data.into_owned(),
                });
            }
            Block::SimplePacket(spb) => {
                let linktype = interfaces.first().copied().ok_or(ImportError::Format(
                    "pcapng simple packet with no interface description block".into(),
                ))?;
                packets.push(Packet {
                    linktype,
                    data: spb.data.into_owned(),
                });
            }
            Block::Packet(pb) => {
                let linktype = interfaces.get(pb.interface_id as usize).copied().ok_or(
                    ImportError::Format(
                        "pcapng packet names an interface with no description block".into(),
                    ),
                )?;
                packets.push(Packet {
                    linktype,
                    data: pb.data.into_owned(),
                });
            }
            _ => {}
        }
    }

    Ok((endian, packets))
}

/// Read a classic pcap capture into its packets and the file's byte order. A classic pcap has one
/// link type in its global header, shared by every packet.
pub(crate) fn read_pcap(bytes: &[u8]) -> Result<(Endian, Vec<Packet>), ImportError> {
    let mut reader = PcapReader::new(bytes)?;
    let header = reader.header();
    let endian = endian_of(header.endianness);
    let linktype: u32 = header.datalink.into();

    let mut packets = Vec::new();
    while let Some(packet) = reader.next_packet() {
        let packet = packet?;
        packets.push(Packet {
            linktype,
            data: packet.data.into_owned(),
        });
    }

    Ok((endian, packets))
}
