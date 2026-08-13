// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public wire container API.

use rallocator_wire::format::{Header, Section, Version};
use rallocator_wire::io::{Reader, Writer};

fn assert_error(error: rallocator_wire::Error, message: &str) {
    assert_eq!(error.to_string(), message);
    assert_eq!(format!("{error:?}"), format!("Error({message})"));
}

#[test]
fn header_and_primitives_round_trip_little_endian() {
    let mut bytes = [0_u8; Header::encoded_len() + Section::encoded_len(15)];
    let mut writer = Writer::new(&mut bytes);
    writer.write_header(Header::new(7, Version::new(1, 2, 3))).unwrap();
    writer.begin_section(9, 2, 15).unwrap();
    writer.write_u8(0xAA).unwrap();
    writer.write_u16(0x1234).unwrap();
    writer.write_u32(0x1234_5678).unwrap();
    writer.write_u64(0x0123_4567_89AB_CDEF).unwrap();
    writer.finish().unwrap();

    assert_eq!(
        &bytes[Header::encoded_len()..Header::encoded_len() + Section::encoded_len(0)],
        &[9, 0, 2, 0, 15, 0, 0, 0]
    );
    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.read_header().unwrap(), Header::new(7, Version::new(1, 2, 3)));
    let section = reader.read_section().unwrap().unwrap();
    let mut payload = Reader::new(section.payload());
    assert_eq!(payload.read_u8().unwrap(), 0xAA);
    assert_eq!(payload.read_u16().unwrap(), 0x1234);
    assert_eq!(payload.read_u32().unwrap(), 0x1234_5678);
    assert_eq!(payload.read_u64().unwrap(), 0x0123_4567_89AB_CDEF);
    assert!(reader.read_section().unwrap().is_none());
}

#[test]
fn malformed_inputs_are_rejected() {
    Reader::new(&[0; 7]).read_header().unwrap_err();
    let mut bad_magic = [0_u8; Header::encoded_len()];
    bad_magic[..8].copy_from_slice(b"NOTSNAP\0");
    Reader::new(&bad_magic).read_header().unwrap_err();

    let mut truncated_section = [0_u8; Header::encoded_len() + Section::encoded_len(0)];
    let mut writer = Writer::new(&mut truncated_section);
    writer.write_header(Header::new(1, Version::new(0, 1, 0))).unwrap();
    assert!(writer.begin_section(1, 1, 4).is_err());
    let header_len = Header::encoded_len();
    truncated_section[header_len..header_len + 2].copy_from_slice(&1_u16.to_le_bytes());
    truncated_section[header_len + 2..header_len + 4].copy_from_slice(&1_u16.to_le_bytes());
    truncated_section[header_len + 4..header_len + 8].copy_from_slice(&4_u32.to_le_bytes());
    let mut reader = Reader::new(&truncated_section);
    reader.read_header().unwrap();
    reader.read_section().unwrap_err();

    let mut unsupported = [0_u8; Header::encoded_len()];
    unsupported[..8].copy_from_slice(b"RALSNAP\0");
    unsupported[8..10].copy_from_slice(&2_u16.to_le_bytes());
    Reader::new(&unsupported).read_header().unwrap_err();

    let mut reserved = [0_u8; Header::encoded_len()];
    let mut writer = Writer::new(&mut reserved);
    writer.write_header(Header::new(1, Version::new(0, 1, 0))).unwrap();
    writer.finish().unwrap();
    reserved[18..20].copy_from_slice(&1_u16.to_le_bytes());
    Reader::new(&reserved).read_header().unwrap_err();

    let mut short_section = [0_u8; Header::encoded_len() + Section::encoded_len(0) - 1];
    let mut writer = Writer::new(&mut short_section[..Header::encoded_len()]);
    writer.write_header(Header::new(1, Version::new(0, 1, 0))).unwrap();
    assert_eq!(writer.position(), Header::encoded_len());
    assert_eq!(writer.finish(), Ok(Header::encoded_len()));
    let mut reader = Reader::new(&short_section);
    reader.read_header().unwrap();
    reader.read_section().unwrap_err();
}

#[test]
fn writer_reports_unused_output_capacity() {
    let mut bytes = [0_u8; 2];
    let mut writer = Writer::new(&mut bytes);
    writer.write_u8(1).unwrap();
    assert_eq!(writer.position(), 1);
    writer.finish().unwrap_err();
}

#[test]
fn writer_enforces_declared_section_lengths() {
    let mut bytes = [0_u8; Section::encoded_len(2)];
    let mut writer = Writer::new(&mut bytes);
    writer.begin_section(1, 1, 2).unwrap();
    writer.write_u8(1).unwrap();
    writer.finish().unwrap_err();

    let mut bytes = [0_u8; Section::encoded_len(1)];
    let mut writer = Writer::new(&mut bytes);
    writer.begin_section(1, 1, 1).unwrap();
    assert!(writer.write_u16(1).is_err());

    let mut bytes = [0_u8; 2 * Section::encoded_len(1)];
    let mut writer = Writer::new(&mut bytes);
    writer.begin_section(1, 1, 1).unwrap();
    assert!(writer.begin_section(2, 1, 1).is_err());
}

#[test]
fn errors_have_descriptive_display_and_debug_output() {
    assert_error(Reader::new(&[]).read_u8().unwrap_err(), "the input or output ended unexpectedly");

    let invalid_magic = [0_u8; Header::encoded_len()];
    assert_error(
        Reader::new(&invalid_magic).read_header().unwrap_err(),
        "the container magic bytes are invalid",
    );

    let mut unsupported = [0_u8; Header::encoded_len()];
    Writer::new(&mut unsupported)
        .write_header(Header::new(1, Version::new(0, 1, 0)))
        .unwrap();
    unsupported[8..10].copy_from_slice(&2_u16.to_le_bytes());
    assert_error(
        Reader::new(&unsupported).read_header().unwrap_err(),
        "wire-format version 2 is unsupported",
    );

    #[cfg(target_pointer_width = "64")]
    {
        assert_error(
            Writer::new(&mut []).begin_section(1, 1, usize::MAX).unwrap_err(),
            "a length cannot be represented",
        );
    }

    assert_error(
        Writer::new(&mut [0_u8]).finish().unwrap_err(),
        "the output contains unused trailing bytes",
    );

    let mut reserved = [0_u8; Header::encoded_len()];
    Writer::new(&mut reserved)
        .write_header(Header::new(1, Version::new(0, 1, 0)))
        .unwrap();
    reserved[18..20].copy_from_slice(&1_u16.to_le_bytes());
    assert_error(
        Reader::new(&reserved).read_header().unwrap_err(),
        "a reserved field contains a nonzero value",
    );

    let mut section = [0_u8; Section::encoded_len(1)];
    let mut writer = Writer::new(&mut section);
    writer.begin_section(1, 1, 1).unwrap();
    assert_error(writer.finish().unwrap_err(), "a section payload differs from its declared length");
}
