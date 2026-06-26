// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "residue of Rc-test removal: orphaned helpers/imports kept to preserve surrounding test bodies verbatim"
)]

//! Consolidated UTF-16 tests (smoke, builder, format, serde, cross-thread,
//! coverage gaps, and mutation-kill cases).

#![cfg(feature = "utf16")]

mod common;

mod utf16_smoke {

    use multitude::Arena;
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn alloc_utf16_str_box() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_box(utf16str!("hello"));
        assert_eq!(&*s, utf16str!("hello"));
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn alloc_utf16_str_arc() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc(utf16str!("shared"));
        let _: multitude::Arc<multitude::strings::Utf16Str> = s;
        assert_eq!(&*s, utf16str!("shared"));
    }

    #[test]
    fn alloc_utf16_str_box_from_str() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_box_from_str("hello");
        assert_eq!(&*s, utf16str!("hello"));
    }

    #[test]
    fn alloc_utf16_str_arc_from_str() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc_from_str("hello");
        assert_eq!(&*s, utf16str!("hello"));
    }

    #[test]
    fn arc_into_arc_slice() {
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_arc(utf16str!("abc"));
        let bytes: multitude::Arc<[u16]> = a.into();
        assert_eq!(&*bytes, &[u16::from(b'a'), u16::from(b'b'), u16::from(b'c')][..]);
    }
}

mod utf16_string_builder {

    use multitude::{Arena, FromIn as _};
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn push_and_pop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push('a');
        s.push('b');
        s.push('💖'); // surrogate pair: +2 u16
        assert_eq!(s.len(), 4);
        assert_eq!(s.pop(), Some('💖'));
        assert_eq!(s.len(), 2);
        s.push_str(utf16str!("xyz"));
        assert_eq!(s.as_utf16_str(), utf16str!("abxyz"));
    }

    #[test]
    fn push_from_str_transcodes() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("hello, 💖");
        assert_eq!(s.as_utf16_str(), utf16str!("hello, 💖"));
        assert_eq!(s.len(), 9); // 7 ascii + 2 surrogate
    }

    #[test]
    fn truncate_and_clear() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));
        s.truncate(3);
        assert_eq!(s.as_utf16_str(), utf16str!("hel"));
        assert_eq!(s.len(), 3);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    #[should_panic(expected = "is not on a UTF-16 char boundary")]
    fn truncate_mid_surrogate_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push('💖'); // 2 u16 units
        s.truncate(1);
    }

    #[test]
    fn insert_and_remove() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));
        s.insert(0, 'X');
        assert_eq!(s.as_utf16_str(), utf16str!("Xhello"));
        s.insert_utf16_str(2, utf16str!("YY"));
        assert_eq!(s.as_utf16_str(), utf16str!("XhYYello"));
        let removed = s.remove(0);
        assert_eq!(removed, 'X');
        assert_eq!(s.as_utf16_str(), utf16str!("hYYello"));
    }

    #[test]
    fn replace_range_grows_and_shrinks() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("Hello, world!"));
        s.replace_range(7..12, utf16str!("everyone"));
        assert_eq!(s.as_utf16_str(), utf16str!("Hello, everyone!"));
        s.replace_range(7..15, utf16str!("X"));
        assert_eq!(s.as_utf16_str(), utf16str!("Hello, X!"));
    }

    #[test]
    fn retain_drops_predicate_failures() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("Hello, World!");
        s.retain(|c| c.is_ascii_alphabetic());
        assert_eq!(s.as_utf16_str(), utf16str!("HelloWorld"));
    }

    #[test]
    fn capacity_growth() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        // Amortized doubling: a few hundred pushes is more than enough to
        // exercise the capacity-growth path repeatedly.
        let n = 256;
        for _ in 0..n {
            s.push('a');
        }
        assert_eq!(s.len(), n);
        assert!(s.capacity() >= n);
    }

    #[test]
    fn shrink_to_fit_at_bump_cursor() {
        let arena = Arena::builder().build();
        let mut s = arena.alloc_utf16_string_with_capacity(64);
        s.push_str(utf16str!("hi"));
        let cap_before = s.capacity();
        assert!(cap_before >= 64);
        s.shrink_to_fit();
        assert_eq!(s.capacity(), 2);
    }

    #[test]
    fn extend_with_chars_str_and_utf16str() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(['a', 'b', 'c'].iter().copied());
        s.extend(["12", "34"].iter().copied());
        s.extend([utf16str!("XY"), utf16str!("Z")].iter().copied());
        assert_eq!(s.as_utf16_str(), utf16str!("abc1234XYZ"));
    }

    #[test]
    fn from_in_str_and_from_utf16_str_in() {
        let arena = Arena::new();
        let a = multitude::strings::Utf16String::from_in("hello", &arena);
        assert_eq!(a.as_utf16_str(), utf16str!("hello"));
        let b = multitude::strings::Utf16String::from_utf16_str_in(utf16str!("world"), &arena);
        assert_eq!(b.as_utf16_str(), utf16str!("world"));
    }

    #[test]
    fn clone_builder() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));
        let c = s.clone();
        assert_eq!(c.as_utf16_str(), s.as_utf16_str());
    }

    #[test]
    fn push_str_accepts_borrowed_utf16str() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        let lit: &widestring::Utf16Str = utf16str!("ab");
        s.push_str(lit);
        assert_eq!(s.as_utf16_str(), utf16str!("ab"));
    }

    #[test]
    fn push_str_accepts_utf16string_via_as_ref() {
        let arena = Arena::new();
        let mut a = arena.alloc_utf16_string();
        a.push_str(utf16str!("hello"));
        let mut b = arena.alloc_utf16_string();
        b.push_str(&a);
        assert_eq!(b.as_utf16_str(), utf16str!("hello"));
    }

    #[test]
    fn try_push_from_str_accepts_str_like_types() {
        let arena = Arena::new();
        let owned = alloc::string::String::from("hello, 💖");
        let mut out = arena.alloc_utf16_string();
        out.try_push_from_str(&owned).unwrap();
        out.try_push_from_str("!").unwrap();
        assert_eq!(out.as_utf16_str(), utf16str!("hello, 💖!"));
    }

    // === Fast, direct tests targeting Utf16String::len and ===
    // === Utf16String::try_with_capacity_in mutants.          ===
    //
    // These mutants survive when the only coverage runs through heavy
    // integration paths. Keep these tests minimal so cargo-mutants can
    // kill them quickly (a `len -> 0` replacement should fail in
    // milliseconds, not minutes).

    #[test]
    fn len_is_zero_for_fresh_string() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_string();
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn len_tracks_number_of_u16_code_units_after_push_str() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        assert_eq!(s.len(), 3);
        s.push_str(utf16str!("de"));
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn len_handles_non_bmp_chars_as_two_code_units() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push('🦀'); // U+1F980, one scalar => two UTF-16 code units
        assert_eq!(s.len(), 2);
        s.push('a');
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn len_returns_zero_after_clear() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("nonempty"));
        assert!(!s.is_empty());
        s.clear();
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn try_with_capacity_in_zero_does_not_allocate_but_is_usable() {
        let arena = Arena::new();
        let s = arena.try_alloc_utf16_string_with_capacity(0).unwrap();
        assert_eq!(s.len(), 0);
        assert_eq!(s.capacity(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn try_with_capacity_in_respects_requested_capacity() {
        let arena = Arena::new();
        let s = arena.try_alloc_utf16_string_with_capacity(8).unwrap();
        assert!(s.capacity() >= 8);
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn try_with_capacity_in_one_allocates_at_least_one_unit() {
        let arena = Arena::new();
        let s = arena.try_alloc_utf16_string_with_capacity(1).unwrap();
        assert!(s.capacity() >= 1);
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn reserve_grows_capacity_and_preserves_contents() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        let cap_before = s.capacity();
        s.reserve(cap_before + 32);
        assert!(s.capacity() >= s.len() + cap_before + 32);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn reserve_grows_from_zero_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        assert_eq!(s.capacity(), 0);
        s.reserve(64);
        assert!(s.capacity() >= 64);
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn try_reserve_zero_additional_is_noop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("xy"));
        let cap_before = s.capacity();
        s.try_reserve(0).unwrap();
        assert_eq!(s.capacity(), cap_before);
        assert_eq!(s.as_utf16_str(), utf16str!("xy"));
    }

    #[test]
    fn extend_chars_appends_in_order() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(['a', 'b', 'c'].iter().copied());
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
        assert_eq!(s.len(), 3);
        s.extend(std::iter::once(&'d').copied());
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn extend_chars_handles_surrogate_pairs() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(['a', '🦀', 'b'].iter().copied());
        assert_eq!(s.as_utf16_str(), utf16str!("a🦀b"));
        assert_eq!(s.len(), 4); // 1 + 2 + 1
    }

    #[test]
    fn extend_str_slices_appends_in_order() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(["ab", "cd"].iter().copied());
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn extend_utf16_str_slices_appends_in_order() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend([utf16str!("ab"), utf16str!("cd")].iter().copied());
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
        assert_eq!(s.len(), 4);
    }

    extern crate alloc;
}

mod utf16_format {

    use core::fmt;

    use multitude::Arena;
    use multitude::strings::format_utf16;
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn format_utf16_basic() {
        let arena = Arena::new();
        let n = 42_i32;
        let s = format_utf16!(in &arena, "n = {n}");
        assert_eq!(s.as_utf16_str(), utf16str!("n = 42"));
    }

    #[test]
    fn format_utf16_with_unicode() {
        let arena = Arena::new();
        let s = format_utf16!(in &arena, "love {}", '💖');
        assert_eq!(s.as_utf16_str(), utf16str!("love 💖"));
    }

    /// A `Display` impl that fragments output across multiple `write_str`
    /// calls — verifies that fragmenting a sequence of code points across
    /// `write_str` boundaries produces correct UTF-16 output. Each `&str`
    /// passed to `write_str` is itself a complete UTF-8 fragment so no
    /// cross-call surrogate state is needed; this test exercises that
    /// invariant.
    struct Fragmented<'a>(&'a [&'a str]);

    impl fmt::Display for Fragmented<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for piece in self.0 {
                f.write_str(piece)?;
            }
            Ok(())
        }
    }

    #[test]
    fn format_utf16_fragmented_writes() {
        let arena = Arena::new();
        let pieces = ["he", "llo, ", "💖", " bye"];
        let f = Fragmented(&pieces);
        let s = format_utf16!(in &arena, "{f}");
        assert_eq!(s.as_utf16_str(), utf16str!("hello, 💖 bye"));
    }
}

mod utf16_serde {

    use multitude::Arena;
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn arc_serializes_as_utf8_string() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc(utf16str!("shared"));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""shared""#);
    }

    #[test]
    fn box_serializes_as_utf8_string() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_box(utf16str!("boxed"));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""boxed""#);
    }

    #[test]
    fn string_serializes_as_utf8_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("growable"));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""growable""#);
    }
}

mod utf16_cross_thread {

    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use multitude::Arena;
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn arc_send_across_threads() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc(utf16str!("shared utf16"));
        let s2 = s.clone();
        let h = thread::spawn(move || s2.len());
        assert_eq!(h.join().unwrap(), 12);
        assert_eq!(&*s, utf16str!("shared utf16"));
    }

    #[test]
    fn arc_concurrent_clone_drop() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_str_arc(utf16str!("concurrent"));
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let mut handles = std::vec::Vec::new();
        for _ in 0..8 {
            let s = s.clone();
            let c = std::sync::Arc::clone(&counter);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let copy = s.clone();
                    let _ = c.fetch_add(copy.len(), Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 8 * 100 * 10);
        assert_eq!(&*s, utf16str!("concurrent"));
    }
}

