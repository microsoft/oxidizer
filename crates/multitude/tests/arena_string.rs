// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "residue of Rc-test removal: orphaned helpers/imports kept to preserve surrounding test bodies verbatim"
)]

//! Tests for [`String`]: the growable arena-backed string builder.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::approx_constant, reason = "test uses 3.14 intentionally as a display value")]

mod common;

use core::cmp::Ordering;

use multitude::strings::String;
use multitude::vec::CollectIn;
use multitude::{Arena, FromIn};

#[test]
fn clear_and_reuse() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hello");
    let cap_before = s.capacity();
    s.clear();
    assert_eq!(s.len(), 0);
    assert_eq!(s.capacity(), cap_before);
    s.push_str("world");
    assert_eq!(s.as_str(), "world");
}

#[test]
fn clear_when_unallocated_is_noop() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.clear();
    assert!(s.is_empty());
}

#[test]
fn size_40_bytes_on_64bit() {
    // Five pointer-sized fields hold the buffer, length, capacity, freeze
    // prefix state, and arena reference.
    if size_of::<usize>() == 8 {
        assert_eq!(size_of::<String<'_>>(), 40);
    }
}

#[test]
fn push_single_char() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push('a');
    s.push('é');
    s.push('日');
    assert_eq!(s.as_str(), "aé日");
}

#[test]
fn push_str_handles_every_inline_copy_length() {
    let arena = Arena::new();
    let mut string = arena.alloc_string_with_capacity(45);
    let source = "abcdefghi";

    for len in 0..=9 {
        string.push_str(&source[..len]);
    }

    assert_eq!(string.as_str(), "aababcabcdabcdeabcdefabcdefgabcdefghabcdefghi");
}

#[test]
fn reserve_grows_capacity() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.reserve(100);
    assert!(s.capacity() >= 100);
    s.push_str("hi");
    assert_eq!(s.as_str(), "hi");
}

#[test]
fn reserve_noop_when_already_large() {
    let arena = Arena::new();
    let mut s = arena.alloc_string_with_capacity(200);
    let cap = s.capacity();
    s.reserve(50);
    assert_eq!(s.capacity(), cap);
}

#[test]
fn as_str_when_empty_returns_empty_slice() {
    let arena = Arena::new();
    let s = arena.alloc_string();
    assert_eq!(s.as_str(), "");
    assert_eq!(s.len(), 0);
    assert_eq!(s.capacity(), 0);
}

#[test]
fn extend_chars() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.extend(['a', 'b', 'c'].iter().copied());
    assert_eq!(s.as_str(), "abc");
}

#[test]
fn extend_chars_empty_iter() {
    // Hits the `lower == 0` branch in Extend<char>::extend.
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    let empty: [char; 0] = [];
    s.extend(empty.iter().copied());
    assert_eq!(s.as_str(), "");
}

#[test]
fn extend_strs() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.extend(["foo", "bar", "baz"].iter().copied());
    assert_eq!(s.as_str(), "foobarbaz");
}

#[test]
fn collect_in_chars() {
    let arena = Arena::new();
    let s: String<'_> = "héllo".chars().collect_in(&arena);
    assert_eq!(s.as_str(), "héllo");
}

#[test]
fn traits_compile() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hi");
    let _: &str = s.as_ref();
    let r: &str = core::borrow::Borrow::borrow(&s);
    assert_eq!(r, "hi");
    assert_eq!(format!("{s:?}"), "\"hi\"");
    assert_eq!(format!("{s}"), "hi");
    let mut other = arena.alloc_string();
    other.push_str("hi");
    let mut big = arena.alloc_string();
    big.push_str("z");
    assert_eq!(s, other);
    assert!(s < big);
    assert_eq!(s.cmp(&big), Ordering::Less);
    assert_eq!(s.partial_cmp(&big), Some(Ordering::Less));
    assert_eq!(common::hash_of(&s), common::hash_of(&other));
}

#[test]
fn try_push_succeeds() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.try_push('a').unwrap();
    s.try_push('b').unwrap();
    assert_eq!(&*s, "ab");
}

#[test]
fn try_push_str_succeeds() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.try_push_str("hello").unwrap();
    s.try_push_str(" world").unwrap();
    assert_eq!(&*s, "hello world");
}

#[test]
fn try_push_str_empty_is_noop_ok() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.try_push_str("").unwrap();
    assert!(s.is_empty());
}

#[test]
fn try_reserve_succeeds() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.try_reserve(64).unwrap();
    assert!(s.capacity() >= 64);
}

#[test]
fn try_with_capacity_in_succeeds() {
    let arena = Arena::new();
    let s = arena.try_alloc_string_with_capacity(32).unwrap();
    assert!(s.capacity() >= 32);
    assert!(s.is_empty());
}

#[test]
fn try_with_capacity_in_zero_does_not_allocate() {
    let arena = Arena::new();
    let s = arena.try_alloc_string_with_capacity(0).unwrap();
    assert_eq!(s.capacity(), 0);
}

