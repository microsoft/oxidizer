// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Error;

pub(crate) fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    push_u16(bytes, u16::try_from(value.len()).map_err(|_error| Error::MessageTooLarge)?);
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(crate) struct SliceReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> SliceReader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.cursor.checked_add(len).ok_or(Error::InvalidDescriptor)?;
        let result = self.bytes.get(self.cursor..end).ok_or(Error::InvalidDescriptor)?;
        self.cursor = end;
        Ok(result)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub(crate) fn string(&mut self) -> Result<String, Error> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_error| Error::InvalidDescriptor)
    }

    pub(crate) fn finish(self) -> Result<(), Error> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidDescriptor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexadecimal_encoding_uses_lowercase_digits() {
        assert_eq!(hex(&[0x00, 0x1f, 0xa5, 0xff]), "001fa5ff");
    }

    #[test]
    fn reader_rejects_trailing_bytes() {
        let mut reader = SliceReader::new(&[1, 2]);
        assert_eq!(reader.u8().unwrap(), 1);
        assert!(matches!(reader.finish(), Err(Error::InvalidDescriptor)));
    }
}
