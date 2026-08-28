// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! String storage for telemetry attribute values.

use std::borrow::Cow;
use std::sync::Arc;
use std::{fmt, hash};

/// A string in a telemetry [`Value`](crate::Value), stored as a borrowed
/// `&'static str` or a shared [`Arc<str>`].
///
/// Two representations cover every call site:
///
/// - `Static` for values that reach `Text` as compile-time literals - free to
///   store and free to clone,
/// - `Shared` for owned, dynamic, or non-empty redacted text, cloned by bumping
///   a refcount.
///
/// Both clone in O(1), which matters because an enrichment's stored value is
/// cloned on every event that sees it. An owned `Box<str>` variant would let a
/// [`String`] hand over its buffer instead of copying once, at the cost of
/// copying on every clone thereafter - so a [`String`] is copied into a fresh
/// `Arc<str>` on the way in.
///
/// There is deliberately no conversion from a non-`'static` `&str`: that has to
/// copy, so it must be spelled out at the call site as `Arc::from(s)` rather
/// than hidden inside a `From` impl.
///
/// Two `Text`s are equal when their contents are equal, regardless of how each
/// one is stored.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Text {
    /// A compile-time string literal.
    Static(&'static str),
    /// A reference-counted string shared with the caller.
    Shared(Arc<str>),
}

impl Text {
    /// Returns the string contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Static(s) => s,
            Self::Shared(s) => s,
        }
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// Equality is by contents, not by representation: the same text stored as a
// literal and as an `Arc` must compare equal.
impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Text {}

impl hash::Hash for Text {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl From<&'static str> for Text {
    fn from(v: &'static str) -> Self {
        Self::Static(v)
    }
}

impl From<Arc<str>> for Text {
    fn from(v: Arc<str>) -> Self {
        Self::Shared(v)
    }
}

impl From<String> for Text {
    /// Copies into a fresh [`Arc<str>`].
    ///
    /// An `Arc<str>` is one allocation laid out as `[refcounts][bytes]`, so the
    /// refcount header sits *before* the data and a `String`'s existing buffer
    /// has nowhere to put it. Reusing the buffer would mean either an owned
    /// `Box<str>` variant, which cannot clone in O(1), or an extra indirection
    /// such as `Arc<String>`. Callers that already hold an `Arc<str>` avoid the
    /// copy by passing it directly.
    fn from(v: String) -> Self {
        Self::Shared(v.into())
    }
}

impl From<Cow<'static, str>> for Text {
    fn from(v: Cow<'static, str>) -> Self {
        match v {
            Cow::Borrowed(s) => Self::Static(s),
            Cow::Owned(s) => Self::Shared(s.into()),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_str_is_borrowed() {
        assert!(matches!(Text::from("hi"), Text::Static("hi")));
    }

    #[test]
    fn arc_is_shared_without_copying() {
        let arc: Arc<str> = Arc::from("shared");
        let ptr = Arc::as_ptr(&arc);
        let Text::Shared(stored) = Text::from(arc) else {
            panic!("expected a shared string");
        };
        assert_eq!(Arc::as_ptr(&stored), ptr);
    }

    #[test]
    fn string_becomes_shared() {
        let text = Text::from(String::from("owned"));
        assert!(matches!(text, Text::Shared(_)));
        assert_eq!(text.as_str(), "owned");
    }

    #[test]
    fn borrowed_cow_stays_static_and_owned_cow_is_shared() {
        assert!(matches!(Text::from(Cow::Borrowed("literal")), Text::Static(_)));
        assert!(matches!(Text::from(Cow::Owned(String::from("runtime"))), Text::Shared(_)));
    }

    #[test]
    fn equality_ignores_representation() {
        assert_eq!(Text::from("same"), Text::from(String::from("same")));
        assert_eq!(Text::from("same"), Text::from(Arc::<str>::from("same")));
        assert_ne!(Text::from("a"), Text::from("b"));
    }

    #[test]
    fn display_writes_contents() {
        assert_eq!(Text::from("hello").to_string(), "hello");
    }

    #[test]
    fn hash_agrees_with_equality_across_representations() {
        fn hash_of(text: &Text) -> u64 {
            use std::hash::{Hash as _, Hasher as _};

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        }

        // `Text` hashes its contents, so a literal and an owned string with the
        // same bytes are interchangeable as hash-map keys.
        assert_eq!(hash_of(&Text::from("same")), hash_of(&Text::from(String::from("same"))));

        // Distinct contents must reach the hasher, otherwise every `Text` would
        // land in one bucket and equality would be the only discriminator.
        assert_ne!(hash_of(&Text::from("same")), hash_of(&Text::from("other")));
    }
}