#[test]
fn try_push_str_returns_err_on_alloc_failure() {
    use multitude::AllocError;
    // FailingAllocator with 0 budget: every allocate() fails.
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut s = arena.alloc_string();
    let err: AllocError = s.try_push_str("x").unwrap_err();
    assert!(err.is_allocator_failure());
}

#[test]
fn try_push_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut s = arena.alloc_string();
    let _ = s.try_push('x').unwrap_err();
}

#[test]
fn try_reserve_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut s = arena.alloc_string();
    let _ = s.try_reserve(16).unwrap_err();
}

#[test]
fn try_with_capacity_in_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let result = arena.try_alloc_string_with_capacity(16);
    let _ = result.unwrap_err();
}

#[test]
fn try_grow_path_via_push_str_after_initial() {
    // Drives try_grow_to_at_least's slow path (cap > 0 branch).
    let arena = Arena::new();
    let mut s = arena.try_alloc_string_with_capacity(4).unwrap();
    s.try_push_str("abcd").unwrap(); // fills initial cap exactly
    s.try_push_str("e").unwrap(); // forces grow
    assert_eq!(&*s, "abcde");
    assert!(s.capacity() >= 5);
}

#[test]
fn from_in_str_copies_content() {
    let arena = Arena::new();
    let s = String::from_in("hello, world", &arena);
    assert_eq!(s.as_str(), "hello, world");
    assert!(s.capacity() >= "hello, world".len());
}

#[test]
fn from_in_str_empty() {
    let arena = Arena::new();
    let s = String::from_in("", &arena);
    assert!(s.is_empty());
    assert_eq!(s.capacity(), 0);
    assert_eq!(s.as_str(), "");
}

#[test]
fn as_bytes_returns_correct_bytes() {
    let arena = Arena::new();
    let s = String::from_in("héllo", &arena);
    assert_eq!(s.as_bytes(), "héllo".as_bytes());
}

#[test]
fn as_bytes_empty() {
    let arena = Arena::new();
    let s = arena.alloc_string();
    assert_eq!(s.as_bytes(), b"");
}

#[test]
fn as_mut_str_allows_mutation() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    s.as_mut_str().make_ascii_uppercase();
    assert_eq!(s.as_str(), "HELLO");
}

#[test]
fn as_mut_str_empty() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    assert_eq!(s.as_mut_str(), "");
}

#[test]
fn as_ptr_and_as_mut_ptr() {
    let arena = Arena::new();
    let mut s = String::from_in("hi", &arena);
    // Read the bytes through a shared pointer *before* taking any mutable
    // pointer: `as_mut_ptr` reborrows `&mut str`/`&mut [u8]`, which would
    // invalidate an earlier shared pointer under Stacked Borrows — exactly as
    // it does in `std`, where both pointers come via `Deref`/`DerefMut` to
    // `str`.
    let p = s.as_ptr();
    // SAFETY: `p` addresses the first of the two initialized bytes "hi".
    unsafe {
        assert_eq!(*p, b'h');
    }
    // SAFETY: offset 1 is within the two initialized bytes.
    let p1 = unsafe { p.add(1) };
    // SAFETY: `p1` addresses the second initialized byte.
    unsafe {
        assert_eq!(*p1, b'i');
    }
    let const_addr = p.addr();
    // `as_ptr` and `as_mut_ptr` address the same buffer (compare addresses
    // only; the pointers' borrow tags differ).
    let mut_addr = s.as_mut_ptr().addr();
    assert_eq!(const_addr, mut_addr);
}

#[test]
fn pop_returns_chars_in_reverse() {
    let arena = Arena::new();
    let mut s = String::from_in("a💖é", &arena);
    assert_eq!(s.pop(), Some('é'));
    assert_eq!(s.pop(), Some('💖'));
    assert_eq!(s.pop(), Some('a'));
    assert_eq!(s.pop(), None);
    assert!(s.is_empty());
}

#[test]
fn truncate_shortens() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    let cap = s.capacity();
    s.truncate(3);
    assert_eq!(s.as_str(), "hel");
    assert_eq!(s.capacity(), cap, "capacity unchanged");
}

#[test]
fn truncate_noop_when_longer() {
    let arena = Arena::new();
    let mut s = String::from_in("hi", &arena);
    s.truncate(50);
    assert_eq!(s.as_str(), "hi");
}

#[test]
#[should_panic(expected = "char boundary")]
fn truncate_panics_on_non_boundary() {
    let arena = Arena::new();
    let mut s = String::from_in("é", &arena); // 2 bytes
    s.truncate(1);
}

#[test]
fn shrink_to_fit_reclaims_when_at_cursor() {
    let arena = Arena::new();
    let mut s = arena.alloc_string_with_capacity(1024);
    s.push_str("short");
    let _len = s.len();
    s.shrink_to_fit();
    // Buffer is at the bump cursor, so shrink should succeed.
    assert_eq!(s.capacity(), 5);
    assert_eq!(s.as_str(), "short");
}