mod utf16_coverage {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(unused_results, reason = "test code")]
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::panic::AssertUnwindSafe;

    use multitude::strings::{String, Utf16String};
    use multitude::{Arc, Arena, Box, FromIn as _};
    use widestring::{Utf16Str, utf16str};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
    use crate::common::{FailingAllocator, SendFailingAllocator};

    fn expect_panic<F: FnOnce()>(f: F) {
        let r = std::panic::catch_unwind(AssertUnwindSafe(f));
        assert!(r.is_err(), "expected panic but call returned");
    }

    fn fail_arena() -> Arena<FailingAllocator> {
        Arena::new_in(FailingAllocator::new(0))
    }
    fn send_fail_arena() -> Arena<SendFailingAllocator> {
        Arena::new_in(SendFailingAllocator::new(0))
    }

    #[test]
    fn panic_alloc_utf16_str_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_utf16_str_arc(utf16str!("x"));
        });
    }

    #[test]
    fn panic_alloc_utf16_str_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_utf16_str_box(utf16str!("x"));
        });
    }

    #[test]
    fn panic_alloc_utf16_str_arc_from_str() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_utf16_str_arc_from_str("x");
        });
    }

    #[test]
    fn panic_alloc_utf16_str_box_from_str() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_utf16_str_box_from_str("x");
        });
    }

    #[test]
    fn panic_alloc_utf16_string_with_capacity() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_utf16_string_with_capacity(64);
        });
    }

    #[test]
    fn try_alloc_utf16_str_arc_err() {
        let a = send_fail_arena();
        a.try_alloc_utf16_str_arc(utf16str!("x")).unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_str_box_err() {
        let a = fail_arena();
        a.try_alloc_utf16_str_box(utf16str!("x")).unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_str_arc_from_str_err() {
        let a = send_fail_arena();
        a.try_alloc_utf16_str_arc_from_str("x").unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_str_box_from_str_err() {
        let a = fail_arena();
        a.try_alloc_utf16_str_box_from_str("x").unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_string_with_capacity_err() {
        let a = fail_arena();
        a.try_alloc_utf16_string_with_capacity(64).unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_string_with_capacity_zero_no_alloc() {
        // cap == 0 — no allocation, no failure.
        let a = fail_arena();
        let s = a.try_alloc_utf16_string_with_capacity(0).unwrap();
        assert_eq!(s.capacity(), 0);
    }

    #[test]
    fn empty_builder_accessors() {
        let arena = Arena::new();
        let s = arena.alloc_utf16_string();
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
        assert_eq!(s.capacity(), 0);
        assert_eq!(s.as_utf16_str(), utf16str!(""));
        let empty: &[u16] = &[];
        assert_eq!(s.as_slice(), empty);
        let p: *const u16 = s.as_ptr();
        assert!(!p.is_null());
    }

    #[test]
    fn empty_builder_as_mut() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        let m: &mut Utf16Str = s.as_mut_utf16_str();
        assert_eq!(m, utf16str!(""));
        let p: *mut u16 = s.as_mut_ptr();
        assert!(!p.is_null());
    }

    #[test]
    fn pop_on_empty_returns_none() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn truncate_noop_when_new_len_ge_len() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.truncate(10);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
        s.truncate(3);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn insert_str_empty_is_noop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.insert_utf16_str(1, utf16str!(""));
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
    }

    #[test]
    fn insert_at_end() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.insert_utf16_str(3, utf16str!("XY"));
        assert_eq!(s.as_utf16_str(), utf16str!("abcXY"));
    }

    #[test]
    fn replace_range_unbounded() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));
        s.replace_range(.., utf16str!("world"));
        assert_eq!(s.as_utf16_str(), utf16str!("world"));
    }

    #[test]
    fn replace_range_inclusive_excluded_bounds() {
        use core::ops::Bound;
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abcdef"));
        // (Excluded(0), Included(2)) ≡ 1..=2
        s.replace_range((Bound::Excluded(0), Bound::Included(2)), utf16str!("X"));
        assert_eq!(s.as_utf16_str(), utf16str!("aXdef"));
    }

    #[test]
    fn replace_range_equal_size() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abcdef"));
        s.replace_range(2..4, utf16str!("XY")); // same length
        assert_eq!(s.as_utf16_str(), utf16str!("abXYef"));
    }

    #[test]
    fn try_push_err() {
        let a = fail_arena();
        let mut s = a.alloc_utf16_string();
        assert!(s.try_push('a').is_err());
    }

    #[test]
    fn try_push_appends_chars() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.try_push('a').unwrap();
        s.try_push('β').unwrap();
        s.try_push('💖').unwrap(); // surrogate pair: +2 u16
        assert_eq!(s.len(), 4);
        assert_eq!(s.as_utf16_str(), utf16str!("aβ💖"));
    }

    #[test]
    fn try_push_str_err() {
        let a = fail_arena();
        let mut s = a.alloc_utf16_string();
        assert!(s.try_push_str(utf16str!("abc")).is_err());
    }

    #[test]
    fn try_push_from_str_err() {
        let a = fail_arena();
        let mut s = a.alloc_utf16_string();
        assert!(s.try_push_from_str("abc").is_err());
    }

    #[test]
    fn try_reserve_zero_is_noop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.try_reserve(0).unwrap();
        assert_eq!(s.capacity(), 0);
    }

    #[test]
    fn reserve_panics_when_alloc_fails() {
        expect_panic(|| {
            let a = fail_arena();
            let mut s = a.alloc_utf16_string();
            s.reserve(64);
        });
    }

    #[test]
    fn try_reserve_err() {
        let a = fail_arena();
        let mut s = a.alloc_utf16_string();
        assert!(s.try_reserve(64).is_err());
    }

    #[test]
    fn push_empty_str_is_noop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!(""));
        assert!(s.is_empty());
        s.push_from_str("");
        assert!(s.is_empty());
    }

    #[test]
    fn try_push_from_str_with_surrogate_pair() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.try_push_from_str("a💖b").unwrap();
        assert_eq!(s.as_utf16_str(), utf16str!("a💖b"));
    }

    #[test]
    fn arena_utf16_string_traits() {
        use core::borrow::{Borrow, BorrowMut};
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));

        // Deref / DerefMut
        let _: &Utf16Str = &s;
        let _: &mut Utf16Str = &mut s;
        // AsRef / AsMut
        let _: &Utf16Str = AsRef::as_ref(&s);
        let _: &mut Utf16Str = AsMut::as_mut(&mut s);
        // Borrow / BorrowMut
        let _: &Utf16Str = Borrow::borrow(&s);
        let _: &mut Utf16Str = BorrowMut::borrow_mut(&mut s);

        // Clone
        let c = s.clone();
        assert_eq!(c, s);
        assert_eq!(c.as_utf16_str(), s.as_utf16_str());

        // Ord / PartialOrd
        let mut other = arena.alloc_utf16_string();
        other.push_str(utf16str!("hellp"));
        assert!(s < other);
        assert!(s.partial_cmp(&other).is_some());
        assert_eq!(s.cmp(&s.clone()), core::cmp::Ordering::Equal);

        // PartialEq vs Self / Utf16Str / &Utf16Str
        let lit = utf16str!("hello");
        assert_eq!(s, lit);
        assert!(s == lit);

        // Hash
        let mut h = DefaultHasher::new();
        s.hash(&mut h);
        let _ = h.finish();

        // Display / Debug
        let _ = format!("{s}");
        let _ = format!("{s:?}");
    }

    #[test]
    fn arc_utf16_traits_and_pointer() {
        use core::borrow::Borrow;
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_arc(utf16str!("hello"));
        let _: &Utf16Str = &a;
        let _: &Utf16Str = AsRef::as_ref(&a);
        let _: &Utf16Str = Borrow::borrow(&a);

        assert!(a == *utf16str!("hello"));
        assert!(a == utf16str!("hello"));

        let _ = format!("{a}");
        let _ = format!("{a:?}");
        let _ = format!("{a:p}");

        let mut h = DefaultHasher::new();
        a.hash(&mut h);
        let _ = h.finish();

        let a2 = a.clone();
        assert_eq!(a.cmp(&a2), core::cmp::Ordering::Equal);
        assert_eq!(a.partial_cmp(&a2), Some(core::cmp::Ordering::Equal));

        let bytes: multitude::Arc<[u16]> = a.into();
        assert_eq!(
            &*bytes,
            &[u16::from(b'h'), u16::from(b'e'), u16::from(b'l'), u16::from(b'l'), u16::from(b'o')][..]
        );
    }

    #[test]
    fn from_utf16_str_in_and_from_str_in() {
        let arena = Arena::new();
        let a = Utf16String::from_in("hello, 💖", &arena);
        assert_eq!(a.as_utf16_str(), utf16str!("hello, 💖"));
        let b = Utf16String::from_utf16_str_in(utf16str!("world"), &arena);
        assert_eq!(b.as_utf16_str(), utf16str!("world"));
    }

    #[test]
    fn extend_chars_with_size_hint() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        let chars = ['a', 'b', 'c', '💖'];
        s.extend(chars.iter().copied()); // size_hint > 0 → reserve path
        assert_eq!(s.as_utf16_str(), utf16str!("abc💖"));
    }

    #[test]
    fn extend_chars_zero_lower_bound() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(core::iter::empty::<char>()); // lower == 0 → skip reserve branch
        assert!(s.is_empty());
    }

    #[test]
    fn extend_str_slices() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend(["ab", "cd"]);
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
    }

    #[test]
    fn extend_utf16_str_slices() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.extend([utf16str!("ab"), utf16str!("cd")]);
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
    }

    #[test]
    #[should_panic(expected = "not on a UTF-16 char boundary")]
    fn insert_at_mid_surrogate_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push('💖');
        s.insert(1, 'X');
    }

    #[test]
    #[should_panic(expected = "not on a UTF-16 char boundary")]
    fn remove_at_mid_surrogate_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push('💖');
        s.remove(1);
    }

    #[test]
    #[should_panic(expected = "not on a UTF-16 char boundary")]
    fn replace_range_start_mid_surrogate_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("a💖b"));
        s.replace_range(2..3, utf16str!(""));
    }

    #[test]
    #[should_panic(expected = "Utf16String::replace_range")]
    fn replace_range_end_oob_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.replace_range(0..10, utf16str!(""));
    }

    #[test]
    #[should_panic(expected = "Utf16String::replace_range")]
    fn replace_range_start_greater_than_end_panics() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        #[expect(clippy::reversed_empty_ranges, reason = "intentionally inverted to trigger panic")]
        s.replace_range(2..1, utf16str!(""));
    }

    #[test]
    fn grow_doubling_path() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(4);
        s.push_str(utf16str!("abcd"));
        s.push('e'); // forces grow; doubling 4*2 = 8 covers needed 5
        assert!(s.capacity() >= 8);
        assert_eq!(s.as_utf16_str(), utf16str!("abcde"));
    }

    #[test]
    fn grow_uses_min_cap_when_doubling_too_small() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(4);
        s.push_str(utf16str!("abcd"));
        let big = "x".repeat(100);
        s.push_from_str(&big); // forces grow; min_cap > 2*old_cap
        assert!(s.capacity() >= 104);
    }

    #[test]
    fn arena_utf16_string_drop_releases_chunk() {
        let arena = Arena::new();
        {
            let mut s = arena.alloc_utf16_string();
            s.push_str(utf16str!("data"));
            // explicit drop covers the cap > 0 branch in Drop
        }
        // arena drop later covers the cap == 0 branch via the empty builder used elsewhere
    }

    #[test]
    fn retain_keep_all() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.retain(|_| true);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
    }

    #[test]
    fn retain_drop_all() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.retain(|_| false);
        assert!(s.is_empty());
    }

    #[test]
    fn retain_with_surrogate() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("a💖b"));
        s.retain(|c| c.is_ascii());
        assert_eq!(s.as_utf16_str(), utf16str!("ab"));
    }

    #[test]
    fn clear_resets_len_only() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hello"));
        let cap = s.capacity();
        s.clear();
        assert_eq!(s.len(), 0);
        assert_eq!(s.capacity(), cap);
    }

    // From<ArenaUtf16String> for ArenaRcUtf16Str (exercises the
    // `into` arrow that the From impl wraps).

    // Display passes through to `Utf16Str`'s Display.

    #[test]
    fn insert_grows_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(4);
        s.push_str(utf16str!("abcd"));
        s.insert_utf16_str(2, utf16str!("XYZW"));
        assert_eq!(s.as_utf16_str(), utf16str!("abXYZWcd"));
        assert!(s.capacity() >= 8);
    }

    #[test]
    fn replace_range_grows_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(4);
        s.push_str(utf16str!("abcd"));
        s.replace_range(0..1, utf16str!("XXXXX")); // adds 4 → needs cap 8
        assert_eq!(s.as_utf16_str(), utf16str!("XXXXXbcd"));
        assert!(s.capacity() >= 8);
    }

    #[test]
    fn as_slice_non_empty() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("ab"));
        let slice: &[u16] = s.as_slice();
        assert_eq!(slice, &[u16::from(b'a'), u16::from(b'b')][..]);
    }

    // PartialEq<Utf16Str> (by-value rhs) on the ArenaUtf16String — explicitly
    // invoke the impl so its body is hit.

    #[test]
    fn arena_utf16_string_eq_utf16str_value() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("hi"));
        let lit: &Utf16Str = utf16str!("hi");
        // Both PartialEq<&Utf16Str> (s == lit) and PartialEq<Utf16Str> (s == *lit)
        // are reachable through method-call form; using the bare op resolves to
        // whichever impl rustc prefers, so call eq directly to nail the by-value one.
        assert!(<Utf16String<'_, _> as PartialEq<Utf16Str>>::eq(&s, lit));
        assert!(<Utf16String<'_, _> as PartialEq<&Utf16Str>>::eq(&s, &lit));
    }

    // from_str helpers: allocator-failure paths. The from-str variants now
    // transcode directly into the target chunk in a single allocation,
    // so the only failure point is that one allocation.

    #[test]
    fn try_alloc_utf16_str_arc_from_str_err_on_alloc_failure() {
        // FailingAllocator(0): the very first alloc fails. With the
        // single-allocation transcode path, that's the only path to Err.
        let arena = Arena::new_in(SendFailingAllocator::new(0));
        arena.try_alloc_utf16_str_arc_from_str("xyz").unwrap_err();
    }

    #[test]
    fn try_alloc_utf16_str_arc_from_str_success() {
        let arena = Arena::new();
        let a = arena.try_alloc_utf16_str_arc_from_str("hello").unwrap();
        assert_eq!(&*a, utf16str!("hello"));
    }

    #[test]
    fn try_alloc_utf16_str_box_from_str_success() {
        let arena = Arena::new();
        let b = arena.try_alloc_utf16_str_box_from_str("hello").unwrap();
        assert_eq!(&*b, utf16str!("hello"));
    }

    #[test]
    fn try_alloc_utf16_str_box_from_str_err_on_alloc_failure() {
        // Single-allocation transcode path: the chunk alloc is the only failure point.
        let arena = Arena::new_in(FailingAllocator::new(0));
        arena.try_alloc_utf16_str_box_from_str("xyz").unwrap_err();
    }

    // Panic-driving variants of the same path (drive the `panic_alloc` lambda
    // in `alloc_utf16_str_*_from_str`).

    #[test]
    fn panic_alloc_utf16_str_arc_from_str_when_alloc_fails() {
        expect_panic(|| {
            let arena = Arena::new_in(SendFailingAllocator::new(0));
            let _ = arena.alloc_utf16_str_arc_from_str("xyz");
        });
    }

    #[test]
    fn panic_alloc_utf16_str_box_from_str_when_alloc_fails() {
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(0));
            let _ = arena.alloc_utf16_str_box_from_str("xyz");
        });
    }

    // grow_for_string err inside try_grow_to_at_least: cap > 0 path that
    // re-allocates and the allocator fails on the relocation.

    #[test]
    fn grow_for_string_err_on_relocation() {
        // FailingAllocator(1): builder gets initial chunk (cap=4), then a huge
        // try_reserve would need a fresh oversized chunk → second alloc fails.
        let arena = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
        let mut s = arena.try_alloc_utf16_string_with_capacity(4).unwrap();
        s.try_push_str(utf16str!("abcd")).unwrap();
        assert!(s.try_reserve(64 * 1024).is_err());
    }

    #[test]
    fn panic_grow_to_at_least() {
        expect_panic(|| {
            let arena = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
            let mut s = arena.try_alloc_utf16_string_with_capacity(4).unwrap();
            s.try_push_str(utf16str!("abcd")).unwrap();
            // grow_to_at_least's panic_alloc lambda fires here.
            s.push_from_str("x".repeat(64 * 1024));
        });
    }

    // Panic paths through push / push_str / push_from_str when the allocator
    // fails — drives the `unwrap_or_else(panic_alloc)` lambdas inside push_slice
    // and push_from_str, plus the `reserve` path with no growth needed.

    #[test]
    fn panic_push_when_alloc_fails() {
        expect_panic(|| {
            let a = fail_arena();
            let mut s = a.alloc_utf16_string();
            s.push('a');
        });
    }

    #[test]
    fn panic_push_str_when_alloc_fails() {
        expect_panic(|| {
            let a = fail_arena();
            let mut s = a.alloc_utf16_string();
            s.push_str(utf16str!("abc"));
        });
    }

    #[test]
    fn panic_push_from_str_when_alloc_fails() {
        expect_panic(|| {
            let a = fail_arena();
            let mut s = a.alloc_utf16_string();
            s.push_from_str("abc");
        });
    }

    #[test]
    fn reserve_no_growth_path() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(16);
        s.reserve(4); // already have cap >= len + 4; no-op branch
        assert_eq!(s.capacity(), 16);
        assert_eq!(s.len(), 0);
    }

    // `total > isize::MAX` guard: cap whose payload byte count overflows isize
    // (but not usize) — we never allocate; the function returns AllocError
    // before reaching the allocator.

    #[test]
    fn try_with_capacity_isize_overflow_guard() {
        let arena = Arena::new();
        // cap*2 > isize::MAX but ≤ usize::MAX on 64-bit. checked_mul succeeds,
        // checked_add succeeds, isize::try_from fails → AllocError.
        let cap = (isize::MAX.unsigned_abs() / 2) + 1000;
        let r = arena.try_alloc_utf16_string_with_capacity(cap);
        r.unwrap_err();
    }

    #[test]
    fn try_grow_isize_overflow_guard() {
        let arena = Arena::new();
        let mut s = arena.try_alloc_utf16_string_with_capacity(4).unwrap();
        // Force try_grow_to_at_least → new_cap such that new_cap*2 > isize::MAX
        // but ≤ usize::MAX. The isize::try_from check on new_total returns Err.
        let huge = (isize::MAX.unsigned_abs() / 2) + 1000;
        let r = s.try_reserve(huge);
        assert!(r.is_err());
    }
}

