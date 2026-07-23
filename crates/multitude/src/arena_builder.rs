// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::internal::constants::{MAX_NORMAL_ALLOC, MIN_CHUNK_BYTES, SizeClass};
use crate::{AllocError, Arena};

/// Minimum value accepted for the `max_normal_alloc` knob.
const MIN_MAX_NORMAL_ALLOC: usize = 4096;

/// Fluent builder for [`Arena`].
///
/// All knobs have sensible defaults. The defaults reproduce
/// `Arena::new()` exactly.
///
/// # Example
///
/// ```
/// let builder: multitude::ArenaBuilder = multitude::Arena::builder();
/// assert!(format!("{builder:?}").contains("ArenaBuilder"));
/// ```
pub struct ArenaBuilder<A: Allocator + Clone = Global> {
    allocator: A,
    max_normal_alloc: usize,
    byte_budget: Option<usize>,
    capacity: usize,
    _phantom: PhantomData<A>,
}

impl ArenaBuilder<Global> {
    /// Start a new builder with default knobs and the [`Global`] allocator.
    ///
    /// Crate-internal: the public entry point is
    /// [`Arena::builder`](crate::Arena::builder), per the builder convention
    /// that a builder is obtained from its target type, not constructed
    /// directly.
    #[must_use]
    #[inline]
    pub(crate) fn new() -> Self {
        Self::new_in(Global)
    }
}

impl<A: Allocator + Clone> ArenaBuilder<A> {
    /// Start a default builder with a custom backing allocator.
    ///
    /// Crate-internal: the public entry point is
    /// [`Arena::builder_in`](crate::Arena::builder_in).
    #[must_use]
    #[inline]
    pub(crate) fn new_in(allocator: A) -> Self {
        Self {
            allocator,
            max_normal_alloc: MAX_NORMAL_ALLOC,
            byte_budget: None,
            capacity: 0,
            _phantom: PhantomData,
        }
    }

    /// Set the oversized-allocation cutover threshold.
    ///
    /// Requests that need a fresh chunk and exceed this size get their own
    /// oversized chunk. Must be in `[4096, MAX_CHUNK_BYTES - chunk_header_size]`;
    /// out-of-range values cause [`Self::build`] / [`Self::try_build`] to panic
    /// with the resolved bounds in the panic message.
    ///
    /// # Example
    ///
    /// ```
    /// let builder = multitude::Arena::builder().max_normal_alloc(8 * 1024);
    /// assert!(format!("{builder:?}").contains("8192"));
    /// ```
    #[must_use]
    #[inline]
    pub const fn max_normal_alloc(mut self, bytes: usize) -> Self {
        self.max_normal_alloc = bytes;
        self
    }

    /// Set a cap on outstanding chunk-capacity bytes.
    ///
    /// Limits the total bytes of chunk capacity (live + cached) that may be
    /// outstanding at any one time.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::builder().byte_budget(64 * 1024).build();
    /// assert!(arena.try_alloc(1_u8).is_ok());
    /// ```
    #[must_use]
    #[inline]
    pub const fn byte_budget(mut self, bytes: usize) -> Self {
        self.byte_budget = Some(bytes);
        self
    }

    /// Preallocate `bytes` bytes of total chunk allocation up front
    /// (header + payload), warming the arena's chunk cache. `bytes` must be
    /// `0` or at least 512. One capacity knob covers references and smart
    /// pointers alike.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::builder().with_capacity(4096).build();
    /// assert_eq!(*arena.alloc(7), 7);
    /// ```
    #[must_use]
    #[inline]
    pub const fn with_capacity(mut self, bytes: usize) -> Self {
        self.capacity = bytes;
        self
    }

    /// Replace the backing allocator. Returns a builder over the new
    /// allocator type with all other settings preserved.
    ///
    /// # Example
    ///
    /// ```
    /// use allocator_api2::alloc::Global;
    /// let arena = multitude::Arena::builder().allocator_in(Global).build();
    /// let _: &Global = arena.allocator();
    /// ```
    #[must_use]
    #[inline]
    pub fn allocator_in<A2: Allocator + Clone>(self, allocator: A2) -> ArenaBuilder<A2> {
        ArenaBuilder {
            allocator,
            max_normal_alloc: self.max_normal_alloc,
            byte_budget: self.byte_budget,
            capacity: self.capacity,
            _phantom: PhantomData,
        }
    }

    /// Validate this builder's configuration. Panics if any knob is
    /// out of range.
    #[cold]
    fn validate(&self) {
        let upper = crate::internal::chunk::max_bump_extent::<A>();
        assert!(
            (MIN_MAX_NORMAL_ALLOC..=upper).contains(&self.max_normal_alloc),
            "max_normal_alloc must be in [{MIN_MAX_NORMAL_ALLOC}, {upper}], got {}",
            self.max_normal_alloc,
        );
        assert!(
            self.capacity == 0 || self.capacity >= MIN_CHUNK_BYTES,
            "with_capacity(bytes) must be either 0 or at least {MIN_CHUNK_BYTES}, got {}",
            self.capacity,
        );
    }

    /// Resolve a desired preallocation `capacity` (total chunk-allocation
    /// bytes) into a `(target_class, chunk_count)` pair.
    #[cfg_attr(test, mutants::skip)] // belt-and-suspenders cap; inner helper already saturates
    fn resolve_capacity(capacity: usize) -> Option<(SizeClass, usize)> {
        if capacity == 0 {
            return None;
        }
        let target_class = SizeClass::min_for_bytes(capacity).min(SizeClass::MAX);
        let class_total = target_class.bytes();
        let count = capacity.div_ceil(class_total);
        Some((target_class, count))
    }

    /// Consume this builder and produce a configured [`Arena`].
    ///
    /// # Panics
    ///
    /// Panics if any builder knob is out of range, or if the backing
    /// allocator fails while preallocating chunks.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::builder().with_capacity(4096).build();
    /// assert_eq!(*arena.alloc(1), 1);
    /// ```
    #[must_use]
    #[cold]
    pub fn build(self) -> Arena<A>
    where
        A: 'static,
    {
        match self.try_build() {
            Ok(a) => a,
            Err(_) => panic_build(),
        }
    }

    /// Fallible variant of [`Self::build`].
    ///
    /// # Panics
    ///
    /// Panics if any builder knob is out of range. Allocator failures during
    /// preallocation are returned as [`AllocError`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails while
    /// preallocating chunks.
    ///
    /// # Example
    ///
    /// ```
    /// let Ok(arena) = multitude::Arena::builder().try_build() else {
    ///     panic!("arena construction failed");
    /// };
    /// assert_eq!(*arena.alloc(2), 2);
    /// ```
    #[cold]
    pub fn try_build(self) -> Result<Arena<A>, AllocError>
    where
        A: 'static,
    {
        self.validate();
        let capacity = Self::resolve_capacity(self.capacity);
        let arena = Arena::try_from_config(self.allocator, self.max_normal_alloc, self.byte_budget)?;
        if let Some((class, n)) = capacity {
            for _ in 0..n {
                arena.preallocate_one(class)?;
            }
        }
        Ok(arena)
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "Allocator and PhantomData fields are not useful in debug output"
)]
impl<A: Allocator + Clone> fmt::Debug for ArenaBuilder<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArenaBuilder")
            .field("max_normal_alloc", &self.max_normal_alloc)
            .field("byte_budget", &self.byte_budget)
            .field("capacity", &self.capacity)
            .finish()
    }
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "panicking constructor matches Arena's `panic_alloc` style")]
fn panic_build() -> ! {
    panic!("multitude::ArenaBuilder::build: backing allocator failed");
}