#[test]
fn shrink_to_fit_empty_or_full_noop() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.shrink_to_fit();
    assert_eq!(s.capacity(), 0);

    let mut s2 = arena.alloc_string_with_capacity(4);
    s2.push_str("abcd");
    let cap = s2.capacity();
    s2.shrink_to_fit();
    assert_eq!(s2.capacity(), cap);
}

#[test]
fn leak_reclaims_unused_capacity_tail() {
    // `String::leak` hands its unused `[len, cap)` byte tail back to the
    // chunk when the buffer is still at the bump cursor, so the next
    // allocation reuses that space.
    let arena = Arena::new();
    let mut s = arena.alloc_string_with_capacity(64);
    s.push_str("abc");
    let base = s.as_str().as_ptr() as usize;
    let leaked: &mut str = s.leak();
    assert_eq!(leaked, "abc");
    assert_eq!(leaked.as_ptr() as usize, base);
    // Reclaimed tail reused: the next str lands right after "abc".
    let next = arena.alloc_str("XY");
    assert_eq!(next.as_ptr() as usize, base + 3);
}

#[test]
fn drop_at_cursor_reclaims_storage() {
    // Dropping a `String` whose buffer ends at the bump cursor returns its
    // whole storage to the chunk; the next allocation reuses it.
    let arena = Arena::new();
    let base = {
        let mut s = arena.alloc_string_with_capacity(64);
        s.push_str("abc");
        s.as_str().as_ptr() as usize
    }; // `s` dropped here -> reclaims its backing bytes.
    let next = arena.alloc_str("WXYZ");
    assert_eq!(next.as_ptr() as usize, base);
}

#[test]
fn insert_at_various_positions() {
    let arena = Arena::new();
    let mut s = String::from_in("ac", &arena);
    s.insert(1, 'b');
    assert_eq!(s.as_str(), "abc");
    s.insert(0, 'Z');
    assert_eq!(s.as_str(), "Zabc");
    s.insert(s.len(), '!');
    assert_eq!(s.as_str(), "Zabc!");
}

#[test]
fn insert_multibyte_char() {
    let arena = Arena::new();
    let mut s = String::from_in("ab", &arena);
    s.insert(1, '💖');
    assert_eq!(s.as_str(), "a💖b");
}

#[test]
fn insert_str_grows() {
    let arena = Arena::new();
    let mut s = String::from_in("ad", &arena);
    s.insert_str(1, std::string::String::from("bc"));
    assert_eq!(s.as_str(), "abcd");
}

#[test]
fn insert_str_empty_is_noop() {
    let arena = Arena::new();
    let mut s = String::from_in("hi", &arena);
    s.insert_str(1, "");
    assert_eq!(s.as_str(), "hi");
}

#[test]
#[should_panic(expected = "char boundary")]
fn insert_panics_on_bad_index() {
    let arena = Arena::new();
    let mut s = String::from_in("é", &arena);
    s.insert(1, 'x');
}

#[test]
#[should_panic(expected = "insertion index out of bounds")]
fn insert_panics_when_idx_past_end() {
    let arena = Arena::new();
    let mut s = String::from_in("hi", &arena);
    s.insert(99, 'x');
}

#[test]
fn remove_returns_char() {
    let arena = Arena::new();
    let mut s = String::from_in("a💖c", &arena);
    let ch = s.remove(1);
    assert_eq!(ch, '💖');
    assert_eq!(s.as_str(), "ac");
}

#[test]
fn remove_first_and_last() {
    let arena = Arena::new();
    let mut s = String::from_in("abcd", &arena);
    assert_eq!(s.remove(0), 'a');
    assert_eq!(s.as_str(), "bcd");
    assert_eq!(s.remove(s.len() - 1), 'd');
    assert_eq!(s.as_str(), "bc");
}

#[test]
#[should_panic(expected = "out of bounds")]
fn remove_panics_when_empty() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    let _ = s.remove(0);
}

#[test]
fn retain_filters_chars() {
    let arena = Arena::new();
    let mut s = String::from_in("a1b2c3", &arena);
    s.retain(|c| c.is_ascii_alphabetic());
    assert_eq!(s.as_str(), "abc");
}

#[test]
fn retain_removes_all() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    s.retain(|_| false);
    assert!(s.is_empty());
}

#[test]
fn retain_keeps_all() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    s.retain(|_| true);
    assert_eq!(s.as_str(), "hello");
}

#[test]
fn retain_with_multibyte() {
    let arena = Arena::new();
    let mut s = String::from_in("a💖b💖c", &arena);
    s.retain(|c| c != '💖');
    assert_eq!(s.as_str(), "abc");
}

#[test]
fn replace_range_same_length() {
    let arena = Arena::new();
    let mut s = String::from_in("hello world", &arena);
    s.replace_range(6..11, std::string::String::from("earth"));
    assert_eq!(s.as_str(), "hello earth");
}

#[test]
fn replace_range_grow() {
    let arena = Arena::new();
    let mut s = String::from_in("hi world", &arena);
    s.replace_range(0..2, "hello");
    assert_eq!(s.as_str(), "hello world");
}

#[test]
fn replace_range_shrink() {
    let arena = Arena::new();
    let mut s = String::from_in("hello world", &arena);
    s.replace_range(0..5, "hi");
    assert_eq!(s.as_str(), "hi world");
}