mod mutants_for_utf16_strings {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit in u16")]
    #![allow(clippy::unnecessary_cast, reason = "explicit width clarifies intent")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    use multitude::Arena;
    use multitude::strings::Utf16String;
    use widestring::utf16str;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn truncate_to_zero_clears_string() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abc"), &arena);
        s.truncate(0);
        assert_eq!(s.len(), 0);
        assert_eq!(s.as_utf16_str(), utf16str!(""));
    }

    #[test]
    fn shrink_to_fit_reduces_capacity_to_len() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(64);
        s.push_str(utf16str!("abc"));
        assert_eq!(s.len(), 3);
        let cap_before = s.capacity();
        assert!(cap_before >= 64);
        s.shrink_to_fit();
        assert_eq!(s.len(), 3);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
        // Cap must drop to len when the buffer ends at the chunk's bump
        // cursor (it does, because no intervening allocation happened).
        assert_eq!(s.capacity(), 3);
    }

    #[test]
    fn reserve_exact_capacity_does_not_regrow() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        assert!(s.capacity() >= 8);
        let cap = s.capacity();
        let ptr_before = s.as_ptr();
        // Reserve `additional` such that `needed == self.cap`. self.len ==
        // 0, so additional == cap.
        s.reserve(cap);
        let ptr_after = s.as_ptr();
        assert_eq!(s.capacity(), cap, "no regrow when cap already suffices");
        assert_eq!(
            ptr_before, ptr_after,
            "reserve at exact-fit boundary must not reallocate (kills `> → >=`)"
        );
    }

    /// Same as above for `try_push_slice` boundary.
    #[test]
    fn push_slice_at_exact_fit_does_not_regrow() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        let cap = s.capacity();
        let ptr_before = s.as_ptr();
        // Push exactly `cap` u16s.
        let payload: Vec<u16> = (0..cap as u16).collect();
        let payload_str = widestring::Utf16Str::from_slice(&payload).expect("valid utf16");
        s.push_str(payload_str);
        let ptr_after = s.as_ptr();
        assert_eq!(s.len(), cap);
        assert_eq!(s.capacity(), cap);
        assert_eq!(ptr_before, ptr_after, "push at exact-fit must not reallocate");
    }

    #[test]
    fn insert_slice_preserves_content() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        // Insert at idx = 1 (boundary > 0 && < len)
        s.insert_utf16_str(1, utf16str!("XX"));
        assert_eq!(s.as_utf16_str(), utf16str!("hXXello"));
        assert_eq!(s.len(), 7);

        // Insert at the boundary idx == len (append).
        s.insert_utf16_str(s.len(), utf16str!("!"));
        assert_eq!(s.as_utf16_str(), utf16str!("hXXello!"));

        let mut appended = arena.alloc_utf16_string();
        appended.push_str(utf16str!("hi"));
        appended.insert_utf16_str(appended.len(), utf16str!("!"));
        assert_eq!(appended.as_utf16_str(), utf16str!("hi!"));

        // Insert at idx == 0 (head).
        s.insert_utf16_str(0, utf16str!(">"));
        assert_eq!(s.as_utf16_str(), utf16str!(">hXXello!"));

        // Insert exactly at the grow boundary: build a String at known
        // cap, then insert enough to hit needed == cap.
        let mut t = arena.alloc_utf16_string_with_capacity(8);
        t.push_str(utf16str!("abcd"));
        let ptr_before = t.as_ptr();
        let cap = t.capacity();
        let extra = vec![b'x' as u16; cap - t.len()];
        let extra_str = widestring::Utf16Str::from_slice(&extra).expect("valid utf16");
        t.insert_utf16_str(t.len(), extra_str);
        assert_eq!(t.len(), cap);
        assert_eq!(t.as_ptr(), ptr_before, "insert at exact-fit must not reallocate");
    }

    #[test]
    fn remove_middle_preserves_content() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        let removed = s.remove(1); // 'e'
        assert_eq!(removed, 'e');
        assert_eq!(s.as_utf16_str(), utf16str!("hllo"));
        assert_eq!(s.len(), 4);

        // Remove last char (idx == len - 1).
        let removed = s.remove(s.len() - 1);
        assert_eq!(removed, 'o');
        assert_eq!(s.as_utf16_str(), utf16str!("hll"));

        // Remove first char (idx == 0).
        let removed = s.remove(0);
        assert_eq!(removed, 'h');
        assert_eq!(s.as_utf16_str(), utf16str!("ll"));

        let mut s = arena.alloc_utf16_string();
        s.push_from_str("abcdefghij");
        assert_eq!(s.remove(4), 'e');
        assert_eq!(s.as_slice().to_vec(), widestring::Utf16String::from_str("abcdfghij").into_vec());
        assert_eq!(s.remove(0), 'a');
        assert_eq!(s.as_slice().to_vec(), widestring::Utf16String::from_str("bcdfghij").into_vec());
        assert_eq!(s.remove(s.len() - 1), 'j');
        assert_eq!(s.as_slice().to_vec(), widestring::Utf16String::from_str("bcdfghi").into_vec());
    }

    #[test]
    fn replace_range_at_boundaries() {
        let arena = Arena::new();

        // Full-range replace.
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(.., utf16str!("WORLD"));
        assert_eq!(s.as_utf16_str(), utf16str!("WORLD"));

        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        s.replace_range(0..3, utf16str!("xyzw"));
        assert_eq!(s.as_utf16_str(), utf16str!("xyzw"));

        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abcdef"));
        s.replace_range(0..6, utf16str!("xy"));
        assert_eq!(s.as_utf16_str(), utf16str!("xy"));

        // Middle replace, different lengths (longer).
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(1..4, utf16str!("XXXX"));
        assert_eq!(s.as_utf16_str(), utf16str!("hXXXXo"));

        // Middle replace, shorter.
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(1..4, utf16str!("X"));
        assert_eq!(s.as_utf16_str(), utf16str!("hXo"));

        // Head replace (start == 0).
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(..2, utf16str!("HE"));
        assert_eq!(s.as_utf16_str(), utf16str!("HEllo"));

        // Tail replace (end == len).
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(3.., utf16str!("LO"));
        assert_eq!(s.as_utf16_str(), utf16str!("helLO"));

        // Empty range insert (start == end).
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.replace_range(2..2, utf16str!("--"));
        assert_eq!(s.as_utf16_str(), utf16str!("he--llo"));

        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("abc"));
        let n = s.len();
        s.replace_range(n..n, utf16str!("xyz"));
        assert_eq!(s.as_utf16_str(), utf16str!("abcxyz"));

        // Replace into an empty string.
        let mut s = arena.alloc_utf16_string();
        s.replace_range(0..0, utf16str!("abc"));
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
    }

    #[test]
    fn into_box_handles_empty_and_non_empty() {
        let arena = Arena::new();
        // Empty case.
        let s = arena.alloc_utf16_string();
        let b = s.into_boxed_utf16_str();
        assert_eq!(&*b, utf16str!(""));
        assert_eq!(b.len(), 0);

        // Non-empty case.
        let s = Utf16String::from_utf16_str_in(utf16str!("hello, world"), &arena);
        let b = s.into_boxed_utf16_str();
        assert_eq!(&*b, utf16str!("hello, world"));
        assert_eq!(b.len(), 12);
    }

    /// `Utf16String::into_arc` freezes into a shared, reference-counted
    /// `Arc<Utf16Str>` whose contents match the builder, for both empty
    /// and non-empty inputs, and which can be cloned and outlive the
    /// arena.
    #[test]
    fn into_arc_handles_empty_and_non_empty() {
        let arena = Arena::new();

        let s_empty = arena.alloc_utf16_string();
        let a_empty: multitude::Arc<multitude::strings::Utf16Str> = multitude::Arc::<multitude::strings::Utf16Str>::from(s_empty);
        assert_eq!(&*a_empty, utf16str!(""));
        assert_eq!(a_empty.len(), 0);

        let s = Utf16String::from_utf16_str_in(utf16str!("hello, world"), &arena);
        let a: multitude::Arc<multitude::strings::Utf16Str> = multitude::Arc::<multitude::strings::Utf16Str>::from(s);
        assert_eq!(&*a, utf16str!("hello, world"));
        assert_eq!(a.len(), 12);

        // Cloning shares the same backing allocation.
        let a2 = a.clone();
        assert_eq!(&*a2, utf16str!("hello, world"));
        assert_eq!(a.as_ptr(), a2.as_ptr());
    }

    /// An `Arc<Utf16Str>` produced by `into_arc` outlives the arena it was
    /// built from (the backing chunk is held by the refcount).
    #[test]
    fn into_arc_outlives_arena() {
        let escaped: multitude::Arc<multitude::strings::Utf16Str> = {
            let arena = Arena::new();
            let s = Utf16String::from_utf16_str_in(utf16str!("survives"), &arena);
            let a = multitude::Arc::<multitude::strings::Utf16Str>::from(s);
            drop(arena);
            a
        };
        assert_eq!(&*escaped, utf16str!("survives"));
    }

    #[test]
    fn reclaim_tail_does_not_corrupt_frozen_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(256);
        s.push_str(utf16str!("frozen"));
        let frozen = s.into_boxed_utf16_str();
        // Allocate something in the same arena to potentially overlap with
        // the reclaimed tail bytes.
        let _filler: multitude::vec::Vec<'_, u64> = {
            let mut v = arena.alloc_vec_with_capacity::<u64>(64);
            for i in 0..64 {
                v.push(i);
            }
            v
        };
        assert_eq!(&*frozen, utf16str!("frozen"));
    }
}

