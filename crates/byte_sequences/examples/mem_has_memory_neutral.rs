// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to implement the `HasMemory` trait
//! using the `NeutralMemoryPool` implementation strategy.

use byte_sequences::{HasMemory, MemoryShared, NeutralMemoryPool, Sequence};
use bytes::Buf;

fn main() {
    // The neutral memory pool in a real application would be provided by the framework.
    let neutral_memory_pool = NeutralMemoryPool::new();

    let mut checksum_calculator = ChecksumCalculator::new(neutral_memory_pool);

    // When we obtain an instance of a type that implements `HasMemory`,
    // we should extract the memory provider so we can reuse it across calls to the instance.
    let memory = checksum_calculator.memory();

    // These byte sequences are meant to be passed to the checksum calculator,
    // so they use the memory provider we obtained from the checksum calculator.
    let data1 = Sequence::copy_from_slice(b"Hello, world!", &memory);
    let data2 = Sequence::copy_from_slice(b"Goodbye, world!", &memory);
    let data3 = Sequence::copy_from_slice(b"Goodbye, universe!", &memory);

    checksum_calculator.add_bytes(data1);
    checksum_calculator.add_bytes(data2);
    checksum_calculator.add_bytes(data3);

    println!("Checksum: {}", checksum_calculator.checksum());
}

/// Calculates a checksum for a given byte sequence.
///
/// # Implementation strategy for `HasMemory`
///
/// This type does not benefit from any specific memory configuration - it consumes bytes no
/// matter what sort of memory they are in. It also does not pass the bytes to some other type.
///
/// Therefore, we simply use `NeutralMemoryPool` as the memory provider we publish, as this is
/// the default choice when there is no specific provider to prefer.
#[derive(Debug)]
struct ChecksumCalculator {
    // The application logic must provide this - it is our dependency.
    memory_provider: NeutralMemoryPool,

    checksum: u64,
}

impl ChecksumCalculator {
    pub const fn new(memory_provider: NeutralMemoryPool) -> Self {
        Self {
            memory_provider,
            checksum: 0,
        }
    }

    pub fn add_bytes(&mut self, mut bytes: Sequence) {
        while !bytes.is_empty() {
            let b = bytes.get_u8();
            self.checksum = self.checksum.wrapping_add(u64::from(b));
        }
    }

    pub const fn checksum(&self) -> u64 {
        self.checksum
    }
}

impl HasMemory for ChecksumCalculator {
    fn memory(&self) -> impl MemoryShared {
        // Cloning a memory provider is intended to be a cheap operation, reusing resources.
        self.memory_provider.clone()
    }
}
