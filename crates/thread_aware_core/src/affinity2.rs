// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Experimental opaque affinity model (iteration 2).
//!
//! This module explores a redesign of [`Affinity`](crate::Affinity) that keeps the
//! `Affinity` name but removes the frozen four-number shape. Instead of public
//! `processor_index`/`memory_region_index`/count accessors, an affinity is an opaque
//! identity that projects onto per-[`Dimension`] [`LocalityId`] keys.
//!
//! Design goals addressed here:
//!
//! - **Opaque:** no index, count, or integer width appears in the public API, so the
//!   representation can grow without a breaking change.
//! - **Extensible:** [`Dimension`] is `#[non_exhaustive]`; new locality axes (cache
//!   domain, socket, processor group, efficiency class) are additive.
//! - **Unpinned-aware:** a dimension may be absent, expressing "this context makes no
//!   claim along that axis" rather than forcing a fabricated coordinate.
//! - **Map keys:** [`LocalityId`] is `Copy + Eq + Hash`, so per-processor and
//!   per-memory-region storage strategies can key on it directly.
//!
//! Construction goes through [`Affinity::builder`], setting one dimension at a time.
//!
//! # Open question: topology identity
//!
//! Locality ids here compare by value alone, so two independently built affinities
//! that use the same processor locality id are equal. A future iteration may fold in a
//! topology identity so that affinities minted for unrelated registries never compare
//! equal; that is deliberately omitted from this first sketch.

/// A locality dimension of the machine topology.
///
/// New dimensions may be added in a minor release, so consumers matching on this enum
/// must include a wildcard arm.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Dimension {
    /// One logical processor.
    Processor,
    /// One memory region (for example, a NUMA node).
    MemoryRegion,
}

/// Opaque identity of one group within a single [`Dimension`].
///
/// Two affinities that carry equal `LocalityId` values along a dimension belong to the
/// same group along that dimension (for example, two processors in the same memory
/// region share their [`Dimension::MemoryRegion`] locality id). A `LocalityId` is
/// created from a numeric index (`u16`) or a `&'static str` name, and carries no
/// observable index or count once built, so it is suitable as a map key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalityId(LocalityIdRepr);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum LocalityIdRepr {
    Index(u16),
    Name(&'static str),
}

impl From<u16> for LocalityId {
    fn from(index: u16) -> Self {
        Self(LocalityIdRepr::Index(index))
    }
}

impl From<&'static str> for LocalityId {
    fn from(name: &'static str) -> Self {
        Self(LocalityIdRepr::Name(name))
    }
}

/// Identifies the execution context in which a relocation is observed.
///
/// An `Affinity` is an opaque locality identity, not a pin: it does not bind a thread
/// and does not decide what a value is affine to. A dimension may be absent, meaning
/// the runtime that produced the affinity makes no claim along that axis.
///
/// Construct one with [`Affinity::builder`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Affinity {
    processor: Option<LocalityId>,
    memory_region: Option<LocalityId>,
}

impl Affinity {
    /// Starts building an affinity.
    #[must_use]
    pub const fn builder() -> AffinityBuilder {
        AffinityBuilder {
            processor: None,
            memory_region: None,
        }
    }

    /// Returns the opaque [`LocalityId`] of this affinity along `dimension`, or `None`
    /// when the affinity is not constrained along it.
    #[must_use]
    pub fn locality_id(self, dimension: Dimension) -> Option<LocalityId> {
        match dimension {
            Dimension::Processor => self.processor,
            Dimension::MemoryRegion => self.memory_region,
        }
    }

    /// Returns `true` when both affinities share the same locality along `dimension`.
    ///
    /// Returns `false` when either affinity is unconstrained along `dimension`.
    #[must_use]
    pub fn shares_locality(self, other: Self, dimension: Dimension) -> bool {
        match (self.locality_id(dimension), other.locality_id(dimension)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

/// Builds an [`Affinity`] one [`Dimension`] at a time.
///
/// Dimensions left unset are absent from the resulting affinity. Adding a new locality
/// axis in the future is a new [`Dimension`] variant, so callers that ignore it keep
/// working.
#[derive(Debug, Clone)]
pub struct AffinityBuilder {
    processor: Option<LocalityId>,
    memory_region: Option<LocalityId>,
}

impl AffinityBuilder {
    /// Sets the [`LocalityId`] for `dimension`.
    ///
    /// The locality id is created from anything convertible into one, such as a numeric
    /// index (`u16`) or a `&'static str` name.
    #[must_use]
    pub fn dimension(mut self, dimension: Dimension, locality_id: impl Into<LocalityId>) -> Self {
        let locality_id = locality_id.into();

        match dimension {
            Dimension::Processor => self.processor = Some(locality_id),
            Dimension::MemoryRegion => self.memory_region = Some(locality_id),
        }

        self
    }

    /// Finishes building the affinity.
    #[must_use]
    pub fn build(self) -> Affinity {
        Affinity {
            processor: self.processor,
            memory_region: self.memory_region,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Affinity, Dimension, LocalityId};

    #[test]
    fn builder_accepts_numeric_and_named_locality_ids() {
        let affinity = Affinity::builder()
            .dimension(Dimension::Processor, 3u16)
            .dimension(Dimension::MemoryRegion, "numa0")
            .build();

        assert_eq!(affinity.locality_id(Dimension::Processor), Some(LocalityId::from(3u16)));
        assert_eq!(affinity.locality_id(Dimension::MemoryRegion), Some(LocalityId::from("numa0")));
    }

    #[test]
    fn unset_dimension_is_absent() {
        let affinity = Affinity::builder().dimension(Dimension::MemoryRegion, 0u16).build();

        assert!(affinity.locality_id(Dimension::Processor).is_none());
    }

    #[test]
    fn shared_locality_compares_equal() {
        let first = Affinity::builder()
            .dimension(Dimension::Processor, 0u16)
            .dimension(Dimension::MemoryRegion, 2u16)
            .build();
        let second = Affinity::builder()
            .dimension(Dimension::Processor, 1u16)
            .dimension(Dimension::MemoryRegion, 2u16)
            .build();

        assert!(first.shares_locality(second, Dimension::MemoryRegion));
        assert!(!first.shares_locality(second, Dimension::Processor));
    }

    #[test]
    fn named_locality_ids_compare_by_value() {
        let first = Affinity::builder().dimension(Dimension::MemoryRegion, "numa0").build();
        let second = Affinity::builder().dimension(Dimension::MemoryRegion, "numa0").build();
        let third = Affinity::builder().dimension(Dimension::MemoryRegion, "numa1").build();

        assert!(first.shares_locality(second, Dimension::MemoryRegion));
        assert!(!first.shares_locality(third, Dimension::MemoryRegion));
    }

    #[test]
    fn unset_dimension_never_shares() {
        let with_processor = Affinity::builder().dimension(Dimension::Processor, 0u16).build();
        let without_processor = Affinity::builder().dimension(Dimension::MemoryRegion, 0u16).build();

        assert!(!with_processor.shares_locality(without_processor, Dimension::Processor));
    }

    #[test]
    fn numeric_and_named_locality_ids_differ() {
        assert_ne!(LocalityId::from(0u16), LocalityId::from("0"));
    }
}