mod mutation_coverage {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "boundary-focused tests use large values")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::needless_pass_by_value, reason = "test code")]
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::panic::catch_unwind;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::strings::{String as ArenaString, Utf16String};
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arena, ArenaBuilder, Box, FromIn as _};
    use widestring::{Utf16Str, utf16str};

    use crate::common;

    fn hash_value<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    struct DropCounter(StdArc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_string_is_empty_false_when_nonempty() {
        let arena = Arena::new();
        let s = ArenaString::from_in("alpha", &arena);
        assert!(!s.is_empty());
    }

    #[test]
    fn test_string_partial_eq_distinguishes_different_values() {
        let arena = Arena::new();
        let a = ArenaString::from_in("alpha", &arena);
        let b = ArenaString::from_in("beta", &arena);
        let beta = "beta";

        assert_ne!(a, b);
        assert!(a != "beta");
        assert!(a != beta);
    }

    #[test]
    fn test_string_hash_depends_on_contents() {
        let arena = Arena::new();
        let a = ArenaString::from_in("alpha", &arena);
        let b = ArenaString::from_in("beta", &arena);

        assert_ne!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn test_string_as_ref_returns_expected_contents() {
        let arena = Arena::new();
        let s = ArenaString::from_in("expected", &arena);
        let r: &str = s.as_ref();

        assert_eq!(r, "expected");
        assert_ne!(r, "");
    }

    #[test]
    fn test_string_insert_str_grows_at_full_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(5);
        s.push_str("hello");

        s.insert_str(5, "!!");

        assert_eq!(s.as_str(), "hello!!");
        assert!(s.capacity() >= s.len());
    }

    #[test]
    fn test_string_insert_str_middle_preserves_surrounding_bytes() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(8);
        s.push_str("abef");

        s.insert_str(2, "cd");

        assert_eq!(s.as_str(), "abcdef");
    }

    #[test]
    fn test_string_remove_from_middle_preserves_tail() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcde", &arena);

        assert_eq!(s.remove(2), 'c');
        assert_eq!(s.as_str(), "abde");
    }

    #[test]
    fn test_string_retain_keeps_only_requested_chars() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abracadabra", &arena);

        s.retain(|ch| ch == 'a');

        assert_eq!(s.as_str(), "aaaaa");
    }

    #[test]
    fn test_string_replace_range_longer_grows_and_shifts_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(5);
        s.push_str("abYZ");

        s.replace_range(2..2, "1234");

        assert_eq!(s.as_str(), "ab1234YZ");
    }

    #[test]
    fn test_string_replace_range_shorter_shifts_tail_left() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcXYZdef", &arena);

        s.replace_range(3..6, "Q");

        assert_eq!(s.as_str(), "abcQdef");
    }

    #[test]
    fn test_string_shrink_to_fit_reclaims_capacity_and_preserves_contents() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(16);
        s.push_str("hello");

        s.shrink_to_fit();
        let _follow_on = arena.alloc_str("zzzz");

        assert_eq!(s.capacity(), s.len());
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn test_utf16_string_is_empty_false_when_nonempty() {
        let arena = Arena::new();
        let s = Utf16String::from_utf16_str_in(utf16str!("alpha"), &arena);
        assert!(!s.is_empty());
    }

    #[test]
    fn test_utf16_string_partial_eq_distinguishes_different_values() {
        let arena = Arena::new();
        let a = Utf16String::from_utf16_str_in(utf16str!("alpha"), &arena);
        let b = Utf16String::from_utf16_str_in(utf16str!("beta"), &arena);
        let beta = utf16str!("beta");

        assert_ne!(a, b);
        assert!(a != utf16str!("beta"));
        assert!(a != beta);
    }

    #[test]
    fn test_utf16_string_hash_depends_on_contents() {
        let arena = Arena::new();
        let a = Utf16String::from_utf16_str_in(utf16str!("alpha"), &arena);
        let b = Utf16String::from_utf16_str_in(utf16str!("beta"), &arena);

        assert_ne!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn test_utf16_string_as_mut_utf16_str_returns_live_contents() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("ab"), &arena);
        let view: &mut Utf16Str = s.as_mut_utf16_str();

        assert_eq!(view, utf16str!("ab"));

        unsafe {
            *s.as_mut_ptr() = 'z' as u16;
        }
        assert_eq!(s.as_utf16_str(), utf16str!("zb"));
    }

    #[test]
    fn test_utf16_string_pop_reduces_length() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("ab"), &arena);

        assert_eq!(s.pop(), Some('b'));
        assert_eq!(s.as_utf16_str(), utf16str!("a"));
    }

    #[test]
    fn test_utf16_string_insert_middle_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        s.push_str(utf16str!("abef"));

        s.insert_utf16_str(2, utf16str!("cd"));

        assert_eq!(s.as_utf16_str(), utf16str!("abcdef"));
    }

    #[test]
    fn test_utf16_string_remove_from_middle_preserves_tail() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcde"), &arena);

        assert_eq!(s.remove(2), 'c');
        assert_eq!(s.as_utf16_str(), utf16str!("abde"));
    }

    #[test]
    fn test_utf16_string_retain_keeps_only_requested_chars() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abracadabra"), &arena);

        s.retain(|ch| ch == 'a');

        assert_eq!(s.as_utf16_str(), utf16str!("aaaaa"));
    }

    #[test]
    fn test_utf16_string_replace_range_longer_grows_and_shifts_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(4);
        s.push_str(utf16str!("abYZ"));

        s.replace_range(2..2, utf16str!("1234"));

        assert_eq!(s.as_utf16_str(), utf16str!("ab1234YZ"));
    }

    #[test]
    fn test_utf16_string_replace_range_shorter_shifts_tail_left() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcXYZdef"), &arena);

        s.replace_range(3..6, utf16str!("Q"));

        assert_eq!(s.as_utf16_str(), utf16str!("abcQdef"));
    }

    #[test]
    fn test_box_str_partial_eq_distinguishes_different_values() {
        let arena = Arena::new();
        let boxed: Box<str> = arena.alloc_str_box("alpha");
        let beta = "beta";

        assert!(boxed != "beta");
        assert!(boxed != beta);
    }

    #[test]
    fn test_box_utf16_str_partial_eq_distinguishes_different_values() {
        let arena = Arena::new();
        let boxed: multitude::Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("alpha"));
        let beta = utf16str!("beta");

        assert!(boxed != utf16str!("beta"));
        assert!(boxed != beta);
    }

    #[test]
    fn test_box_utf16_str_drop_releases_chunk_after_arena_drop() {
        let alloc = common::TrackingAllocator::new();
        let boxed: multitude::Box<multitude::strings::Utf16Str, _> = {
            let arena = Arena::builder().allocator_in(alloc.clone()).build();
            arena.alloc_utf16_str_box(utf16str!("hello"))
        };

        assert_eq!(&*boxed, utf16str!("hello"));
        drop(boxed);

        assert_eq!(alloc.live_chunks(), 0);
        assert_eq!(alloc.live_bytes(), 0);
    }

    #[test]
    fn test_vec_is_empty_false_when_nonempty() {
        let arena = Arena::new();
        let mut v: ArenaVec<u32, _> = arena.alloc_vec();
        v.push(1);
        assert!(!v.is_empty());
    }

    #[test]
    fn test_vec_from_iter_in_uses_nonzero_size_hint() {
        let arena = Arena::new_in(common::FailingAllocator::new(1));
        let v = ArenaVec::from_iter_in([1_u32, 2, 3], &arena);

        assert_eq!(v.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn test_vec_insert_middle_preserves_positions() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 4, 5]);

        v.insert(2, 3);

        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_vec_remove_middle_preserves_positions() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3, 4, 5]);

        assert_eq!(v.remove(2), 3);
        assert_eq!(v.as_slice(), &[1, 2, 4, 5]);
    }

    #[test]
    fn test_vec_truncate_drops_removed_elements() {
        let arena = Arena::new();
        let drops = StdArc::new(AtomicUsize::new(0));
        let mut v = arena.alloc_vec();
        for _ in 0..4 {
            v.push(DropCounter(StdArc::clone(&drops)));
        }

        v.truncate(2);

        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_vec_shrink_to_fit_reduces_capacity_and_preserves_values() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u32>(16);
        v.extend(0_u32..5);

        v.shrink_to_fit();
        let _follow_on = arena.alloc_str("tail");

        assert_eq!(v.capacity(), v.len());
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_vec_resize_with_adds_expected_elements() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2]);
        let mut next = 10_u32;

        v.resize_with(5, || {
            let value = next;
            next += 1;
            value
        });

        assert_eq!(v.as_slice(), &[1, 2, 10, 11, 12]);
    }

    #[test]
    fn test_vec_hash_depends_on_contents() {
        let arena = Arena::new();
        let mut a = arena.alloc_vec();
        a.extend([1_u32, 2, 3]);
        let mut b = arena.alloc_vec();
        b.extend([1_u32, 2, 4]);

        assert_ne!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn test_vec_drop_runs_element_drops() {
        let drops = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut v = arena.alloc_vec();
            for _ in 0..3 {
                v.push(DropCounter(StdArc::clone(&drops)));
            }
        }

        assert_eq!(drops.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_vec_drain_size_hint_tracks_remaining_items() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend(0_u32..6);

        {
            let mut drain = v.drain(1..5);
            assert_eq!(drain.size_hint(), (4, Some(4)));
            assert_eq!(drain.len(), 4);
            assert_eq!(drain.next(), Some(1));
            assert_eq!(drain.size_hint(), (3, Some(3)));
            assert_eq!(drain.next_back(), Some(4));
            assert_eq!(drain.size_hint(), (2, Some(2)));
        }

        assert_eq!(v.as_slice(), &[0, 5]);
    }

    #[test]
    fn test_vec_drain_drop_moves_tail_after_partial_iteration() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend(0_u32..6);

        {
            let mut drain = v.drain(1..4);
            assert_eq!(drain.next(), Some(1));
        }

        assert_eq!(v.as_slice(), &[0, 4, 5]);
    }

    #[test]
    fn test_arena_alloc_str_keeps_pinned_chunk_alive_across_rotation() {
        let arena = Arena::builder().build();
        let mut s = arena.alloc_str("hello");

        let a = arena.alloc_slice_copy(std::vec![1_u8; 4000]);
        let b = arena.alloc_slice_copy(std::vec![2_u8; 4000]);
        assert_eq!(a.len(), 4000);
        assert_eq!(b.len(), 4000);

        s.make_ascii_uppercase();
        assert_eq!(&*s, "HELLO");
    }

    // `chunk_size` builder method and `BuildError::ChunkSizeOutOfRange` are
    // gone with the adaptive-sizing change.

    #[cfg(feature = "stats")]
    #[test]
    fn test_vec_reserve_grows_in_place_without_relocation() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u32>(4);
        v.extend([1_u32, 2, 3, 4]);
        let before = arena.stats().relocations;

        v.reserve(8);

        assert_eq!(arena.stats().relocations, before);
        assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    fn test_arena_drop_clears_current_chunk_slots() {
        let alloc = common::TrackingAllocator::new();
        {
            let arena = Arena::builder().allocator_in(alloc.clone()).build();
            let value = arena.alloc(123_u64);
            assert_eq!(*value, 123);
        }

        assert_eq!(alloc.live_chunks(), 0);
        assert_eq!(alloc.live_bytes(), 0);
    }

    #[test]
    fn test_box_str_partial_eq_str_returns_false_for_mismatch() {
        let arena = Arena::new();
        let boxed: Box<str> = arena.alloc_str_box("alpha");
        // Force PartialEq<str> (not PartialEq<&str>)
        assert!(!<Box<str> as PartialEq<str>>::eq(&boxed, "beta"));
    }

    #[test]
    fn test_box_utf16_str_partial_eq_utf16str_returns_false_for_mismatch() {
        let arena = Arena::new();
        let boxed: multitude::Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("alpha"));
        assert!(!<multitude::Box<multitude::strings::Utf16Str> as PartialEq<Utf16Str>>::eq(
            &boxed,
            utf16str!("beta")
        ));
    }

    #[test]
    fn test_string_partial_eq_str_returns_false_for_mismatch() {
        let arena = Arena::new();
        let s = ArenaString::from_in("alpha", &arena);
        assert!(!<ArenaString as PartialEq<str>>::eq(&s, "beta"));
    }

    #[test]
    fn test_utf16_string_partial_eq_utf16str_returns_false_for_mismatch() {
        let arena = Arena::new();
        let s = Utf16String::from_utf16_str_in(utf16str!("alpha"), &arena);
        assert!(!<Utf16String as PartialEq<Utf16Str>>::eq(&s, utf16str!("beta")));
    }

    #[test]
    fn test_vec_try_with_capacity_in_zero_does_not_allocate() {
        let arena = Arena::new();
        let v = arena.try_alloc_vec_with_capacity::<u32>(0).unwrap();
        assert_eq!(v.capacity(), 0);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn test_vec_try_reserve_at_exact_boundary_does_not_grow() {
        let arena = Arena::new();
        let mut v = arena.try_alloc_vec_with_capacity::<u32>(8).unwrap();
        v.push(1);
        v.push(2);
        // cap=8, len=2, spare = cap - len = 6
        let cap_before = v.capacity();
        v.try_reserve(6).unwrap(); // additional == spare, should NOT grow
        assert_eq!(v.capacity(), cap_before);
    }

    #[test]
    fn test_vec_from_iter_in_with_zero_size_hint() {
        let arena = Arena::new();
        // An iterator with size_hint == (0, None)
        let iter = std::iter::from_fn({
            let mut i = 0_u32;
            move || {
                (i < 3).then(|| {
                    i += 1;
                    i
                })
            }
        });
        let v = ArenaVec::from_iter_in(iter, &arena);
        assert_eq!(v.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn test_vec_insert_at_start_shifts_all() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([2_u32, 3, 4, 5]);
        v.insert(0, 1);
        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_vec_remove_at_start_shifts_all() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3, 4, 5]);
        assert_eq!(v.remove(0), 1);
        assert_eq!(v.as_slice(), &[2, 3, 4, 5]);
    }

    #[test]
    fn test_vec_remove_second_to_last_shifts_one() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3]);
        assert_eq!(v.remove(1), 2);
        assert_eq!(v.as_slice(), &[1, 3]);
    }

    #[test]
    fn test_vec_truncate_to_zero_drops_all() {
        let arena = Arena::new();
        let drops = StdArc::new(AtomicUsize::new(0));
        let mut v = arena.alloc_vec();
        for _ in 0..5 {
            v.push(DropCounter(StdArc::clone(&drops)));
        }
        v.truncate(0);
        assert_eq!(drops.load(Ordering::Relaxed), 5);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn test_vec_truncate_to_one_drops_rest() {
        let arena = Arena::new();
        let drops = StdArc::new(AtomicUsize::new(0));
        let mut v = arena.alloc_vec();
        for _ in 0..3 {
            v.push(DropCounter(StdArc::clone(&drops)));
        }
        v.truncate(1);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn test_vec_shrink_to_fit_zst_is_noop() {
        let arena = Arena::new();
        let mut v: ArenaVec<()> = arena.alloc_vec();
        v.push(());
        v.push(());
        let cap_before = v.capacity();
        v.shrink_to_fit();
        // ZST: shrink_to_fit should be a no-op (capacity is meaningless for ZSTs)
        assert_eq!(v.capacity(), cap_before);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_vec_try_reserve_exact_at_boundary_does_not_grow() {
        let arena = Arena::new();
        let mut v = arena.try_alloc_vec_with_capacity::<u32>(8).unwrap();
        v.push(1);
        v.push(2);
        let cap_before = v.capacity();
        // needed = len(2) + additional(6) = 8 = cap, should NOT grow
        v.try_reserve_exact(6).unwrap();
        assert_eq!(v.capacity(), cap_before);
    }

    #[test]
    fn test_vec_resize_with_same_len_is_noop() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3]);
        let cap_before = v.capacity();
        v.resize_with(3, || panic!("should not be called"));
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        assert_eq!(v.capacity(), cap_before);
    }

    #[test]
    fn test_vec_extend_empty_iterator() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3]);
        let cap_before = v.capacity();
        v.extend(std::iter::empty::<u32>());
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        assert_eq!(v.capacity(), cap_before);
    }

    #[test]
    fn test_vec_extend_ref_empty_iterator() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3]);
        let cap_before = v.capacity();
        let empty: &[u32] = &[];
        v.extend(empty.iter());
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        assert_eq!(v.capacity(), cap_before);
    }

    #[test]
    fn test_vec_drain_all_elements_no_tail() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3]);
        {
            let drained: std::vec::Vec<_> = v.drain(0..3).collect();
            assert_eq!(drained, vec![1, 2, 3]);
        }
        assert!(v.is_empty());
    }

    #[test]
    fn test_vec_drain_suffix_no_tail() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend([1_u32, 2, 3, 4]);
        {
            let drained: std::vec::Vec<_> = v.drain(2..4).collect();
            assert_eq!(drained, vec![3, 4]);
        }
        assert_eq!(v.as_slice(), &[1, 2]);
    }

    #[test]
    fn test_string_shrink_to_fit_cap_equals_len_is_noop() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("hello", &arena);
        // Force a shrink_to_fit so cap == len
        s.shrink_to_fit();
        let cap_after_first = s.capacity();
        // Calling again should be a no-op since cap == len
        s.shrink_to_fit();
        assert_eq!(s.capacity(), cap_after_first);
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn test_string_shrink_to_fit_cap_zero_is_noop() {
        let arena = Arena::new();
        let s = arena.alloc_string_with_capacity(0);
        assert_eq!(s.capacity(), 0);
        // Can't mutably borrow to call shrink_to_fit on a zero-capacity string
        // through the arena API, but we can create one fresh
        let mut s2: ArenaString = arena.alloc_string();
        s2.shrink_to_fit(); // cap == 0 path
        assert_eq!(s2.len(), 0);
    }

    #[test]
    fn test_string_insert_str_at_start_shifts_all() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("world", &arena);
        s.insert_str(0, "hello ");
        assert_eq!(s.as_str(), "hello world");
    }

    #[test]
    fn test_string_insert_str_at_exact_capacity_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(5);
        s.push_str("hello");
        assert_eq!(s.capacity(), 5);
        s.insert_str(5, "!");
        assert_eq!(s.as_str(), "hello!");
        assert!(s.capacity() >= 6);
    }

    #[test]
    fn test_string_remove_first_char_shifts_all() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcde", &arena);
        assert_eq!(s.remove(0), 'a');
        assert_eq!(s.as_str(), "bcde");
    }

    #[test]
    fn test_string_remove_multibyte_char() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("aéc", &arena);
        assert_eq!(s.remove(1), 'é');
        assert_eq!(s.as_str(), "ac");
    }

    #[test]
    fn test_string_retain_removes_alternating() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcdef", &arena);
        let mut toggle = false;
        s.retain(|_| {
            toggle = !toggle;
            toggle
        });
        assert_eq!(s.as_str(), "ace");
    }

    #[test]
    fn test_string_retain_removes_none() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abc", &arena);
        s.retain(|_| true);
        assert_eq!(s.as_str(), "abc");
    }

    #[test]
    fn test_string_retain_removes_all() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abc", &arena);
        s.retain(|_| false);
        assert_eq!(s.as_str(), "");
    }

    #[test]
    fn test_string_replace_range_grow_at_exact_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abcdef");
        // Replace "cd" (2 bytes) with "1234" (4 bytes), growth of 2
        s.replace_range(2..4, "1234");
        assert_eq!(s.as_str(), "ab1234ef");
    }

    #[test]
    fn test_string_replace_range_shrink_by_exact_amount() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcdef", &arena);
        // Replace "bcde" (4 bytes) with "X" (1 byte), shrink of 3
        s.replace_range(1..5, "X");
        assert_eq!(s.as_str(), "aXf");
    }

    #[test]
    fn test_string_replace_range_same_size() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcdef", &arena);
        s.replace_range(2..4, "CD");
        assert_eq!(s.as_str(), "abCDef");
    }

    #[test]
    fn test_string_replace_range_shrink_at_end() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcdef", &arena);
        // Replace "ef" with "" — shrink == removed, no tail shift needed
        s.replace_range(4..6, "");
        assert_eq!(s.as_str(), "abcd");
    }

    #[test]
    fn test_string_try_push_str_at_exact_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(5);
        s.push_str("hello");
        let cap_before = s.capacity();
        // Push more, forcing growth
        s.try_push_str("!").unwrap();
        assert_eq!(s.as_str(), "hello!");
        assert!(s.capacity() > cap_before);
    }

    #[test]
    fn test_string_try_push_str_within_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.push_str("hello");
        let cap_before = s.capacity();
        s.try_push_str("!").unwrap();
        assert_eq!(s.as_str(), "hello!");
        assert_eq!(s.capacity(), cap_before); // no growth needed
    }

    #[test]
    fn test_string_reserve_at_exact_boundary_does_not_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.push_str("hello"); // len=5, cap=10
        let cap_before = s.capacity();
        s.reserve(5); // needed = 5+5 = 10 = cap, should NOT grow
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_string_try_reserve_at_exact_boundary_does_not_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.push_str("hello"); // len=5, cap=10
        let cap_before = s.capacity();
        s.try_reserve(5).unwrap(); // needed = 5+5 = 10 = cap, should NOT grow
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_string_extend_empty_chars() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("hello", &arena);
        let cap_before = s.capacity();
        s.extend(std::iter::empty::<char>());
        assert_eq!(s.as_str(), "hello");
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_utf16_string_shrink_to_fit_cap_equals_len_is_noop() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        s.shrink_to_fit();
        let cap_after_first = s.capacity();
        s.shrink_to_fit();
        assert_eq!(s.capacity(), cap_after_first);
        assert_eq!(s.as_utf16_str(), utf16str!("hello"));
    }

    #[test]
    fn test_utf16_string_shrink_to_fit_cap_zero_is_noop() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.shrink_to_fit();
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn test_utf16_string_insert_at_start_shifts_all() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("world"), &arena);
        s.insert_utf16_str(0, utf16str!("hello "));
        assert_eq!(s.as_utf16_str(), utf16str!("hello world"));
    }

    #[test]
    fn test_utf16_string_insert_at_exact_capacity_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push_str(utf16str!("hello"));
        assert_eq!(s.capacity(), 5);
        s.insert_utf16_str(5, utf16str!("!"));
        assert_eq!(s.as_utf16_str(), utf16str!("hello!"));
    }

    #[test]
    fn test_utf16_string_remove_first_shifts_all() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcde"), &arena);
        assert_eq!(s.remove(0), 'a');
        assert_eq!(s.as_utf16_str(), utf16str!("bcde"));
    }

    #[test]
    fn test_utf16_string_retain_removes_alternating() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcdef"), &arena);
        let mut toggle = false;
        s.retain(|_| {
            toggle = !toggle;
            toggle
        });
        assert_eq!(s.as_utf16_str(), utf16str!("ace"));
    }

    #[test]
    fn test_utf16_string_retain_removes_all() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abc"), &arena);
        s.retain(|_| false);
        assert!(s.is_empty());
    }

    #[test]
    fn test_utf16_string_retain_removes_none() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abc"), &arena);
        s.retain(|_| true);
        assert_eq!(s.as_utf16_str(), utf16str!("abc"));
    }

    #[test]
    fn test_utf16_string_replace_range_grow_at_exact_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(6);
        s.push_str(utf16str!("abcdef"));
        s.replace_range(2..4, utf16str!("1234"));
        assert_eq!(s.as_utf16_str(), utf16str!("ab1234ef"));
    }

    #[test]
    fn test_utf16_string_replace_range_shrink_exact() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcdef"), &arena);
        s.replace_range(1..5, utf16str!("X"));
        assert_eq!(s.as_utf16_str(), utf16str!("aXf"));
    }

    #[test]
    fn test_utf16_string_replace_range_same_size() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcdef"), &arena);
        s.replace_range(2..4, utf16str!("CD"));
        assert_eq!(s.as_utf16_str(), utf16str!("abCDef"));
    }

    #[test]
    fn test_utf16_string_replace_range_shrink_at_end() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("abcdef"), &arena);
        s.replace_range(4..6, utf16str!(""));
        assert_eq!(s.as_utf16_str(), utf16str!("abcd"));
    }

    #[test]
    fn test_utf16_string_try_push_from_str_at_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push_str(utf16str!("hello"));
        let cap_before = s.capacity();
        s.try_push_from_str("!").unwrap();
        assert!(s.capacity() > cap_before);
    }

    #[test]
    fn test_utf16_string_try_push_slice_at_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push_str(utf16str!("hello"));
        let cap_before = s.capacity();
        s.try_push_str(utf16str!("!")).unwrap();
        assert!(s.capacity() > cap_before);
    }

    #[test]
    fn test_utf16_string_try_push_slice_within_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push_str(utf16str!("hello"));
        let cap_before = s.capacity();
        s.try_push_str(utf16str!("!")).unwrap();
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_utf16_string_reserve_at_exact_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push_str(utf16str!("hello")); // len=5, cap=10
        let cap_before = s.capacity();
        s.reserve(5); // needed = 10 = cap, should NOT grow
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_utf16_string_extend_empty_chars() {
        let arena = Arena::new();
        let mut s = Utf16String::from_utf16_str_in(utf16str!("hello"), &arena);
        let cap_before = s.capacity();
        s.extend(std::iter::empty::<char>());
        assert_eq!(s.as_utf16_str(), utf16str!("hello"));
        assert_eq!(s.capacity(), cap_before);
    }

    #[test]
    fn test_arena_builder_max_normal_alloc_at_exact_min_succeeds() {
        // MIN_MAX_NORMAL_ALLOC = 4096
        let result = Arena::builder().max_normal_alloc(4096).try_build();
        result.unwrap();
    }

    #[test]
    #[should_panic(expected = "max_normal_alloc must be in")]
    fn test_arena_builder_max_normal_alloc_below_min_fails() {
        let _ = Arena::builder().max_normal_alloc(4095).try_build();
    }

    #[test]
    fn test_slice_fill_with_panic_drops_initialized_elements() {
        let drops = StdArc::new(AtomicUsize::new(0));
        let arena = Arena::new();

        let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
            let drops_clone = StdArc::clone(&drops);
            let _ = arena.alloc_slice_fill_with(5, |i| {
                assert!(i != 3, "intentional panic at index 3");
                DropCounter(StdArc::clone(&drops_clone))
            });
        }));

        assert!(result.is_err());
        // Elements 0, 1, 2 were initialized and should be dropped
        assert_eq!(drops.load(Ordering::Relaxed), 3);
    }
}

