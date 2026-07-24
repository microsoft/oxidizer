// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Serialize, Serializer};

use crate::Reader;

/// Serializes an interner's strings as a sequence, in [`Reader`] iteration order.
///
/// # Handle preservation requires a matching layout
///
/// The serialized form is a bare string sequence; it does **not** record which
/// engine produced it. Deserializing it rebuilds handles from scratch, so the
/// reconstructed [`Sym`](crate::Sym) values equal the originals **only when the
/// target engine reproduces the source engine's handle layout**:
///
/// - A reader from a [`LocalLexicon`](crate::LocalLexicon) (dense layout)
///   preserves handles only when restored into a
///   [`LocalLexicon`](crate::LocalLexicon).
/// - A reader from a [`ThreadedLexicon`](crate::ThreadedLexicon) (sharded layout)
///   preserves handles only when restored into a
///   [`ThreadedLexicon`](crate::ThreadedLexicon) using its default hasher and the
///   same shard count.
///
/// Crossing engines (flat sequence → `ThreadedLexicon`, or sharded sequence →
/// `LocalLexicon`) still restores the same *strings*, but assigns different
/// handles, so a persisted `Sym` may then refer to a different string. When only
/// the strings matter, any engine is fine; when handle identity must survive,
/// restore into the matching engine.
///
/// ```
/// use internity::LocalLexicon;
/// use internity::se::SerializeReader;
///
/// let mut lexicon = LocalLexicon::new();
/// lexicon.intern("a");
/// lexicon.intern("b");
/// let reader = lexicon.freeze();
/// let json = serde_json::to_string(&SerializeReader(&reader)).unwrap();
/// assert_eq!(json, r#"["a","b"]"#);
/// ```
#[derive(Debug)]
pub struct SerializeReader<'a, R: Reader + ?Sized>(pub &'a R);

impl<R: Reader + ?Sized> Serialize for SerializeReader<'_, R> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.0.iter().map(|(_, value)| value))
    }
}
