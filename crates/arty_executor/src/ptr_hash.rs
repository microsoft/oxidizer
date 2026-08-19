// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::{BuildHasherDefault, Hasher};

/// We want to minimize the cost of hashing data that is already highly unique. In this sense,
/// using pointers as hashes is appealing.
///
/// However, pointers tend to look very similar near the start of the value, with only the end
/// differing.
///
/// ```text
/// 0x0000000012341234
/// 0x0000000056785678
/// 0x0001C00B59595959
/// 0x0001C00B59636363
/// ```
///
/// This can lead to unbalanced hash tables, as the first few bits are the same for all pointers.
/// We do not necessarily know which bits are the most significant, so we xor the pointer with its
/// own bit-reversed value to help spread around the entropy. This results in a hash-quality value.
#[derive(Debug, Default)]
pub(crate) struct PointerHasher {
    value: u64,
}

impl Hasher for PointerHasher {
    #[cfg_attr(test, mutants::skip)] // Difficult to test without getting silly and hardcoding results.
    fn finish(&self) -> u64 {
        self.value
    }

    #[cfg_attr(test, mutants::skip)] // Difficult to test without getting silly and hardcoding results.
    fn write(&mut self, bytes: &[u8]) {
        let input_raw = if let Ok(bytes) = <[u8; 8]>::try_from(bytes) {
            u64::from_ne_bytes(bytes)
        } else {
            u64::from(u32::from_ne_bytes(
                bytes
                    .try_into()
                    .expect("PointerHasher input must represent a 32-bit or 64-bit pointer"),
            ))
        };

        self.value ^= input_raw;
        self.value ^= input_raw.reverse_bits();
    }
}

/// Builder for [`PointerHasher`].
pub(crate) type BuildPointerHasher = BuildHasherDefault<PointerHasher>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_same() {
        let mut hasher1 = PointerHasher::default();
        let mut hasher2 = PointerHasher::default();

        hasher1.write(&1u64.to_ne_bytes());
        hasher2.write(&1u64.to_ne_bytes());

        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn accepts_32_bit_input() {
        let mut hasher = PointerHasher::default();

        hasher.write(&1u32.to_ne_bytes());

        assert_ne!(hasher.finish(), 0);
    }

    #[test]
    #[should_panic(expected = "PointerHasher input must represent a 32-bit or 64-bit pointer")]
    fn rejects_other_input_widths() {
        PointerHasher::default().write(&[1]);
    }
}