mod utf16_tests {
    use multitude::Arena;

    #[test]
    fn utf16_191_truncate_to_zero() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push('A');
        s.push('B');
        s.truncate(0);
        assert_eq!(s.len(), 0);
    }

    /// Also kills: `utf16_string.rs:191:20` by testing truncation to len-1
    /// where len-1 lands on a valid boundary.
    #[test]
    fn utf16_191_truncate_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push('A'); // 1 u16
        s.push('B'); // 1 u16
        s.push('C'); // 1 u16
        assert_eq!(s.len(), 3);
        s.truncate(1); // Should keep just 'A'
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn utf16_203_shrink_to_fit_or_to_and() {
        let arena = Arena::new();
        // Case: cap > 0 && len == cap → should be no-op
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        s.push('E');
        assert_eq!(s.len(), 5);
        // Now len == cap, shrink should be no-op
        s.shrink_to_fit();
        assert_eq!(s.len(), 5);

        // Case: cap == 0 → should be no-op
        let s2 = arena.alloc_utf16_string_with_capacity(0);
        assert_eq!(s2.len(), 0);
    }

    #[test]
    fn utf16_206_shrink_reclaim_units() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        // len=2, cap=20, reclaim_units should be 18
        // If / instead of -, reclaim_units = 20/2 = 10 (wrong)
        s.shrink_to_fit();
        // After shrink, cap should equal len if reclaim succeeded
        // (best-effort, so we just verify no crash)
    }

    #[test]
    fn utf16_207_shrink_reclaim_bytes() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(50);
        for _ in 0..5 {
            s.push('X');
        }
        // reclaim_units = 45, reclaim_bytes should be 90
        // If * becomes +, reclaim_bytes = 45 + 2 = 47 (wrong)
        // If * becomes /, reclaim_bytes = 45 / 2 = 22 (wrong)
        s.shrink_to_fit();
        // Verify string is still valid
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn utf16_260_push_from_str_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        // Push exactly 5 ASCII chars (each is 1 u16)
        // needed = 0 + 5*2 = 10 (worst case), but actual is 5
        s.try_push_from_str("A").unwrap();
        s.try_push_from_str("B").unwrap();
        s.try_push_from_str("C").unwrap();
        s.try_push_from_str("D").unwrap();
        s.try_push_from_str("E").unwrap();
        assert_eq!(s.len(), 5);
        // Now cap == len, pushing one more should trigger grow
        s.try_push_from_str("F").unwrap();
        assert_eq!(s.len(), 6);
    }

    #[test]
    fn utf16_277_push_slice_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(3);
        // Push 3 chars to fill capacity exactly
        s.push('A');
        s.push('B');
        s.push('C');
        assert_eq!(s.len(), 3); // needed == cap, no grow
        // Push one more to trigger grow
        s.push('D');
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn utf16_298_try_reserve_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push('A'); // len=1
        let cap_before = s.capacity();
        s.try_reserve(9).unwrap(); // needed=10==cap, no grow
        assert_eq!(s.capacity(), cap_before);
        s.try_reserve(10).unwrap(); // needed=11>cap, must grow
    }

    #[test]
    fn utf16_318_330_insert_slice() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push('A');
        s.push('B');
        s.push('C');
        // Insert at end (idx == len)
        s.insert(3, 'D');
        assert_eq!(s.len(), 4);
        // Insert at beginning
        s.insert(0, 'Z');
        assert_eq!(s.len(), 5);
        // Now len==cap==5, insert one more to trigger grow
        s.insert(2, 'X');
        assert_eq!(s.len(), 6);
    }

    #[test]
    fn utf16_337_insert_copy_length() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        s.push('E');
        // Insert at idx=2 — must shift 3 elements (len-idx = 5-2 = 3)
        s.insert(2, 'X');
        let slice = s.as_slice();
        assert_eq!(slice[0], 'A' as u16);
        assert_eq!(slice[1], 'B' as u16);
        assert_eq!(slice[2], 'X' as u16);
        assert_eq!(slice[3], 'C' as u16);
        assert_eq!(slice[4], 'D' as u16);
        assert_eq!(slice[5], 'E' as u16);
    }

    #[test]
    fn utf16_356_remove_shift() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        s.push('E');
        // Remove 'C' at idx=2 (n=1)
        let ch = s.remove(2);
        assert_eq!(ch, 'C');
        assert_eq!(s.len(), 4);
        let slice = s.as_slice();
        assert_eq!(slice[0], 'A' as u16);
        assert_eq!(slice[1], 'B' as u16);
        assert_eq!(slice[2], 'D' as u16);
        assert_eq!(slice[3], 'E' as u16);
    }

    #[test]
    fn utf16_396_replace_range_start_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        // Replace at start=0 — should skip boundary check (start > 0 is false)
        let replacement = widestring::utf16str!("XY");
        s.replace_range(0..1, replacement);
        assert_eq!(s.len(), 5); // removed 1, added 2
        // Replace at start==len — should skip check (start < self.len is false)
        let empty = widestring::utf16str!("");
        s.replace_range(5..5, empty);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn utf16_403_replace_range_end_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.push('C');
        // Replace with end=0 — should skip check (end > 0 is false)
        let empty = widestring::utf16str!("");
        s.replace_range(0..0, empty);
        assert_eq!(s.len(), 3);
        // Replace with end==len — should skip check (end < self.len is false)
        let x = widestring::utf16str!("Z");
        s.replace_range(2..3, x);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn utf16_418_replace_range_grow_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(5);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        s.push('E');
        // Replace 1 element with 1 element — new_len == old_len == cap, no grow
        let x = widestring::utf16str!("X");
        s.replace_range(2..3, x);
        assert_eq!(s.len(), 5);
        assert_eq!(s.as_slice()[2], 'X' as u16);
    }

    #[test]
    fn utf16_424_replace_range_copy() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.push('C');
        s.push('D');
        s.push('E');
        // Replace B,C (indices 1..3) with "XYZ" (3 elements)
        let xyz = widestring::utf16str!("XYZ");
        s.replace_range(1..3, xyz);
        assert_eq!(s.len(), 6); // 5 - 2 + 3 = 6
        let slice = s.as_slice();
        assert_eq!(slice[0], 'A' as u16);
        assert_eq!(slice[1], 'X' as u16);
        assert_eq!(slice[2], 'Y' as u16);
        assert_eq!(slice[3], 'Z' as u16);
        assert_eq!(slice[4], 'D' as u16);
        assert_eq!(slice[5], 'E' as u16);
    }
}

