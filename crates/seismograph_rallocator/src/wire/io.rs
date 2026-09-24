// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free readers and writers for wire bytes.

use super::Error;
use super::format::{self, Header, Section};
use crate::snapshot::Version;

/// Allocation-free writer for an exactly sized output buffer.
///
/// Each section must consume exactly the payload length declared by
/// [`Writer::begin_section`], and [`Writer::finish`] succeeds only after the
/// entire output slice has been consumed.
#[derive(Debug)]
pub(crate) struct Writer<'a> {
    bytes: &'a mut [u8],
    position: usize,
    section_end: Option<usize>,
}

impl<'a> Writer<'a> {
    /// Creates a writer for `bytes`.
    pub(crate) fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            section_end: None,
        }
    }

    /// Returns the number of bytes written.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn position(&self) -> usize {
        self.position
    }

    /// Completes the current section and output buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the current section is incomplete or if any bytes
    /// remain unused in the output slice.
    pub(crate) fn finish(self) -> Result<usize, Error> {
        if self.section_end.is_some_and(|end| end != self.position) {
            Err(Error::SECTION_LENGTH_MISMATCH)
        } else if self.position == self.bytes.len() {
            Ok(self.position)
        } else {
            Err(Error::TRAILING_BYTES)
        }
    }

    /// Writes a container header.
    pub(crate) fn write_header(&mut self, header: Header) -> Result<(), Error> {
        self.write_bytes(&format::magic())?;
        self.write_u16(header.wire_format())?;
        self.write_u16(header.telemetry_schema())?;
        let producer = header.producer();
        self.write_u16(producer.major)?;
        self.write_u16(producer.minor)?;
        self.write_u16(producer.patch)?;
        self.write_u16(0)
    }

    /// Starts a section with a declared payload length.
    ///
    /// # Errors
    ///
    /// Returns an error if the previous section was not filled exactly, the
    /// payload length cannot be encoded, or the section exceeds the output
    /// buffer. Subsequent writes may not cross the declared section boundary.
    pub(crate) fn begin_section(&mut self, section_id: u16, section_version: u16, payload_len: usize) -> Result<(), Error> {
        self.finish_section()?;
        let payload_len = u32::try_from(payload_len).map_err(|_error| Error::LENGTH_OVERFLOW)?;
        self.write_u16(section_id)?;
        self.write_u16(section_version)?;
        self.write_u32(payload_len)?;
        let section_end = self.position.checked_add(payload_len as usize).ok_or(Error::LENGTH_OVERFLOW)?;
        if section_end > self.bytes.len() {
            return Err(Error::UNEXPECTED_END);
        }
        self.section_end = Some(section_end);
        Ok(())
    }

    /// Writes an unsigned 8-bit integer.
    pub(crate) fn write_u8(&mut self, value: u8) -> Result<(), Error> {
        self.write_bytes(&[value])
    }
    /// Writes a little-endian unsigned 16-bit integer.
    pub(crate) fn write_u16(&mut self, value: u16) -> Result<(), Error> {
        self.write_bytes(&value.to_le_bytes())
    }
    /// Writes a little-endian unsigned 32-bit integer.
    pub(crate) fn write_u32(&mut self, value: u32) -> Result<(), Error> {
        self.write_bytes(&value.to_le_bytes())
    }
    /// Writes a little-endian unsigned 64-bit integer.
    pub(crate) fn write_u64(&mut self, value: u64) -> Result<(), Error> {
        self.write_bytes(&value.to_le_bytes())
    }
    /// Writes raw bytes.
    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self.position.checked_add(bytes.len()).ok_or(Error::LENGTH_OVERFLOW)?;
        if self.section_end.is_some_and(|section_end| end > section_end) {
            return Err(Error::SECTION_LENGTH_MISMATCH);
        }
        let destination = self.bytes.get_mut(self.position..end).ok_or(Error::UNEXPECTED_END)?;
        destination.copy_from_slice(bytes);
        self.position = end;
        Ok(())
    }

    fn finish_section(&mut self) -> Result<(), Error> {
        if self.section_end.take().is_some_and(|end| end != self.position) {
            return Err(Error::SECTION_LENGTH_MISMATCH);
        }
        Ok(())
    }
}

/// Allocation-free reader for wire bytes.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// Creates a reader for `bytes`.
    #[must_use]
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// Returns the unread byte count.
    #[must_use]
    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    /// Reads and validates a container header.
    pub(crate) fn read_header(&mut self) -> Result<Header, Error> {
        if self.read_bytes(format::magic().len())? != format::magic() {
            return Err(Error::INVALID_MAGIC);
        }
        let wire_format = self.read_u16()?;
        if wire_format != format::wire_format_version() {
            return Err(Error::unsupported_wire_format(wire_format));
        }
        let telemetry_schema = self.read_u16()?;
        let producer = Version::new(self.read_u16()?, self.read_u16()?, self.read_u16()?);
        if self.read_u16()? != 0 {
            return Err(Error::INVALID_RESERVED);
        }
        Ok(Header::from_wire(wire_format, telemetry_schema, producer))
    }

    /// Reads the next section, if one remains.
    pub(crate) fn read_section(&mut self) -> Result<Option<Section<'a>>, Error> {
        if self.remaining() == 0 {
            return Ok(None);
        }
        if self.remaining() < Section::encoded_len(0) {
            return Err(Error::UNEXPECTED_END);
        }
        let id = self.read_u16()?;
        let version = self.read_u16()?;
        let payload_len = self.read_u32()? as usize;
        let payload = self.read_bytes(payload_len)?;
        Ok(Some(Section::new(id, version, payload)))
    }

    /// Reads an unsigned 8-bit integer.
    pub(crate) fn read_u8(&mut self) -> Result<u8, Error> {
        Ok(self.read_bytes(1)?[0])
    }
    /// Reads a little-endian unsigned 16-bit integer.
    pub(crate) fn read_u16(&mut self) -> Result<u16, Error> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    /// Reads a little-endian unsigned 32-bit integer.
    pub(crate) fn read_u32(&mut self) -> Result<u32, Error> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    /// Reads a little-endian unsigned 64-bit integer.
    pub(crate) fn read_u64(&mut self) -> Result<u64, Error> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
    /// Reads `len` raw bytes.
    pub(crate) fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(len).ok_or(Error::LENGTH_OVERFLOW)?;
        let bytes = self.bytes.get(self.position..end).ok_or(Error::UNEXPECTED_END)?;
        self.position = end;
        Ok(bytes)
    }
}
