// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::error::Error;
use core::fmt;

use allocator_api2::alloc::AllocError as BackingAllocError;

/// Why an [`Arena`](crate::Arena) allocation failed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ErrorKind {
    /// The backing allocator failed to provide memory for a new chunk, or
    /// the arena reached its configured byte budget and cannot grow.
    AllocatorFailed,
    /// The requested allocation needs an alignment larger than the arena can
    /// satisfy. Such a request can never succeed, regardless of how much
    /// memory is available.
    AlignmentTooLarge,
    /// Computing the layout for the requested allocation overflowed the
    /// addressable range (the size arithmetic wrapped `usize` or the total
    /// exceeded `isize::MAX`). Such a request can never succeed.
    CapacityOverflow,
}

/// Error returned by the various fallible allocation methods.
///
/// Allocation fails for one of three reasons, which can be told apart with
/// [`is_allocator_failure`](Self::is_allocator_failure),
/// [`is_alignment_too_large`](Self::is_alignment_too_large), and
/// [`is_capacity_overflow`](Self::is_capacity_overflow):
///
/// * the backing allocator failed to provide memory for a new chunk, or the
///   arena reached its configured byte budget and cannot grow;
/// * the request needs an alignment larger than the arena can satisfy; or
/// * computing the request's layout overflowed the addressable range.
///
/// Like [`core::alloc::AllocError`], this carries no backtrace or source error.
///
/// ```
/// use multitude::{AllocError, Arena};
///
/// let arena = Arena::builder().byte_budget(0).build();
/// let Some(error): Option<AllocError> = arena.try_alloc(1_u8).err() else {
///     panic!("zero budget must reject allocation");
/// };
/// assert!(error.is_allocator_failure());
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct AllocError {
    kind: ErrorKind,
}

impl AllocError {
    /// The backing allocator failed, or the arena's byte budget is exhausted
    /// (see [`is_allocator_failure`]).
    ///
    /// [`is_allocator_failure`]: Self::is_allocator_failure
    pub(crate) const ALLOCATOR_FAILED: Self = Self {
        kind: ErrorKind::AllocatorFailed,
    };

    /// The requested alignment exceeds the arena's maximum (see
    /// [`is_alignment_too_large`]).
    ///
    /// [`is_alignment_too_large`]: Self::is_alignment_too_large
    pub(crate) const ALIGNMENT_TOO_LARGE: Self = Self {
        kind: ErrorKind::AlignmentTooLarge,
    };

    /// The requested layout overflowed the addressable range (see
    /// [`is_capacity_overflow`]).
    ///
    /// [`is_capacity_overflow`]: Self::is_capacity_overflow
    pub(crate) const CAPACITY_OVERFLOW: Self = Self {
        kind: ErrorKind::CapacityOverflow,
    };

    /// Report whether the backing allocator or byte budget prevented growth.
    ///
    /// ```
    /// let arena = multitude::Arena::builder().byte_budget(0).build();
    /// let Some(error) = arena.try_alloc(1_u8).err() else {
    ///     panic!("zero budget must reject allocation");
    /// };
    /// assert!(error.is_allocator_failure());
    /// ```
    #[must_use]
    pub fn is_allocator_failure(self) -> bool {
        matches!(self.kind, ErrorKind::AllocatorFailed)
    }

    /// Report whether the request exceeded the arena's supported alignment.
    ///
    /// Such a request can never
    /// succeed, regardless of how much memory is available.
    ///
    /// ```
    /// #[repr(align(32768))]
    /// struct OverAligned;
    ///
    /// let arena = multitude::Arena::new();
    /// let Some(error) = arena.try_alloc(OverAligned).err() else {
    ///     panic!("over-aligned values must be rejected");
    /// };
    /// assert!(error.is_alignment_too_large());
    /// ```
    #[must_use]
    pub fn is_alignment_too_large(self) -> bool {
        matches!(self.kind, ErrorKind::AlignmentTooLarge)
    }