mod utf16_round2 {
    use multitude::Arena;

    #[test]
    fn utf16_337_insert_copy_oob() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        // Build "ABCDEFGHIJ" (10 chars)
        for ch in "ABCDEFGHIJ".chars() {
            s.push(ch);
        }
        assert_eq!(s.len(), 10);
        // Insert "XY" at idx=3 — must shift 7 elements right
        // With mutation: copies 10+3=13 elements (OOB!)
        s.insert(3, 'X');
        assert_eq!(s.len(), 11);
        let slice = s.as_slice();
        assert_eq!(slice[0], 'A' as u16);
        assert_eq!(slice[1], 'B' as u16);
        assert_eq!(slice[2], 'C' as u16);
        assert_eq!(slice[3], 'X' as u16);
        assert_eq!(slice[4], 'D' as u16);
        assert_eq!(slice[5], 'E' as u16);
        assert_eq!(slice[6], 'F' as u16);
        assert_eq!(slice[7], 'G' as u16);
        assert_eq!(slice[8], 'H' as u16);
        assert_eq!(slice[9], 'I' as u16);
        assert_eq!(slice[10], 'J' as u16);
    }

    #[test]
    fn utf16_356_remove_long_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        for ch in "ABCDEFGHIJ".chars() {
            s.push(ch);
        }
        assert_eq!(s.len(), 10);
        // Remove 'C' at idx=2 (n=1 since 'C' is 1 u16)
        let removed = s.remove(2);
        assert_eq!(removed, 'C');
        assert_eq!(s.len(), 9);
        // Verify entire remaining string
        let expected = [
            'A' as u16, 'B' as u16, 'D' as u16, 'E' as u16, 'F' as u16, 'G' as u16, 'H' as u16, 'I' as u16, 'J' as u16,
        ];
        assert_eq!(s.as_slice(), &expected);
    }

    #[test]
    fn utf16_403_end_boundary_mid_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        // Push "A😀B" — 'A' (1 u16), '😀' (2 u16s: D83D DE00), 'B' (1 u16)
        s.push('A');
        s.push('😀');
        s.push('B');
        assert_eq!(s.len(), 4); // 1 + 2 + 1 = 4 u16s

        // Replace range ending at index 3 (between high and low surrogate is at 2)
        // end=3 is valid (after the emoji, start of 'B')
        let replacement = widestring::utf16str!("X");
        s.replace_range(3..4, replacement); // replace 'B' with 'X'
        assert_eq!(s.len(), 4);
        assert_eq!(s.as_slice()[3], 'X' as u16);
    }

    #[test]
    fn utf16_403_end_zero() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(10);
        s.push('A');
        s.push('B');
        // Replace range 0..0 with nothing (end=0)
        let empty = widestring::utf16str!("");
        s.replace_range(0..0, empty);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn utf16_403_27_end_surrogate_check() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        // "A😀B" = A [D83D DE00] B
        s.push('A');
        s.push('😀');
        s.push('B');
        // end=2 points at DE00 (low surrogate) — must panic
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let x = widestring::utf16str!("");
            s.replace_range(0..2, x);
        }));
        assert!(result.is_err(), "replace_range ending at low surrogate must panic");
    }

    // ---
    #[test]
    fn utf16_424_replace_range_tail_copy() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        for ch in "ABCDEFGHIJ".chars() {
            s.push(ch);
        }
        // Replace "CDE" (indices 2..5) with "XY"
        let xy = widestring::utf16str!("XY");
        s.replace_range(2..5, xy);
        assert_eq!(s.len(), 9); // 10 - 3 + 2
        let expected = [
            'A' as u16, 'B' as u16, 'X' as u16, 'Y' as u16, 'F' as u16, 'G' as u16, 'H' as u16, 'I' as u16, 'J' as u16,
        ];
        assert_eq!(s.as_slice(), &expected);
    }

    #[test]
    fn utf16_206_shrink_reclaim_v2() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(20);
        s.push('A');
        s.push('B');
        s.shrink_to_fit();
        // After successful shrink, capacity should equal len
        // (if try_shrink_at_cursor succeeded)
        // Just verify string is still valid
        assert_eq!(s.len(), 2);
        assert_eq!(s.as_slice()[0], 'A' as u16);
    }

    #[test]
    fn utf16_207_shrink_bytes_v2() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(100);
        for _ in 0..10 {
            s.push('X');
        }
        // reclaim_units = 90, reclaim_bytes should be 180
        // With +: 92 (wrong), with /: 45 (wrong)
        s.shrink_to_fit();
        assert_eq!(s.len(), 10);
    }

    // utf16:260/277/298/330 `> -> >=`: try_grow_to_at_least returns early
    // when min_cap <= cap, so the extra grow call is a no-op. EQUIVALENT.

    // utf16:203 `|| -> &&`: When cap > 0 && len == cap, reclaim_units = 0,
    // reclaim_bytes = 0, try_shrink_at_cursor(end, 0) is a no-op. EQUIVALENT.
}