#[test]
fn replace_range_unbounded() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    s.replace_range(.., "goodbye");
    assert_eq!(s.as_str(), "goodbye");
}

#[test]
fn replace_range_empty_replacement() {
    let arena = Arena::new();
    let mut s = String::from_in("hello world", &arena);
    s.replace_range(5..11, "");
    assert_eq!(s.as_str(), "hello");
}

#[test]
fn replace_range_inclusive() {
    let arena = Arena::new();
    let mut s = String::from_in("abcdef", &arena);
    s.replace_range(1..=3, "XYZW");
    assert_eq!(s.as_str(), "aXYZWef");
}

#[test]
#[should_panic(expected = "char boundary")]
fn replace_range_panics_on_non_boundary() {
    let arena = Arena::new();
    let mut s = String::from_in("é", &arena);
    s.replace_range(0..1, "x");
}

#[test]
fn clone_produces_equal_independent_string() {
    let arena = Arena::new();
    let original = String::from_in("hello", &arena);
    let mut cloned = original.clone();
    assert_eq!(original.as_str(), cloned.as_str());
    // Independent buffers
    assert_ne!(original.as_ptr(), cloned.as_ptr());
    cloned.push_str(" world");
    assert_eq!(original.as_str(), "hello");
    assert_eq!(cloned.as_str(), "hello world");
}

#[test]
fn clone_empty() {
    let arena = Arena::new();
    let original = arena.alloc_string();
    let cloned = original.clone();
    assert_eq!(cloned.as_str(), "");
    assert_eq!(cloned.capacity(), 0);
}

#[test]
fn deref_mut_allows_mutation() {
    let arena = Arena::new();
    let mut s = String::from_in("hello", &arena);
    let r: &mut str = &mut s;
    r.make_ascii_uppercase();
    assert_eq!(s.as_str(), "HELLO");
}

#[test]
fn as_mut_trait_allows_mutation() {
    let arena = Arena::new();
    let mut s = String::from_in("abc", &arena);
    let r: &mut str = AsMut::as_mut(&mut s);
    r.make_ascii_uppercase();
    assert_eq!(s.as_str(), "ABC");
}

#[test]
fn borrow_mut_trait_allows_mutation() {
    let arena = Arena::new();
    let mut s = String::from_in("xyz", &arena);
    let r: &mut str = core::borrow::BorrowMut::borrow_mut(&mut s);
    r.make_ascii_uppercase();
    assert_eq!(s.as_str(), "XYZ");
}

#[test]
fn collect_str_slices_into_string() {
    let arena = Arena::new();
    let parts = ["hello", ", ", "world", "!"];
    let s: String = parts.into_iter().collect_in(&arena);
    assert_eq!(s.as_str(), "hello, world!");
}

#[test]
fn collect_empty_str_slices() {
    let arena = Arena::new();
    let parts: [&str; 0] = [];
    let s: String = parts.into_iter().collect_in(&arena);
    assert_eq!(s.as_str(), "");
    assert!(s.is_empty());
}

#[test]
fn collect_single_str_slice() {
    let arena = Arena::new();
    let s: String = core::iter::once("only").collect_in(&arena);
    assert_eq!(s.as_str(), "only");
}

#[test]
fn collect_str_slices_with_unicode() {
    let arena = Arena::new();
    let parts = ["café", " ", "naïve", " ", "résumé"];
    let s: String = parts.into_iter().collect_in(&arena);
    assert_eq!(s.as_str(), "café naïve résumé");
}

#[test]
fn collect_chars_into_string() {
    let arena = Arena::new();
    let s: String = "hello".chars().collect_in(&arena);
    assert_eq!(s.as_str(), "hello");
}

#[test]
fn collect_chars_empty() {
    let arena = Arena::new();
    let s: String = "".chars().collect_in(&arena);
    assert!(s.is_empty());
}

#[test]
fn write_macro_formats_into_string() {
    use core::fmt::Write;
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    write!(s, "x = {}, y = {}", 42, 3.14).unwrap();
    assert_eq!(s.as_str(), "x = 42, y = 3.14");
}

#[test]
fn write_macro_appends() {
    use core::fmt::Write;
    let arena = Arena::new();
    let mut s = String::from_in("prefix:", &arena);
    write!(s, " {}", 100).unwrap();
    assert_eq!(s.as_str(), "prefix: 100");
}

#[test]
fn write_char_via_trait() {
    use core::fmt::Write;
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.write_char('A').unwrap();
    s.write_char('B').unwrap();
    s.write_char('C').unwrap();
    assert_eq!(s.as_str(), "ABC");
}

#[test]
fn write_str_via_trait() {
    use core::fmt::Write;
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.write_str("hello").unwrap();
    s.write_str(", world").unwrap();
    assert_eq!(s.as_str(), "hello, world");
}