    /// Report whether request layout computation exceeded the addressable range.
    ///
    /// This includes wrapped `usize` size arithmetic or totals above
    /// `isize::MAX`. Such a request can never
    /// succeed.
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut values = arena.alloc_vec::<u16>();
    /// let Some(error) = values.try_reserve(usize::MAX).err() else {
    ///     panic!("the capacity calculation must overflow");
    /// };
    /// assert!(error.is_capacity_overflow());
    /// ```
    #[must_use]
    pub fn is_capacity_overflow(self) -> bool {
        matches!(self.kind, ErrorKind::CapacityOverflow)
    }
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            ErrorKind::AllocatorFailed => "the backing allocator failed to provide memory",
            ErrorKind::AlignmentTooLarge => "the requested alignment exceeds the arena's supported maximum",
            ErrorKind::CapacityOverflow => "the requested allocation size exceeds the addressable limit",
        })
    }
}

impl Error for AllocError {}

/// Converts a backing allocator failure into an arena allocation failure.
impl From<BackingAllocError> for AllocError {
    #[inline]
    fn from(_: BackingAllocError) -> Self {
        Self::ALLOCATOR_FAILED
    }
}

/// Converts an arena failure to the backing allocator's marker error.
impl From<AllocError> for BackingAllocError {
    #[inline]
    fn from(_: AllocError) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use core::error::Error;

    use allocator_api2::alloc::AllocError as BackingAllocError;

    use super::AllocError;

    #[test]
    fn predicates_are_mutually_exclusive() {
        let failed = AllocError::ALLOCATOR_FAILED;
        assert!(failed.is_allocator_failure());
        assert!(!failed.is_alignment_too_large());
        assert!(!failed.is_capacity_overflow());

        let align = AllocError::ALIGNMENT_TOO_LARGE;
        assert!(align.is_alignment_too_large());
        assert!(!align.is_allocator_failure());
        assert!(!align.is_capacity_overflow());

        let overflow = AllocError::CAPACITY_OVERFLOW;
        assert!(overflow.is_capacity_overflow());
        assert!(!overflow.is_allocator_failure());
        assert!(!overflow.is_alignment_too_large());
    }

    #[test]
    fn display_and_debug_render_each_kind() {
        assert_eq!(
            AllocError::ALLOCATOR_FAILED.to_string(),
            "the backing allocator failed to provide memory"
        );
        assert_eq!(
            AllocError::ALIGNMENT_TOO_LARGE.to_string(),
            "the requested alignment exceeds the arena's supported maximum"
        );
        assert_eq!(
            AllocError::CAPACITY_OVERFLOW.to_string(),
            "the requested allocation size exceeds the addressable limit"
        );
        assert_eq!(
            format!("{:?}", AllocError::ALLOCATOR_FAILED),
            "AllocError { kind: AllocatorFailed }"
        );
    }

    #[test]
    fn usable_as_error_trait_object() {
        let err = AllocError::CAPACITY_OVERFLOW;
        let as_err: &dyn Error = &err;
        assert_eq!(as_err.to_string(), "the requested allocation size exceeds the addressable limit");
        assert!(as_err.source().is_none());
    }

    #[test]
    fn equality_and_copy() {
        let err = AllocError::ALIGNMENT_TOO_LARGE;
        let copied = err;
        assert_eq!(err, copied);
        assert_ne!(AllocError::ALLOCATOR_FAILED, AllocError::CAPACITY_OVERFLOW);
    }

    #[test]
    fn bridges_to_and_from_allocator_api2() {
        let bridged: AllocError = BackingAllocError.into();
        assert!(bridged.is_allocator_failure());

        for kind in [
            AllocError::ALLOCATOR_FAILED,
            AllocError::ALIGNMENT_TOO_LARGE,
            AllocError::CAPACITY_OVERFLOW,
        ] {
            let _: BackingAllocError = kind.into();
        }
    }
}
