// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;

use crate::Sequence;

impl From<&'static [u8]> for Sequence {
    fn from(value: &'static [u8]) -> Self {
        let bytes = Bytes::from_static(value);
        bytes.into()
    }
}

impl<const LEN: usize> From<&'static [u8; LEN]> for Sequence {
    fn from(value: &'static [u8; LEN]) -> Self {
        value.as_slice().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_slice() {
        let data: &'static [u8] = b"hello";

        let seq = Sequence::from(data);

        assert_eq!(seq, data);
    }

    #[test]
    fn from_array() {
        let data = b"world";

        let seq = Sequence::from(data);

        assert_eq!(seq, data);
    }
}
