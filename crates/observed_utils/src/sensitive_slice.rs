// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A bounded, type-erased view over a collection of classified items.
//!
//! [`SensitiveSlice`] borrows at most `N` items from any iterator of
//! [`RedactedDisplay`] references and stores them inline, so a struct field can
//! hold a redactable collection without naming the iterator's type and without
//! allocating for the collection itself. Rendering delegates to each item's own
//! [`RedactedDisplay`] implementation, driven by the caller's [`Redactor`]: what
//! reaches the output - redacted, suppressed, or in the clear - is decided by
//! that redactor and those items, not by this module.

use std::fmt;

use arrayvec::ArrayVec;
use data_privacy::{RedactedDisplay, Redactor};

/// A type-erased collection of references to [`RedactedDisplay`] items.
///
/// This type does **not** require specifying the iterator type — it stores up
/// to `N` `&(dyn RedactedDisplay + Sync)` fat pointers in an inline array. This
/// makes struct definitions much cleaner when the exact iterator type is an
/// implementation detail. The [`Sync`] bound is part of the contract rather
/// than an implementation detail: `&(dyn Trait + Sync)` is itself [`Send`] and
/// [`Sync`], while a bare `&dyn Trait` is neither, so the bound is what lets an
/// event struct holding one cross threads.
///
/// When formatted via `RedactedDisplay`, it renders at most `N` items separated
/// by the delimiter `D`, appending `...` if there are more items.
///
/// # Performance
///
/// - **No heap allocation for the collection** — the item references are stored
///   inline in a fixed-size array. Rendering an individual item is that item's
///   own implementation and may allocate.
/// - Only the first `N` items are stored; whether the source had more items
///   is tracked to detect overflow.
/// - Virtual dispatch (`dyn RedactedDisplay`) per item during formatting,
///   bounded by `N`.
///
/// # Type parameters
///
/// - `N` — maximum number of items to render (default: `5`). Must be greater
///   than zero: `SensitiveSlice::<0>` is rejected at compile time.
/// - `D` — single-character delimiter between items (default: `','`).
///
/// # Examples
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed_utils::SensitiveSlice;
///
/// let emails = vec![
///     Sensitive::new("alice@example.com", DataClass::new("pii", "email")),
///     Sensitive::new("bob@example.com", DataClass::new("pii", "email")),
/// ];
///
/// // No iterator type needed — just `SensitiveSlice<'_, 5>`.
/// let sc = SensitiveSlice::<5>::new(emails.iter());
/// ```
///
/// With a custom limit and delimiter:
///
/// ```
/// use data_privacy::{DataClass, Sensitive};
/// use observed_utils::SensitiveSlice;
///
/// let items = vec![
///     Sensitive::new("a", DataClass::new("t", "v")),
///     Sensitive::new("b", DataClass::new("t", "v")),
///     Sensitive::new("c", DataClass::new("t", "v")),
/// ];
/// let sc = SensitiveSlice::<2, ';'>::new(items.iter());
/// // Renders as: "a; b; ..."
/// ```
pub struct SensitiveSlice<'a, const N: usize = 5, const D: char = ','> {
    items: ArrayVec<&'a (dyn RedactedDisplay + Sync), N>,
    /// Whether the source iterator had more items than `N`.
    overflowed: bool,
}

impl<'a, const N: usize, const D: char> SensitiveSlice<'a, N, D> {
    /// Collects items from any iterator of references to [`RedactedDisplay`]
    /// implementers. The item type must also be [`Sync`], matching the
    /// `&(dyn RedactedDisplay + Sync)` references this type stores.
    ///
    /// At most `N` items are stored inline. Whether there were additional
    /// items is recorded so overflow can be signaled during formatting.
    ///
    /// `N` must be greater than zero: a zero-capacity instantiation fails the
    /// compile-time assertion below rather than producing an always-empty
    /// collection.
    pub fn new<I, T>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a T>,
        T: RedactedDisplay + Sync + 'a,
    {
        const { assert!(N > 0, "N must be greater than zero") };

        let mut items = ArrayVec::new();
        let mut overflowed = false;

        // `is_full` is checked BEFORE the push, so once `N` items are stored the
        // next one is still pulled from the iterator - and then deliberately
        // dropped. Pulling it is what makes `overflowed` accurate: the flag
        // means "the source had more than `N` items", not "the source had at
        // least `N`". A source of exactly `N` items ends the loop through
        // `None` and leaves the flag false.
        for item in iter {
            if items.is_full() {
                overflowed = true;
                break;
            }
            items.push(item as &(dyn RedactedDisplay + Sync));
        }

        Self { items, overflowed }
    }
}

impl<const N: usize, const D: char> RedactedDisplay for SensitiveSlice<'_, N, D> {
    fn fmt(&self, redactor: &dyn Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const { assert!(N > 0, "N must be greater than zero") };

        if self.items.is_empty() {
            return Ok(());
        }

        RedactedDisplay::fmt(self.items[0], redactor, f)?;

        for item in &self.items[1..] {
            write!(f, "{D} ")?;
            RedactedDisplay::fmt(*item, redactor, f)?;
        }

        if self.overflowed {
            write!(f, "{D} ...")?;
        }

        Ok(())
    }
}

