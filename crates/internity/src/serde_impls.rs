// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `serde` support (behind the `serde` feature) for the interners.
//!
//! * A [`Sym`] deliberately has no plain [`serde::Serialize`]/`Deserialize`: a
//!   bare handle is a meaningless integer outside its interner, and a raw handle
//!   deserialized against a different interner would silently resolve to an
//!   unrelated string. Serialize handles through the reader-aware
//!   [`SerializeIn`](crate::se::SerializeIn) /
//!   [`DeserializeIn`](crate::de::DeserializeIn) derives instead, which carry the
//!   interner and round-trip each handle as its string.
//! * Any [`LocalLexicon`] can be serialized as a sequence of its strings.
//!   Deserialization constructs a default-hasher [`LocalLexicon`] and reproduces
//!   identical [`Sym`] handles because local handles follow insertion order.
//! * A [`ThreadedLexicon`](crate::ThreadedLexicon) can be *deserialized* (it is
//!   built fresh, single-threaded) but intentionally does not implement direct
//!   [`serde::Serialize`]. A shared, live interner *can* be snapshotted — via
//!   [`freeze`](crate::ThreadedLexicon::freeze) — so direct serialization is
//!   withheld deliberately to make that point-in-time snapshot boundary explicit,
//!   not because it is impossible. Freeze it and serialize the resulting
//!   [`Reader`](crate::Reader) with
//!   [`SerializeReader`](crate::se::SerializeReader) instead. Re-interning the
//!   serialized sequence reproduces identical [`Sym`] handles for the
//!   default-hasher [`ThreadedLexicon`](crate::ThreadedLexicon); a custom hasher
//!   could assign strings to different shards, so it cannot provide that
//!   guarantee.

use core::fmt;
use core::hash::BuildHasher;
use core::marker::PhantomData;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::de::{DeserializeInSeed, cautious_capacity};
use crate::sym::Sym;
use crate::{Lexicon, LocalLexicon};

impl<S: BuildHasher> Serialize for LocalLexicon<S> {
    fn serialize<Se: Serializer>(&self, serializer: Se) -> Result<Se::Ok, Se::Error> {
        serializer.collect_seq(self.iter().map(|(_, s)| s))
    }
}

/// Visits a sequence of strings and interns them into a fresh interner.
struct StrSeqVisitor<T>(PhantomData<T>);

trait DeserializeLexicon: Lexicon {
    fn with_entry_capacity(entries: usize) -> Self;
}

impl DeserializeLexicon for LocalLexicon {
    #[cfg_attr(test, mutants::skip)] // Capacity hints affect allocation behavior, not observable values.
    fn with_entry_capacity(entries: usize) -> Self {
        Self::with_capacity(entries, 0)
    }
}

#[cfg(feature = "std")]
impl DeserializeLexicon for crate::ThreadedLexicon {
    #[cfg_attr(test, mutants::skip)] // Capacity hints affect allocation behavior, not observable values.
    fn with_entry_capacity(entries: usize) -> Self {
        Self::with_capacity_for_size_hint(entries)
    }
}

impl<'de, T> Visitor<'de> for StrSeqVisitor<T>
where
    T: DeserializeLexicon,
{
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence of interned strings")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<T, A::Error> {
        let entries = cautious_capacity::<Sym>(seq.size_hint());
        let mut lexicon = T::with_entry_capacity(entries);
        while seq.next_element_seed(DeserializeInSeed::<Sym, T>::new(&mut lexicon))?.is_some() {}
        Ok(lexicon)
    }
}

impl<'de> Deserialize<'de> for LocalLexicon {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(StrSeqVisitor(PhantomData))
    }
}

#[cfg(feature = "std")]
mod threaded {
    use core::marker::PhantomData;

    use serde::{Deserialize, Deserializer};

    use super::StrSeqVisitor;
    use crate::ThreadedLexicon;

    impl<'de> Deserialize<'de> for ThreadedLexicon {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_seq(StrSeqVisitor::<Self>(PhantomData))
        }
    }
}
