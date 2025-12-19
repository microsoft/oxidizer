// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to implement the `HasMemory` trait
//! using the `GlobalPool` implementation strategy.

use bytesbuf::{BytesView, GlobalPool, HasMemory, MemoryShared};

fn main() {
    // The global memory pool in real-world code would be provided by the application framework.
    let global_memory_pool = GlobalPool::new();

    let mut checksum_calculator = ChecksumCalculator::new(global_memory_pool);

    // When we obtain an instance of a type that implements `HasMemory`,
    // we should extract the memory provider if we need to access it on demand.
    let memory = checksum_calculator.memory();

    // These messages are meant to be passed to the checksum calculator,
    // so they use the memory provider we obtained from the checksum calculator.
    let message1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
    let message2 = BytesView::copied_from_slice(b"Goodbye, world!", &memory);
    let message3 = BytesView::copied_from_slice(b"Goodbye, universe!", &memory);

    checksum_calculator.add_bytes(message1);
    checksum_calculator.add_bytes(message2);
    checksum_calculator.add_bytes(message3);

    println!("Checksum: {}", checksum_calculator.checksum());
}

/// Calculates a checksum for a given message.
///
/// # Implementation strategy for `HasMemory`
///
/// This type does not benefit from any specific memory configuration - it consumes bytes no
/// matter what sort of memory they are in. It also does not pass the bytes to some other type.
///
/// Therefore, we simply use `GlobalPool` as the memory provider we publish, as this is
/// the default choice when there is no specific provider to prefer.
#[derive(Debug)]
struct ChecksumCalculator {
    // The application logic must provide this - it is our dependency.
    memory: GlobalPool,

    checksum: u64,
}

impl ChecksumCalculator {
    pub const fn new(memory: GlobalPool) -> Self {
        Self { memory, checksum: 0 }
    }

    pub fn add_bytes(&mut self, mut bytes: BytesView) {
        while !bytes.is_empty() {
            let b = bytes.get_byte();
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
        self.memory.clone()
    }
}