mod arena_str {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::redundant_clone, reason = "tests exercise Clone explicitly")]
    use core::cmp::Ordering;
    use std::collections::{BTreeMap, HashMap};

    use multitude::Arena;

    use crate::common;

    #[test]
    fn arena_arc_str_basic() {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("hi");
        assert_eq!(&*s, "hi");
        assert_eq!(s.len(), 2);
        assert!(!s.is_empty());
        let s2 = s.clone();
        let h = std::thread::spawn(move || {
            assert_eq!(&*s2, "hi");
        });
        h.join().unwrap();
    }

    #[test]
    fn arena_arc_str_empty() {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("");
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn arena_arc_str_traits_compile() {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("hi");
        let _: &str = s.as_ref();
        let r: &str = core::borrow::Borrow::borrow(&s);
        assert_eq!(r, "hi");
        assert_eq!(format!("{s:?}"), "\"hi\"");
        assert_eq!(format!("{s}"), "hi");
        let other = arena.alloc_str_arc("hi");
        let big = arena.alloc_str_arc("z");
        assert_eq!(s, other);
        assert!(s < big);
        assert_eq!(s.cmp(&big), Ordering::Less);
        assert_eq!(s.partial_cmp(&big), Some(Ordering::Less));
        assert_eq!(common::hash_of(&s), common::hash_of(&other));
    }

    #[test]
    fn arena_arc_str_outlives_arena() {
        // Drives the `teardown_chunk(chunk, false)` branch in
        // Arc<str>::Drop when this is the LAST reference and the arena
        // itself has already been dropped.
        let s: multitude::Arc<str> = {
            let arena = Arena::new();
            arena.alloc_str_arc("survives the arena")
        };
        assert_eq!(&*s, "survives the arena");
        drop(s); // teardown_chunk(chunk, false) for the Chunk.
    }

    #[test]
    fn from_arena_arc_str_to_arena_arc_byte_slice() {
        use multitude::Arc;
        let arena = Arena::new();
        let s: Arc<str> = arena.alloc_str_arc("payload");
        let bytes: Arc<[u8]> = s.into();
        assert_eq!(&*bytes, b"payload");
    }

    #[test]
    fn arena_arc_byte_slice_is_send_sync() {
        use multitude::Arc;
        let arena = Arena::new();
        let s: Arc<str> = arena.alloc_str_arc("threaded");
        let bytes: Arc<[u8]> = s.into();
        let bytes2 = bytes.clone();
        let h = std::thread::spawn(move || bytes2.len());
        assert_eq!(h.join().unwrap(), bytes.len());
    }
}

