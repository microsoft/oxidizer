// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;

use crate::ByteSequence;

impl From<Vec<u8>> for ByteSequence {
    fn from(value: Vec<u8>) -> Self {
        Bytes::from(value).into()
    }
}

#[cfg(test)]
mod tests {
    use bytes::Buf;

    use super::*;

    #[test]
    fn vec_into_sequence() {
        let vec = vec![1, 2, 3, 4, 5];
        let mut sequence: ByteSequence = vec.into();
        assert_eq!(sequence.len(), 5);

        assert_eq!(sequence.get_u8(), 1);
        assert_eq!(sequence.get_u8(), 2);
        assert_eq!(sequence.get_u8(), 3);
        assert_eq!(sequence.get_u8(), 4);
        assert_eq!(sequence.get_u8(), 5);

        assert!(sequence.is_empty());
    }
}