impl<const N: usize, const D: char> fmt::Debug for SensitiveSlice<'_, N, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SensitiveSlice")
            .field("stored", &self.items.len())
            .field("overflowed", &self.overflowed)
            .finish()
    }
}

impl<const N: usize, const D: char> Clone for SensitiveSlice<'_, N, D> {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            overflowed: self.overflowed,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use data_privacy::{DataClass, RedactedToString, RedactionEngine, Sensitive};

    use super::*;

    const TEST_CLASS: DataClass = DataClass::new("test", "test");

    fn passthrough_engine() -> RedactionEngine {
        RedactionEngine::builder().suppress_redaction(TEST_CLASS).build()
    }

    fn redacting_engine() -> RedactionEngine {
        RedactionEngine::builder()
            .add_class_redactor(TEST_CLASS, SimpleRedactor::with_mode(SimpleRedactorMode::Insert("X".into())))
            .build()
    }

    fn s(val: &str) -> Sensitive<String> {
        Sensitive::new(val.to_owned(), TEST_CLASS)
    }

    #[test]
    fn items_render_through_the_supplied_redactor() {
        let v = [s("alpha"), s("beta")];
        let sc = SensitiveSlice::<5>::new(v.iter());

        // The delimiter is the slice's own, but each item's text comes from the
        // caller's `Redactor`. Asserting against a redactor that actually
        // rewrites the text is what pins the delegation: with a pass-through
        // engine on both sides, dropping or swapping `redactor` would be
        // invisible.
        assert_eq!(sc.to_redacted_string(&redacting_engine()), "X, X");
        assert_eq!(sc.to_redacted_string(&passthrough_engine()), "alpha, beta");
    }

    #[test]
    fn empty_collection() {
        let v: [Sensitive<String>; 0] = [];
        let sc = SensitiveSlice::<5>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "");
    }

    #[test]
    fn single_element() {
        let v = [s("hello")];
        let sc = SensitiveSlice::<5>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "hello");
    }

    #[test]
    fn multiple_within_limit() {
        let v = [s("a"), s("b"), s("c")];
        let sc = SensitiveSlice::<5>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "a, b, c");
    }

    #[test]
    fn exactly_at_limit() {
        let v = [s("a"), s("b"), s("c")];
        let sc = SensitiveSlice::<3>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "a, b, c");
    }

    #[test]
    fn over_limit_truncates() {
        let v = [s("a"), s("b"), s("c"), s("d")];
        let sc = SensitiveSlice::<2>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "a, b, ...");
    }

    #[test]
    fn custom_delimiter() {
        let v = [s("x"), s("y"), s("z")];
        let sc = SensitiveSlice::<5, ';'>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "x; y; z");
    }

    #[test]
    fn custom_delimiter_with_truncation() {
        let v = [s("a"), s("b")];
        let sc = SensitiveSlice::<1, '|'>::new(v.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "a| ...");
    }

    #[test]
    fn hashmap_keys() {
        let mut map = HashMap::new();
        map.insert(s("key1"), 1);
        let sc = SensitiveSlice::<5>::new(map.keys());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "key1");
    }

    #[test]
    fn vecdeque_iter() {
        let deque: VecDeque<Sensitive<String>> = [s("alpha"), s("beta")].into_iter().collect();
        let sc = SensitiveSlice::<5>::new(deque.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "alpha, beta");
    }

    #[test]
    fn slice_iter() {
        let arr = [s("one"), s("two"), s("three")];
        let sc = SensitiveSlice::<5>::new(arr.iter());
        let result = sc.to_redacted_string(&passthrough_engine());
        assert_eq!(result, "one, two, three");
    }

    #[test]
    fn debug_reports_stored_count_and_overflow_without_leaking_items() {
        let v = [s("a"), s("b"), s("c")];

        let sc = SensitiveSlice::<5>::new(v.iter());
        let rendered = format!("{sc:?}");
        assert_eq!(rendered, "SensitiveSlice { stored: 3, overflowed: false }");

        let truncated = SensitiveSlice::<2>::new(v.iter());
        let rendered = format!("{truncated:?}");
        assert_eq!(rendered, "SensitiveSlice { stored: 2, overflowed: true }");
    }

    #[test]
    fn clone_preserves_items_and_overflow() {
        let v = [s("a"), s("b"), s("c")];
        let sc = SensitiveSlice::<2>::new(v.iter());
        let cloned = sc.clone();

        assert_eq!(
            cloned.to_redacted_string(&passthrough_engine()),
            sc.to_redacted_string(&passthrough_engine())
        );
        assert_eq!(cloned.to_redacted_string(&passthrough_engine()), "a, b, ...");
    }
}