mod arena_box_str {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]

    use core::cmp::Ordering;
    use std::collections::{BTreeMap, HashMap};

    use multitude::{Arena, Box};

    use crate::common;

    #[test]
    fn arena_box_str_basic() {
        let arena = Arena::new();
        let s = arena.alloc_str_box("hello, world");
        assert_eq!(&*s, "hello, world");
        assert_eq!(s.len(), 12);
        assert!(!s.is_empty());
    }

    #[test]
    fn arena_box_str_empty() {
        let arena = Arena::new();
        let s = arena.alloc_str_box("");
        assert_eq!(&*s, "");
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn arena_box_str_is_eight_bytes() {
        // The whole reason `Box<str>` exists rather than `ArenaBox<str>`
        // (16 bytes via fat pointer): single-pointer compactness.
        assert_eq!(size_of::<Box<str>>(), size_of::<usize>());
    }

    #[test]
    fn arena_box_str_mutable_in_place() {
        let arena = Arena::new();
        let mut s = arena.alloc_str_box("hello");
        s.make_ascii_uppercase();
        assert_eq!(&*s, "HELLO");
    }

    #[test]
    fn arena_box_str_as_mut_via_deref_mut() {
        let arena = Arena::new();
        let mut s = arena.alloc_str_box("Mixed Case");
        let m: &mut str = &mut s;
        m.make_ascii_lowercase();
        assert_eq!(&*s, "mixed case");
    }

    #[test]
    fn arena_box_str_accepts_string() {
        // impl AsRef<str> covers both &str and String.
        let arena = Arena::new();
        let owned = std::string::String::from("from String");
        let s = arena.alloc_str_box(owned);
        assert_eq!(&*s, "from String");
    }

    #[test]
    fn try_alloc_str_box_succeeds() {
        let arena = Arena::new();
        let s = arena.try_alloc_str_box("ok").unwrap();
        assert_eq!(&*s, "ok");
    }

    #[test]
    fn arena_box_str_traits_compile() {
        let arena = Arena::new();
        let s = arena.alloc_str_box("hi");
        let _: &str = s.as_ref();
        let r: &str = core::borrow::Borrow::borrow(&s);
        assert_eq!(r, "hi");
        assert_eq!(format!("{s:?}"), "\"hi\"");
        assert_eq!(format!("{s}"), "hi");
        let other = arena.alloc_str_box("hi");
        let big = arena.alloc_str_box("z");
        assert_eq!(s, other);
        assert!(s < big);
        assert_eq!(s.cmp(&big), Ordering::Less);
        assert_eq!(s.partial_cmp(&big), Some(Ordering::Less));
        assert_eq!(common::hash_of(&s), common::hash_of(&other));
    }

    #[test]
    fn arena_box_str_eq_and_hash_via_hashmap() {
        let arena = Arena::new();
        let key = arena.alloc_str_box("key");
        let mut map: HashMap<Box<str>, i32> = HashMap::new();
        let _ = map.insert(key, 1);
        // Borrow<str> lookup also works, so we don't need the original key.
        assert_eq!(map.get("key"), Some(&1));
    }

    #[test]
    fn arena_box_str_works_as_btreemap_key() {
        let arena = Arena::new();
        let mut m: BTreeMap<Box<str>, u32> = BTreeMap::new();
        let _ = m.insert(arena.alloc_str_box("a"), 1);
        let _ = m.insert(arena.alloc_str_box("b"), 2);
        assert_eq!(m.get("a"), Some(&1));
        assert_eq!(m.get("b"), Some(&2));
    }

    #[test]
    fn arena_box_str_drop_releases_chunk_immediately() {
        // Box<str> drops its chunk hold the moment the smart pointer is dropped.
        // Subsequent allocations in the arena must still work, exercising
        // the dec_ref + (optional) teardown_chunk path in Box<str>::Drop.
        let arena = Arena::new();
        let s = arena.alloc_str_box("temporary");
        assert_eq!(&*s, "temporary");
        drop(s);
        // Arena still works.
        let s2 = arena.alloc_str_box("after-drop");
        assert_eq!(&*s2, "after-drop");
    }

    #[test]
    fn arena_box_str_lifetime_bound_to_arena() {
        // The borrow checker must reject use of an `Box<str>` whose
        // arena has been dropped. We can't write a runtime test for the
        // negative case (it's a compile error), but we can verify positive
        // case: dropping the box BEFORE the arena is fine.
        let arena = Arena::new();
        {
            let s = arena.alloc_str_box("inner");
            assert_eq!(&*s, "inner");
        }
        let s2 = arena.alloc_str_box("outer");
        assert_eq!(&*s2, "outer");
    }

    #[test]
    fn many_arena_box_str_allocations_force_chunk_rotation() {
        let arena = Arena::builder().build();
        let mut handles = std::vec::Vec::new();
        for i in 0..200 {
            handles.push(arena.alloc_str_box(format!("item{i}")));
        }
        assert_eq!(&*handles[0], "item0");
        assert_eq!(&*handles[199], "item199");
    }

    #[test]
    fn arena_box_str_round_trip_through_drop_does_not_corrupt() {
        let arena = Arena::new();
        // 256 transient boxes still exercise repeated alloc/drop teardown on
        // the same arena without paying for 1000 interpreted drops under miri.
        for i in 0..256 {
            let s = arena.alloc_str_box(format!("transient-{i}"));
            // Each iteration: alloc, mutate, drop. The dec_ref + teardown
            // must keep the arena healthy across many iterations.
            let _ = s.len();
        }
        // Final allocation works too.
        let s = arena.alloc_str_box("final");
        assert_eq!(&*s, "final");
    }

    #[test]
    fn arena_box_str_borrow_mut_and_pointer() {
        use core::borrow::BorrowMut;
        let arena = Arena::new();
        let mut s = arena.alloc_str_box("hello");
        let m: &mut str = s.borrow_mut();
        m.make_ascii_uppercase();
        assert_eq!(&*s, "HELLO");
        let p = format!("{s:p}");
        assert!(p.starts_with("0x"), "Pointer format: {p}");
    }
}

mod mutants_for_string {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    use multitude::Arena;
    use multitude::strings::String as MString;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn with_capacity_zero_does_not_allocate() {
        let arena = Arena::new();
        let s0 = arena.alloc_string_with_capacity(0);
        assert_eq!(s0.capacity(), 0);
        // Compare against `new_in` (the documented no-alloc constructor).
        let s_new = arena.alloc_string();
        assert_eq!(s_new.capacity(), 0);
        // Both have the same dangling data pointer (== 1 by NonNull::dangling()).
        // ptr identity is the strongest observable signal here.
        assert_eq!(s0.as_ptr() as usize, s_new.as_ptr() as usize);
    }

