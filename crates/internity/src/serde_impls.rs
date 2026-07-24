// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `serde` support (behind the `serde` feature) for [`Sym`] and the interners.
//!
//! * [`Sym`] serializes as its raw `u32`.
//! * [`Lexicon`] and [`ThreadedLexicon`] serialize as a sequence of their strings
//!   in a reproducible order; deserializing re-interns them, reproducing identical
//!   [`Sym`] handles. Deserialization uses the default hasher.

use alloc::string::String;
use core::fmt;
use core::hash::BuildHasher;
use core::marker::PhantomData;

use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Lexicon;
use crate::sym::Sym;

impl Serialize for Sym {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.as_u32())
    }
}

impl<'de> Deserialize<'de> for Sym {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        Self::from_u32(raw).ok_or_else(|| D::Error::custom("internity: Sym must be non-zero"))
    }
}

impl<S: BuildHasher> Serialize for Lexicon<S> {
    fn serialize<Se: Serializer>(&self, serializer: Se) -> Result<Se::Ok, Se::Error> {
        serializer.collect_seq(self.iter().map(|(_, s)| s))
    }
}

/// Visits a sequence of strings and interns them into a fresh interner.
struct StrSeqVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for StrSeqVisitor<T>
where
    T: FromIterator<String>,
{
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a sequence of interned strings")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<T, A::Error> {
        let mut strings = alloc::vec::Vec::new();
        while let Some(s) = seq.next_element::<String>()? {
            strings.push(s);
        }
        Ok(strings.into_iter().collect())
    }
}

impl<'de> Deserialize<'de> for Lexicon {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(StrSeqVisitor(PhantomData))
    }
}

#[cfg(feature = "std")]
mod threaded {
    use core::marker::PhantomData;

    use serde::{Deserialize, Deserializer};

    use super::{BuildHasher, Serialize, Serializer, StrSeqVisitor};
    use crate::{Reader, ThreadedLexicon};

    impl<S: BuildHasher> Serialize for ThreadedLexicon<S> {
        fn serialize<Se: Serializer>(&self, serializer: Se) -> Result<Se::Ok, Se::Error> {
            // Freeze a copy to iterate the strings in shard order (grouped per
            // shard, in local order), which re-interning on deserialize reproduces.
            let reader = self.clone().freeze();
            serializer.collect_seq(reader.iter().map(|(_, s)| s))
        }
    }

    impl<'de> Deserialize<'de> for ThreadedLexicon {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_seq(StrSeqVisitor::<Self>(PhantomData))
        }
    }
}