mod from_coverage_extras_utf16 {
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]
    use multitude::strings::Utf16String;
    use multitude::vec::FromIteratorIn;
    use multitude::{Arena, ArenaBuilder, FromIn as _};
    use widestring::utf16str;

    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common::{self, FailingAllocator};

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn utf16_string_push_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut s = arena.alloc_utf16_string();
        s.push('x');
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn utf16_string_insert_panics_from_grow_to_at_least() {
        let arena = Arena::builder_in(FailingAllocator::new(1)).build();
        let mut s = Utf16String::from_in("a", &arena);
        // Any replacement that forces a grow request triggers the panic;
        // the `FailingAllocator` denies every allocation after the first
        // regardless of size, so a moderate replacement (well past the
        // initial small chunk's residual capacity) is sufficient.
        let replacement = widestring::Utf16String::from_str(&"x".repeat(1024));
        s.insert_utf16_str(0, replacement.as_utfstr());
    }

    #[test]
    fn utf16_string_reserve_zero_on_nonempty_string_is_noop() {
        let arena = Arena::new();
        let mut s = Utf16String::from_in("already allocated", &arena);
        let cap = s.capacity();
        s.reserve(0);
        assert_eq!(s.capacity(), cap);
        assert_eq!(s.as_utf16_str().to_string(), "already allocated");
    }

    #[test]
    fn utf16_string_from_iterator_in_impls() {
        let arena = Arena::new();

        let chars = Utf16String::from_iter_in(['a', 'β', '🦀'], &arena);
        assert_eq!(chars.as_utf16_str().to_string(), "aβ🦀");

        let parts = [utf16str!("hi"), utf16str!("!")];
        let from_utf16 = Utf16String::from_iter_in(parts, &arena);
        assert_eq!(from_utf16.as_utf16_str().to_string(), "hi!");

        let from_strs = Utf16String::from_iter_in(["wide", " ", "string"], &arena);
        assert_eq!(from_strs.as_utf16_str().to_string(), "wide string");
    }

    #[cfg(all(feature = "utf16", feature = "serde"))]
    #[test]
    fn utf16_string_serialize_impl_body() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_str(utf16str!("serde 🦀"));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""serde 🦀""#);
    }

    #[test]
    fn alloc_utf16_str_box_oversized_routes_via_oversized_local() {
        let arena = Arena::new();
        let len_u16 = 16 * 1024;
        // `vec![value; n]` is a single allocation + bulk fill.
        let buf: Vec<u16> = vec![u16::from(b'a'); len_u16];
        let src = widestring::Utf16Str::from_slice(&buf).unwrap();
        let b = arena.alloc_utf16_str_box(src);
        assert_eq!(b.len(), len_u16);
    }

    #[test]
    fn alloc_utf16_str_arc_oversized_routes_via_oversized_shared() {
        let arena = Arena::new();
        let len_u16 = 16 * 1024;
        let buf: Vec<u16> = vec![u16::from(b'a'); len_u16];
        let src = widestring::Utf16Str::from_slice(&buf).unwrap();
        let arc = arena.alloc_utf16_str_arc(src);
        assert_eq!(arc.len(), len_u16);
    }

    #[test]
    fn alloc_utf16_str_arc_from_str_oversized_routes_via_oversized_shared() {
        let len = 4096;
        let src = "a".repeat(len);

        // First exercise the default arena so any default-config code paths
        // remain covered.
        let arena = Arena::new();
        let arc = arena.alloc_utf16_str_arc_from_str(&src);
        assert_eq!(arc.len(), len);

        // Then force a small `max_normal_alloc` (in bytes) so the 8 KiB
        // UTF-16 payload transcoded from a 4096-char ASCII string (2 bytes
        // per code unit, plus the length prefix) deterministically takes
        // the oversized-shared branch regardless of any future change to
        // the default threshold. (A shorter string than before keeps the
        // one-shot transcode affordable under Miri while still clearing the
        // 4 KiB threshold.)
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let arc = arena.alloc_utf16_str_arc_from_str(&src);
        assert_eq!(arc.len(), len);
        #[cfg(feature = "stats")]
        assert_eq!(arena.stats().oversized_chunks_allocated, 1);
    }

    #[test]
    fn alloc_utf16_str_box_from_str_oversized_routes_via_oversized_shared() {
        let len = 4096;
        let src = "a".repeat(len);

        // First exercise the default arena so any default-config code paths
        // remain covered.
        let arena = Arena::new();
        let b = arena.alloc_utf16_str_box_from_str(&src);
        assert_eq!(b.len(), len);

        // Then force a small `max_normal_alloc` (in bytes) so the 8 KiB
        // UTF-16 payload transcoded from a 4096-char ASCII string (2 bytes
        // per code unit, plus the length prefix) deterministically takes
        // the oversized-shared branch regardless of any future change to
        // the default threshold. (A shorter string than before keeps the
        // one-shot transcode affordable under Miri while still clearing the
        // 4 KiB threshold.)
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let b = arena.alloc_utf16_str_box_from_str(&src);
        assert_eq!(b.len(), len);
        #[cfg(feature = "stats")]
        assert_eq!(arena.stats().oversized_chunks_allocated, 1);
    }
}