    #[test]
    fn string_reserve_at_exact_fit_does_not_regrow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(16);
        let cap = s.capacity();
        let ptr_before = s.as_ptr();
        s.reserve(cap); // additional == cap → needed == cap (len was 0)
        assert_eq!(s.capacity(), cap);
        assert_eq!(s.as_ptr(), ptr_before);
    }

    #[test]
    fn into_box_handles_empty_and_non_empty() {
        let arena = Arena::new();

        let s_empty = arena.alloc_string();
        let b_empty = s_empty.into_boxed_str();
        assert_eq!(&*b_empty, "");
        assert_eq!(b_empty.len(), 0);

        let mut s = arena.alloc_string_with_capacity(16);
        s.push_str("hello");
        let b = s.into_boxed_str();
        assert_eq!(&*b, "hello");
        assert_eq!(b.len(), 5);
    }

    /// `String::into_arc` freezes into a shared, reference-counted
    /// `Arc<str>` whose contents match the builder, for both empty and
    /// non-empty inputs, and which can be cloned and outlive the arena.
    #[test]
    fn into_arc_handles_empty_and_non_empty() {
        use multitude::Arc;

        let arena = Arena::new();

        let s_empty = arena.alloc_string();
        let a_empty: Arc<str> = Arc::from(s_empty);
        assert_eq!(&*a_empty, "");
        assert_eq!(a_empty.len(), 0);

        let mut s = arena.alloc_string_with_capacity(16);
        s.push_str("hello");
        let a: Arc<str> = Arc::from(s);
        assert_eq!(&*a, "hello");
        assert_eq!(a.len(), 5);

        // Cloning shares the same backing allocation.
        let a2 = a.clone();
        assert_eq!(&*a2, "hello");
        assert_eq!(a.as_ptr(), a2.as_ptr());
    }

    /// An `Arc<str>` produced by `into_arc` outlives the arena it was
    /// built from (the backing chunk is held by the refcount).
    #[test]
    fn into_arc_outlives_arena() {
        use multitude::Arc;

        let escaped: Arc<str> = {
            let arena = Arena::new();
            let mut s = arena.alloc_string();
            s.push_str("survives");
            let a = Arc::from(s);
            drop(arena);
            a
        };
        assert_eq!(&*escaped, "survives");
    }

    #[test]
    fn reclaim_tail_does_not_corrupt_frozen_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(256);
        s.push_str("frozen!");
        let frozen = s.into_boxed_str();
        let _filler: multitude::vec::Vec<'_, u64> = {
            let mut v = arena.alloc_vec_with_capacity::<u64>(64);
            for i in 0..64 {
                v.push(i);
            }
            v
        };
        assert_eq!(&*frozen, "frozen!");
    }
}

mod format_macro {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn format_macro_basic() {
        let arena = Arena::new();
        let name = "world";
        let s = multitude::strings::format!(in &arena, "hello, {name}!");
        assert_eq!(&*s, "hello, world!");
    }

    #[test]
    fn format_macro_with_multiple_args() {
        let arena = Arena::new();
        let s = multitude::strings::format!(in &arena, "{}+{}={}", 2, 3, 5);
        assert_eq!(&*s, "2+3=5");
    }

    #[test]
    fn format_macro_empty_format_string() {
        let arena = Arena::new();
        let s = multitude::strings::format!(in &arena, "");
        assert_eq!(&*s, "");
    }

    #[test]
    fn arena_string_is_a_fmt_write_target() {
        use core::fmt::Write;
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        write!(&mut s, "x={}", 42).unwrap();
        assert_eq!(s.as_str(), "x=42");

        s.write_char('!').unwrap();
        assert_eq!(s.as_str(), "x=42!");
    }
}

/// Zero-copy freeze: `String::into_boxed_str` / `into_arc_str` reuse the
/// backing `Vec<u8>` storage in place (retag `[u8] → str`, no copy).
mod string_zero_copy_freeze {
    use std::thread;

    use multitude::strings::String;
    use multitude::{Arc, Arena, Box};

    #[test]
    fn into_boxed_str_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello, world");
        let data_ptr = s.as_str().as_ptr();
        let b: Box<str> = s.into_boxed_str();
        assert_eq!(&*b, "hello, world");
        assert_eq!(b.as_str().as_ptr(), data_ptr, "into_boxed_str must not copy");
    }

    #[test]
    fn into_arc_str_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("shared string");
        let data_ptr = s.as_str().as_ptr();
        let a: Arc<str> = Arc::from(s);
        assert_eq!(&*a, "shared string");
        assert_eq!(a.as_str().as_ptr(), data_ptr, "into_arc_str must not copy");
        let a2 = a.clone();
        assert!(Arc::ptr_eq(&a, &a2), "clone shares the same frozen payload");
        assert_eq!(&*a2, "shared string");
    }

    #[test]
    fn frozen_arc_str_outlives_arena_and_crosses_threads() {
        let arc: Arc<str> = {
            let arena = Arena::new();
            let mut s = arena.alloc_string();
            s.push_str("survives teardown");
            Arc::from(s)
        };
        let clone = arc.clone();
        let len = thread::spawn(move || clone.len()).join().unwrap();
        assert_eq!(len, "survives teardown".len());
        assert_eq!(&*arc, "survives teardown");
    }

    #[test]
    fn grown_string_freezes_in_place_after_relocation() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        for _ in 0..64 {
            s.push_str("abcd");
        }
        let data_ptr = s.as_str().as_ptr();
        let b = s.into_boxed_str();
        assert_eq!(b.len(), 256);
        assert_eq!(b.as_str().as_ptr(), data_ptr, "relocated buffer still freezes in place");
    }

    #[test]
    fn empty_string_freezes_to_empty_str() {
        let arena = Arena::new();
        let s = arena.alloc_string();
        let b = s.into_boxed_str();
        assert_eq!(&*b, "");
        let s2 = arena.alloc_string();
        let a: Arc<str> = Arc::from(s2);
        assert_eq!(&*a, "");
    }
}

