// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Serialize, Serializer};

use super::SerializeIn;
use crate::Reader;

/// Adapts a [`SerializeIn`] value into an ordinary [`Serialize`] by capturing a [`Reader`].
///
/// This is the serialization counterpart to
/// [`DeserializeInSeed`](crate::de::DeserializeInSeed): the derive macro emits
/// it for each field, and you can use it to hand a reader-aware value to any
/// Serde entry point (`serde_json::to_string`, `collect_seq`, …).
///
/// ```
/// use internity::se::SerializeInWith;
/// use internity::{LocalLexicon, Reader};
///
/// let mut lexicon = LocalLexicon::new();
/// let sym = lexicon.intern("hello");
/// let reader = lexicon.freeze();
/// let json = serde_json::to_string(&SerializeInWith::new(&sym, &reader)).unwrap();
/// assert_eq!(json, r#""hello""#);
/// ```
#[derive(Debug)]
pub struct SerializeInWith<'a, T: ?Sized, R: Reader + ?Sized> {
    value: &'a T,
    reader: &'a R,
}

impl<'a, T: ?Sized, R: Reader + ?Sized> SerializeInWith<'a, T, R> {
    /// Pair `value` with the `reader` used to resolve its handles.
    #[must_use]
    pub const fn new(value: &'a T, reader: &'a R) -> Self {
        Self { value, reader }
    }
}

impl<T, R> Serialize for SerializeInWith<'_, T, R>
where
    T: ?Sized + SerializeIn<R>,
    R: Reader + ?Sized,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.serialize_in(self.reader, serializer)
    }
}