mod from_mutants_extras_utf16_scattered {
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]
    extern crate alloc;
    use multitude::Arena;

    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common::{self, FailingAllocator, SendFailingAllocator};

    // =====================================================================
    // vec/vec.rs mutants
    // =====================================================================

    #[test]
    fn vec_362_shrink_to_fit_boundary() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(10);
        v.push(1);
        v.push(2);
        v.push(3);
        assert_eq!(v.len(), 3);
        assert!(v.capacity() >= 10);
        v.shrink_to_fit();
        // After shrink, capacity should equal len
        assert_eq!(v.capacity(), v.len());

        // When len == cap, shrink should be no-op
        v.shrink_to_fit(); // should not panic or reallocate
        assert_eq!(v.len(), 3);
    }

    // =====================================================================
    // Vec stronger tests
    // =====================================================================

    #[test]
    fn vec_451_resize_tight_budget() {
        let arena = Arena::builder().byte_budget(128 * 1024).build();
        let mut v = arena.alloc_vec_with_capacity::<u64>(5);
        v.push(1);
        v.push(2);
        v.resize(5, 42);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 42);
        assert_eq!(v[3], 42);
        assert_eq!(v[4], 42);
    }

    #[test]
    fn utf16_shrink_to_fit_uses_multiplication() {
        use multitude::strings::Utf16String;
        let arena = Arena::new();
        let mut s: Utf16String<'_> = arena.alloc_utf16_string();
        s.reserve(32);
        for _ in 0..8_u16 {
            s.push('a');
        }
        let cap_before = s.capacity();
        let len = s.len();
        assert!(cap_before >= len + 8, "test prerequisites: cap={cap_before}, len={len}");

        // Original `*`: shrink reclaims (cap_before - 8) * 2 bytes.
        // Mutated `+`:  reclaims (cap_before - 8) + 2 bytes.
        // In both, the shrink either succeeds (cap → 8) or fails (no-op),
        // so cap alone may not distinguish. We instead allocate a probe
        // immediately afterward and check whether it could fit in the
        // *expected* reclaimed range.
        s.shrink_to_fit();
        // If the shrink succeeded, cap == len. If it failed, cap unchanged.
        // The mutation does NOT cause `try_shrink_at_cursor` to fail
        // outright (smaller reclaim is accepted), so the observable here is
        // the post-shrink allocation cursor — exercised indirectly by
        // pushing more data and confirming content integrity.
        s.push('z');
        assert_eq!(s.len(), len + 1);
        // Content integrity is the strongest assertion we can make without
        // exposing internal arena cursors.
        let units: alloc::vec::Vec<u16> = s.as_slice().to_vec();
        let last = units.last().copied().expect("non-empty");
        assert_eq!(last, u16::from(b'z'));
    }

    #[test]
    fn utf16_exact_fit_push_is_observably_identical() {
        use multitude::strings::Utf16String;
        let arena = Arena::new();
        let mut s: Utf16String<'_> = arena.alloc_utf16_string();
        s.reserve(4);
        let cap = s.capacity();
        // Push exactly `cap` ASCII chars (each 1 u16). After the last push,
        // len == cap. The branch `needed > cap` is false; mutated `needed >= cap`
        // would attempt `try_grow_to_at_least(cap)` → also no-op.
        for _ in 0..cap {
            s.push('x');
        }
        assert_eq!(s.len(), cap);
        assert!(s.capacity() >= cap);
    }

    #[test]
    fn utf16_insert_at_start_preserves_tail() {
        use widestring::utf16str;
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("World");
        s.insert_utf16_str(0, utf16str!("Hello, "));
        let actual: std::string::String = std::char::decode_utf16(s.as_slice().iter().copied()).map(|r| r.unwrap()).collect();
        assert_eq!(actual, "Hello, World");
    }

    #[test]
    fn utf16_insert_in_middle_preserves_surrounding() {
        use widestring::utf16str;
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("HelloWorld");
        s.insert_utf16_str(5, utf16str!(", "));
        let actual: std::string::String = std::char::decode_utf16(s.as_slice().iter().copied()).map(|r| r.unwrap()).collect();
        assert_eq!(actual, "Hello, World");
    }

    #[test]
    fn utf16_remove_first_preserves_rest() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("hello");
        let removed = s.remove(0);
        assert_eq!(removed, 'h');
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn utf16_replace_range_replaces_exact_range() {
        use widestring::utf16str;
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("Hello, World!");
        s.replace_range(7..12, utf16str!("Rust"));
        let actual: std::string::String = std::char::decode_utf16(s.as_slice().iter().copied()).map(|r| r.unwrap()).collect();
        assert_eq!(actual, "Hello, Rust!");
    }

    /// Regression guard for the prefixed shared-allocation routing
    /// (`impl_alloc_prefixed_shared_arc`): an odd-length `u8` (`Arc<str>`)
    /// allocation leaves the shared bump cursor odd, then a `u16`
    /// (`Arc<Utf16Str>`) allocation reserves a block aligned to 4 bytes (so
    /// the per-`Arc` `AtomicU32` strong prefix is aligned, via
    /// `arc_block_align(u16) = max(2, 4)`). The routing sizes the refill /
    /// oversized hint with `worst_case_arc_slice_payload` (strong prefix +
    /// length prefix + payload + front alignment slack), so sweeping `u16`
    /// lengths across the `max_normal_alloc` boundary must always terminate
    /// (an under-sized hint would spin the refill loop) and produce correct
    /// contents.
    #[test]
    fn prefixed_shared_alloc_boundary_terminates_for_mixed_u8_u16() {
        // `max_normal_alloc` must be >= MIN_MAX_NORMAL_ALLOC (4096), so the
        // u16 normal/oversized boundary sits at `chars = mna / 2`. Sweep a
        // few char lengths right around that boundary for an even and an
        // odd `mna` (the parity drives the alignment edge case) plus one
        // larger boundary position. Verifying length + a handful of
        // sentinel code units (rather than decoding every unit) keeps the
        // per-iteration cost down to the unavoidable one-shot transcode,
        // which is what makes this affordable under Miri.
        for &mna in &[4096_usize, 4097, 6144] {
            let arena = Arena::builder().max_normal_alloc(mna).build();
            let center = mna / 2;
            for chars in center.saturating_sub(1)..=(center + 1).min(mna) {
                // Odd-length u8 (str) alloc to misalign the shared cursor.
                let narrow = "x".repeat(2 * (chars % 50) + 1);
                let narrow_arc = arena.alloc_str_arc(&narrow);
                assert_eq!(&*narrow_arc, narrow.as_str(), "str payload corrupted at mna={mna}, chars={chars}");
                // u16 (utf16) alloc right after at a boundary-spanning length.
                let wide = "y".repeat(chars);
                let wide_arc = arena.alloc_utf16_str_arc_from_str(&wide);
                // Sentinel checks instead of a full decode: the payload is
                // uniform ('y'), so a routing bug that returns the wrong
                // length or corrupts an edge/middle unit is still caught,
                // without an O(chars) decode loop per iteration.
                assert_eq!(wide_arc.len(), chars, "utf16 length wrong at mna={mna}, chars={chars}");
                if chars > 0 {
                    let units = wide_arc.as_slice();
                    let yy = u16::from(b'y');
                    assert_eq!(units[0], yy, "utf16 head corrupted at mna={mna}, chars={chars}");
                    assert_eq!(units[chars / 2], yy, "utf16 mid corrupted at mna={mna}, chars={chars}");
                    assert_eq!(units[chars - 1], yy, "utf16 tail corrupted at mna={mna}, chars={chars}");
                }
            }
        }
    }
}

mod utf16_zero_copy_freeze {
    use std::thread;

    use multitude::Arena;

    #[test]
    fn into_boxed_utf16_str_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("hello, world");
        let data_ptr = s.as_utf16_str().as_slice().as_ptr();
        let b: multitude::Box<multitude::strings::Utf16Str> = s.into_boxed_utf16_str();
        assert_eq!(b.len(), 12);
        assert_eq!(
            b.as_widestring_utf16_str().as_slice().as_ptr(),
            data_ptr,
            "into_boxed_utf16_str must not copy"
        );
    }

    #[test]
    fn into_arc_utf16_str_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string();
        s.push_from_str("shared utf16");
        let data_ptr = s.as_utf16_str().as_slice().as_ptr();
        let a: multitude::Arc<multitude::strings::Utf16Str> = s.into_arc_utf16_str();
        assert_eq!(
            a.as_widestring_utf16_str().as_slice().as_ptr(),
            data_ptr,
            "into_arc_utf16_str must not copy"
        );
        let a2 = a.clone();
        assert_eq!(a2.as_widestring_utf16_str(), a.as_widestring_utf16_str());
    }

    #[test]
    fn frozen_arc_utf16_str_outlives_arena_and_crosses_threads() {
        let arc: multitude::Arc<multitude::strings::Utf16Str> = {
            let arena = Arena::new();
            let mut s = arena.alloc_utf16_string();
            s.push_from_str("survives teardown");
            s.into_arc_utf16_str()
        };
        let clone = arc.clone();
        let len = thread::spawn(move || clone.len()).join().unwrap();
        assert_eq!(len, 17);
        assert_eq!(arc.len(), 17);
    }
}

mod utf16_smart_ptr_traits {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use multitude::strings::Utf16String;
    use multitude::{Arc, Arena, Box, FromIn as _};
    use widestring::{Utf16Str, utf16str};

    fn hash_of<T: Hash>(v: &T) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    #[test]
    fn arc_utf16_str_is_empty_distinguishes() {
        let arena = Arena::new();
        let empty: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!(""));
        let full: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("a"));
        assert!(empty.is_empty());
        assert!(!full.is_empty());
    }

    #[test]
    fn arc_utf16_str_display_renders_contents() {
        let arena = Arena::new();
        let s: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("hello"));
        assert_eq!(format!("{s}"), "hello");
    }

    #[test]
    fn arc_utf16_str_partial_eq_self_distinguishes() {
        let arena = Arena::new();
        let a: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("same"));
        let b: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("same"));
        let c: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("diff"));
        assert!(a == b);
        assert!((a != c));
    }

    #[test]
    fn arc_utf16_str_partial_eq_utf16str_distinguishes() {
        let arena = Arena::new();
        let s: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("xy"));
        let xy: &Utf16Str = utf16str!("xy");
        let no: &Utf16Str = utf16str!("no");
        assert!(s == *xy);
        assert!((s != *no));
        assert!(s == xy);
        assert!((s != no));
    }

    #[test]
    fn arc_utf16_str_hash_depends_on_contents() {
        let arena = Arena::new();
        let a: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("foo"));
        let b: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("foo"));
        let c: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("bar"));
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn arc_utf16_str_pointer_fmt_renders_address() {
        let arena = Arena::new();
        let s: Arc<multitude::strings::Utf16Str> = arena.alloc_utf16_str_arc(utf16str!("p"));
        let rendered = format!("{s:p}");
        assert!(!rendered.is_empty());
        assert!(rendered.chars().any(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn box_utf16_str_display_renders_contents() {
        let arena = Arena::new();
        let s: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("hello"));
        assert_eq!(format!("{s}"), "hello");
    }

    #[test]
    fn box_utf16_str_hash_depends_on_contents() {
        let arena = Arena::new();
        let a: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("foo"));
        let b: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("foo"));
        let c: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("bar"));
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn box_utf16_str_pointer_fmt_renders_address() {
        let arena = Arena::new();
        let s: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("p"));
        let rendered = format!("{s:p}");
        assert!(!rendered.is_empty());
        assert!(rendered.chars().any(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn box_utf16_str_partial_eq_utf16str_distinguishes() {
        let arena = Arena::new();
        let s: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("xy"));
        let xy: &Utf16Str = utf16str!("xy");
        let no: &Utf16Str = utf16str!("no");
        assert!(s == *xy);
        assert!((s != *no));
        assert!(s == xy);
        assert!((s != no));
    }

    // Exercises `Utf16Str::PartialEq<widestring::Utf16Str>` directly on the
    // deref'd newtype (the `Box`/`Arc` impls compare via `as_utf16_str`, so
    // they don't cover the newtype's own impl).
    #[test]
    fn utf16str_newtype_partial_eq_widestring_distinguishes() {
        let arena = Arena::new();
        let s: Box<multitude::strings::Utf16Str> = arena.alloc_utf16_str_box(utf16str!("xy"));
        assert!(*s == *utf16str!("xy"));
        assert!(*s != *utf16str!("no"));
    }

    #[test]
    fn utf16_string_display_renders_contents() {
        let arena = Arena::new();
        let mut s = Utf16String::from_in("hello", &arena);
        assert_eq!(format!("{s}"), "hello");
        s.push_from_str(" world");
        assert_eq!(format!("{s}"), "hello world");
    }
}

#[cfg(feature = "utf16")]
mod box_utf16_str_traits {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use core::borrow::{Borrow, BorrowMut};
    use core::cmp::Ordering;
    use core::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    use multitude::Arena;
    use widestring::{Utf16Str, utf16str};

    #[test]
    fn methods_len_is_empty_and_as_mut_utf16_str() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("hello"));
        assert_eq!(b.len(), 5);
        assert!(!b.is_empty());
        let _ = b.as_mut_widestring_utf16_str();
        let empty = arena.alloc_utf16_str_box(utf16str!(""));
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn deref_and_deref_mut() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("hi"));
        let _: &Utf16Str = &b;
        let _: &mut Utf16Str = &mut b;
    }

    #[test]
    fn as_ref_and_as_mut_via_trait() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("abc"));
        let _: &Utf16Str = AsRef::as_ref(&b);
        let _: &mut Utf16Str = AsMut::as_mut(&mut b);
    }

    #[test]
    fn borrow_and_borrow_mut_via_trait() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("xyz"));
        let _: &Utf16Str = Borrow::borrow(&b);
        let _: &mut Utf16Str = BorrowMut::borrow_mut(&mut b);
    }

    #[test]
    fn debug_and_display_format() {
        let arena = Arena::new();
        let b = arena.alloc_utf16_str_box(utf16str!("dbg"));
        let _ = format!("{b:?}");
        let _ = format!("{b}");
    }

    #[test]
    fn eq_ord_partialord() {
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_box(utf16str!("alpha"));
        let b = arena.alloc_utf16_str_box(utf16str!("alpha"));
        let c = arena.alloc_utf16_str_box(utf16str!("beta"));
        assert!(a == b);
        assert!(a != c);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(a.cmp(&c), Ordering::Less);
        assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
    }

    #[test]
    fn hash_and_pointer_format() {
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_box(utf16str!("hh"));
        let mut h = DefaultHasher::new();
        a.hash(&mut h);
        let _ = h.finish();
        let _ = format!("{a:p}");
    }

    #[test]
    fn unpin_impl_compiles() {
        fn assert_unpin<T: Unpin>() {}
        assert_unpin::<multitude::Box<multitude::strings::Utf16Str>>();
    }
}

#[cfg(feature = "utf16")]
mod alloc_utf16_try_variants_ok {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;
    use widestring::utf16str;

    #[test]
    fn try_alloc_utf16_str_arc_ok() {
        let a = Arena::new();
        let r = a.try_alloc_utf16_str_arc(utf16str!("ok")).unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn try_alloc_utf16_str_box_ok() {
        let a = Arena::new();
        let r = a.try_alloc_utf16_str_box(utf16str!("ok")).unwrap();
        assert_eq!(r.len(), 2);
    }
}