mod str_smart_ptr_traits {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use multitude::{Arc, Arena, Box};

    fn hash_of<T: Hash>(v: &T) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    // --- Arc<str> -----------------------------------------------------------

    #[test]
    fn arc_str_partial_eq_ref_str_true_and_false() {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("hello");
        assert_eq!(s, "hello");
        assert!((s != "world"));
    }

    #[test]
    fn arc_str_partial_eq_str_returns_actual_compare() {
        let arena = Arena::new();
        let s: Arc<str> = arena.alloc_str_arc("alpha");
        let alpha: std::string::String = "alpha".to_owned();
        let beta: std::string::String = "beta".to_owned();
        assert_eq!(s, *alpha.as_str());
        assert_ne!(s, *beta.as_str());
    }

    #[test]
    fn arc_str_as_ref_returns_actual_contents() {
        let arena = Arena::new();
        let s: Arc<str> = arena.alloc_str_arc("payload");
        let r: &str = s.as_ref();
        assert_eq!(r, "payload");
        assert_ne!(r, "");
        assert_ne!(r, "xyzzy");
    }

    #[test]
    fn arc_str_partial_eq_self_distinguishes() {
        let arena = Arena::new();
        let a = arena.alloc_str_arc("same");
        let b = arena.alloc_str_arc("same");
        let c = arena.alloc_str_arc("diff");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn arc_str_hash_depends_on_contents() {
        let arena = Arena::new();
        let a = arena.alloc_str_arc("foo");
        let b = arena.alloc_str_arc("foo");
        let c = arena.alloc_str_arc("bar");
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn arc_str_pointer_fmt_renders_some_address() {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("ptr");
        let rendered = format!("{s:p}");
        // Pointer formatting can produce either `0x…` or the bare hex form;
        // both have non-empty content with at least one hex digit.
        assert!(!rendered.is_empty());
        assert!(rendered.chars().any(|c| c.is_ascii_hexdigit()));
    }

    // --- Box<str> -----------------------------------------------------------

    #[test]
    fn box_str_as_ref_returns_actual_contents() {
        let arena = Arena::new();
        let s: Box<str> = arena.alloc_str_box("payload");
        let r: &str = s.as_ref();
        assert_eq!(r, "payload");
        assert_ne!(r, "");
        assert_ne!(r, "xyzzy");
    }

    #[test]
    fn box_str_partial_eq_self_distinguishes() {
        let arena = Arena::new();
        let a = arena.alloc_str_box("alpha");
        let b = arena.alloc_str_box("alpha");
        let c = arena.alloc_str_box("beta");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn box_str_partial_eq_str_true_and_false() {
        let arena = Arena::new();
        let s: Box<str> = arena.alloc_str_box("hi");
        let hi: std::string::String = "hi".to_owned();
        let bye: std::string::String = "bye".to_owned();
        assert_eq!(s, *hi.as_str());
        assert_ne!(s, *bye.as_str());
    }

    #[test]
    fn box_str_partial_eq_ref_str_true_and_false() {
        let arena = Arena::new();
        let s: Box<str> = arena.alloc_str_box("ok");
        assert_eq!(s, "ok");
        assert!((s != "no"));
    }

    // --- multitude::String --------------------------------------------------

    #[test]
    fn string_partial_eq_str_true_and_false() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("equal");
        let equal: std::string::String = "equal".to_owned();
        let other: std::string::String = "other".to_owned();
        assert_eq!(s, *equal.as_str());
        assert_ne!(s, *other.as_str());
    }

    #[test]
    fn string_partial_eq_ref_str_true_and_false() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("eq");
        assert_eq!(s, "eq");
        assert!((s != "neq"));
    }
}

mod arc_str_traits_coverage {
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
    #[cfg(feature = "utf16")]
    use widestring::utf16str;

    #[test]
    fn arc_str_partial_eq_str_and_ref_str() {
        let a = Arena::new();
        let s = a.alloc_str_arc("hello");
        // PartialEq<str>
        assert_eq!(s, *"hello");
        // PartialEq<&str>
        assert_eq!(s, "hello");
        let bad = "world";
        assert_ne!(s, *bad);
    }

    #[test]
    fn arc_str_pointer_fmt() {
        let a = Arena::new();
        let s = a.alloc_str_arc("p");
        let _ = format!("{s:p}");
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn arc_utf16_str_is_empty_and_eq_and_display() {
        let a = Arena::new();
        let s = a.alloc_utf16_str_arc(utf16str!(""));
        assert!(s.is_empty());
        let one = a.alloc_utf16_str_arc(utf16str!("x"));
        let two = a.alloc_utf16_str_arc(utf16str!("x"));
        assert_eq!(one, two);
        let _ = format!("{one}");
    }
}

mod box_str_into_box_u8_slice {
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
    use multitude::{Arena, Box as ArenaBox};

    #[test]
    fn from_box_str_to_box_u8_slice() {
        let a = Arena::new();
        let s = a.alloc_str_box("hello");
        let bytes: ArenaBox<[u8]> = ArenaBox::from(s);
        assert_eq!(&*bytes, b"hello");
    }
}
