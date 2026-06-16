// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the local uninit/zeroed family, the `try_*` fallible
//! counterparts of panicking allocation APIs, and the string `Arc` freezes.

#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
#![allow(clippy::assertions_on_result_states, reason = "tests assert Result states directly")]
#![allow(clippy::cast_possible_truncation, reason = "test code casts small known-bounded indices")]

mod common;

use common::{FailingAllocator, SendFailingAllocator};
use multitude::Arena;
use multitude::vec::Vec as MVec;

// ===========================================================================
// A. Local uninit / zeroed family
// ===========================================================================

#[test]
fn alloc_uninit_write_read() {
    let arena = Arena::new();
    let slot = arena.alloc_uninit::<u64>();
    let r = slot.write(42);
    assert_eq!(*r, 42);
}

#[test]
fn alloc_zeroed_is_zero() {
    let arena = Arena::new();
    let slot = arena.alloc_zeroed::<u64>();
    assert_eq!(unsafe { *slot.assume_init_ref() }, 0);
}

#[test]
fn alloc_uninit_slice_write_read() {
    let arena = Arena::new();
    let s = arena.alloc_uninit_slice::<u32>(4);
    assert_eq!(s.len(), 4);
    for (i, e) in s.iter_mut().enumerate() {
        e.write(i as u32);
    }
    let init: &[u32] = unsafe { &*(std::ptr::from_ref(s) as *const [u32]) };
    assert_eq!(init, &[0, 1, 2, 3]);
}

#[test]
fn alloc_zeroed_slice_is_zero() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice::<u16>(5);
    assert_eq!(s.len(), 5);
    for e in s {
        assert_eq!(unsafe { *e.assume_init_ref() }, 0);
    }
}

#[test]
fn try_uninit_family_ok() {
    let arena = Arena::new();
    assert!(arena.try_alloc_uninit::<u64>().is_ok());
    assert!(arena.try_alloc_zeroed::<u64>().is_ok());
    assert!(arena.try_alloc_uninit_slice::<u32>(3).is_ok());
    assert!(arena.try_alloc_zeroed_slice::<u32>(3).is_ok());
}

#[test]
fn try_uninit_family_errors_on_alloc_failure() {
    let arena = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc_uninit::<u64>().is_err());
    assert!(arena.try_alloc_zeroed::<u64>().is_err());
    assert!(arena.try_alloc_uninit_slice::<u32>(3).is_err());
    assert!(arena.try_alloc_zeroed_slice::<u32>(3).is_err());
}

#[test]
#[should_panic(expected = "AllocError")]
fn alloc_uninit_panics_on_alloc_failure() {
    let arena = Arena::new_in(FailingAllocator::new(0));
    let _ = arena.alloc_uninit::<u64>();
}

// ===========================================================================
// B. Arena `try_` string decoders (clean: lossy / unchecked)
// ===========================================================================

#[test]
fn try_from_utf8_lossy_ok_and_err() {
    let arena = Arena::new();
    let s = arena.try_alloc_string_from_utf8_lossy(b"ab\xFFc").unwrap();
    assert_eq!(s.as_str(), "ab\u{FFFD}c");

    let arena = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc_string_from_utf8_lossy(b"abc").is_err());
}

#[test]
fn try_from_utf8_unchecked_ok_and_err() {
    let arena = Arena::new();
    let s = unsafe { arena.try_alloc_string_from_utf8_unchecked(b"hi").unwrap() };
    assert_eq!(s.as_str(), "hi");

    let arena = Arena::new_in(FailingAllocator::new(0));
    assert!(unsafe { arena.try_alloc_string_from_utf8_unchecked(b"hi") }.is_err());
}

#[test]
fn try_from_utf16_lossy_ok_and_err() {
    let arena = Arena::new();
    let units = [0x0048u16, 0x0069]; // "Hi"
    let s = arena.try_alloc_string_from_utf16_lossy(&units).unwrap();
    assert_eq!(s.as_str(), "Hi");
    // Unpaired surrogate -> replacement char.
    let bad = [0xD800u16];
    let s2 = arena.try_alloc_string_from_utf16_lossy(&bad).unwrap();
    assert_eq!(s2.as_str(), "\u{FFFD}");

    let arena = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc_string_from_utf16_lossy(&units).is_err());
}

#[test]
fn try_from_utf16le_be_lossy_ok_and_err() {
    let arena = Arena::new();
    // "Hi" little-endian and big-endian.
    let le = [0x48u8, 0x00, 0x69, 0x00];
    let be = [0x00u8, 0x48, 0x00, 0x69];
    assert_eq!(arena.try_alloc_string_from_utf16le_lossy(&le).unwrap().as_str(), "Hi");
    assert_eq!(arena.try_alloc_string_from_utf16be_lossy(&be).unwrap().as_str(), "Hi");
    // Odd trailing byte -> trailing replacement char.
    let odd = [0x48u8, 0x00, 0x69];
    assert_eq!(arena.try_alloc_string_from_utf16le_lossy(&odd).unwrap().as_str(), "H\u{FFFD}");

    let arena = Arena::new_in(FailingAllocator::new(0));
    assert!(arena.try_alloc_string_from_utf16le_lossy(&le).is_err());
    assert!(arena.try_alloc_string_from_utf16be_lossy(&be).is_err());
}

#[test]
fn try_from_utf16_bytes_lossy_reserves_exact_capacity() {
    // Four ASCII units (eight bytes) decode to four UTF-8 bytes, so the
    // `bytes.len() / 2 + 1` capacity hint reserves exactly five bytes and no
    // growth occurs. Any arithmetic deviation in the hint changes the observed
    // capacity (too-small hints grow to a different value; too-large hints
    // over-reserve), so an exact-capacity assertion pins the expression down.
    let le = [0x74u8, 0x00, 0x65, 0x00, 0x73, 0x00, 0x74, 0x00]; // "test"
    let be = [0x00u8, 0x74, 0x00, 0x65, 0x00, 0x73, 0x00, 0x74]; // "test"

    let arena = Arena::new();
    let s = arena.try_alloc_string_from_utf16le_lossy(&le).unwrap();
    assert_eq!(s.as_str(), "test");
    assert_eq!(s.capacity(), 5);

    let arena = Arena::new();
    let s = arena.try_alloc_string_from_utf16be_lossy(&be).unwrap();
    assert_eq!(s.as_str(), "test");
    assert_eq!(s.capacity(), 5);
}

// ===========================================================================
// C. Vec `try_*`
// ===========================================================================

#[test]
fn vec_try_insert_resize_append_split_extend_ok() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.try_insert(0, 1).unwrap();
    v.try_insert(1, 3).unwrap();
    v.try_insert(1, 2).unwrap();
    assert_eq!(v.as_slice(), &[1, 2, 3]);

    v.try_resize(5, 9).unwrap();
    assert_eq!(v.as_slice(), &[1, 2, 3, 9, 9]);
    v.try_resize_with(6, || 7).unwrap();
    assert_eq!(v.as_slice(), &[1, 2, 3, 9, 9, 7]);

    let mut other: MVec<'_, u32> = arena.alloc_vec();
    other.try_insert(0, 100).unwrap();
    v.try_append(&mut other).unwrap();
    assert_eq!(v.as_slice().last(), Some(&100));
    assert!(other.is_empty());

    v.try_extend_from_within(0..2).unwrap();
    assert_eq!(&v.as_slice()[v.len() - 2..], &[1, 2]);

    let tail = v.try_split_off(3).unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(v.as_slice(), &[1, 2, 3]);
    assert!(!tail.is_empty());
}

#[test]
fn vec_try_methods_error_on_alloc_failure() {
    // Allow only the first chunk so the Vec exists but growth fails.
    let arena = Arena::new_in(FailingAllocator::new(1));
    let mut v = arena.alloc_vec::<u32>();
    // Fill the first chunk so the next growth needs a (failing) refill.
    while v.try_insert(v.len(), 0).is_ok() {}
    assert!(v.try_insert(0, 1).is_err());
    assert!(v.try_resize(v.len() + 1000, 0).is_err());
    assert!(v.try_resize_with(v.len() + 1000, || 0).is_err());
    assert!(v.try_extend_from_within(0..1).is_err());

    let arena2 = Arena::new_in(FailingAllocator::new(0));
    let mut a = arena2.alloc_vec::<u32>();
    let mut b = arena2.alloc_vec::<u32>();
    assert!(a.try_insert(0, 1).is_err());
    // `try_split_off` on an empty vec takes the zero-copy/empty path and
    // performs no allocation, so it succeeds even here.
    assert!(b.try_split_off(0).is_ok());
    assert!(a.try_append(&mut b).is_ok()); // both empty -> no alloc
}

#[test]
#[should_panic(expected = "AllocError")]
fn vec_insert_panics_on_alloc_failure() {
    let arena = Arena::new_in(FailingAllocator::new(1));
    let mut v = arena.alloc_vec::<u32>();
    // Bounded so a regression that stops `insert` from panicking fails the
    // test quickly instead of hanging the suite; the allocator fails well
    // within the cap.
    for _ in 0..1 << 20 {
        v.insert(v.len(), 0);
    }
}

// ===========================================================================
// D. String `try_*`
// ===========================================================================

#[test]
fn string_try_methods_ok() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.try_insert_str(0, "world").unwrap();
    s.try_insert(0, ' ').unwrap();
    s.try_insert_str(0, "hello").unwrap();
    assert_eq!(s.as_str(), "hello world");

    s.try_replace_range(0..5, "HELLO").unwrap();
    assert_eq!(s.as_str(), "HELLO world");

    s.try_extend_from_within(0..5).unwrap();
    assert_eq!(s.as_str(), "HELLO worldHELLO");

    let tail = s.try_split_off(5).unwrap();
    assert_eq!(s.as_str(), "HELLO");
    assert_eq!(tail.as_str(), " worldHELLO");

    let boxed = s.try_into_boxed_str().unwrap();
    assert_eq!(&*boxed, "HELLO");
}

#[test]
fn string_try_methods_error_on_alloc_failure() {
    let arena = Arena::new_in(FailingAllocator::new(1));
    let mut s = arena.alloc_string();
    // Bounded so a buggy `try_push` that never reports failure cannot hang the
    // suite; `FailingAllocator` exhausts well within the cap.
    for _ in 0..1 << 20 {
        if s.try_push('x').is_err() {
            break;
        }
    }
    assert!(s.try_insert(0, 'y').is_err());
    assert!(s.try_insert_str(0, "yy").is_err());
    assert!(s.try_replace_range(0..0, "yy").is_err());
    assert!(s.try_extend_from_within(0..1).is_err());

    let arena2 = Arena::new_in(FailingAllocator::new(0));
    let mut s2 = arena2.alloc_string();
    assert!(s2.try_insert_str(0, "a").is_err());
    // `split_off` on an empty string performs no allocation -> Ok.
    assert!(s2.try_split_off(0).is_ok());
    let s3 = arena2.alloc_string();
    assert!(s3.try_into_boxed_str().is_err());
}

#[test]
fn string_into_arc_str_ok_and_try() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("shared");
    let a = s.into_arc_str();
    assert_eq!(&*a, "shared");

    let mut s2 = arena.alloc_string();
    s2.push_str("again");
    let a2 = s2.try_into_arc_str().unwrap();
    assert_eq!(&*a2, "again");

    // From impl routes through into_arc_str.
    let mut s3 = arena.alloc_string();
    s3.push_str("via_from");
    let a3: multitude::Arc<str> = s3.into();
    assert_eq!(&*a3, "via_from");
}

#[test]
fn string_try_into_arc_str_error() {
    let arena = Arena::new_in(SendFailingAllocator::new(0));
    let s = arena.alloc_string();
    assert!(s.try_into_arc_str().is_err());
}

// ===========================================================================
// E. Utf16String `try_*` + Arc freeze (feature-gated)
// ===========================================================================

#[cfg(feature = "utf16")]
mod utf16_tests {
    use multitude::strings::Utf16String;
    use widestring::utf16str;

    use super::{Arena, FailingAllocator, SendFailingAllocator};

    #[test]
    fn utf16_try_methods_ok() {
        let arena = Arena::new();
        let mut s = Utf16String::try_from_utf16_str_in(utf16str!("world"), &arena).unwrap();
        s.try_insert(0, ' ').unwrap();
        s.try_insert_utf16_str(0, utf16str!("hi")).unwrap();
        assert_eq!(s.as_utf16_str(), utf16str!("hi world"));

        s.try_replace_range(0..2, utf16str!("HI")).unwrap();
        assert_eq!(s.as_utf16_str(), utf16str!("HI world"));

        s.try_extend_from_within(0..2).unwrap();
        assert_eq!(s.as_utf16_str(), utf16str!("HI worldHI"));

        let tail = s.try_split_off(2).unwrap();
        assert_eq!(s.as_utf16_str(), utf16str!("HI"));
        assert_eq!(tail.as_utf16_str(), utf16str!(" worldHI"));

        let boxed = s.try_into_boxed_utf16_str().unwrap();
        assert_eq!(boxed.as_utf16_str(), utf16str!("HI"));
    }

    #[test]
    fn utf16_into_arc_ok_and_try() {
        let arena = Arena::new();
        let s = Utf16String::from_utf16_str_in(utf16str!("shared"), &arena);
        let a = s.into_arc_utf16_str();
        assert_eq!(a.as_utf16_str(), utf16str!("shared"));

        let s2 = Utf16String::from_utf16_str_in(utf16str!("again"), &arena);
        let a2 = s2.try_into_arc_utf16_str().unwrap();
        assert_eq!(a2.as_utf16_str(), utf16str!("again"));
    }

    #[test]
    fn utf16_try_methods_error_on_alloc_failure() {
        let arena = Arena::new_in(FailingAllocator::new(0));
        assert!(Utf16String::try_from_utf16_str_in(utf16str!("x"), &arena).is_err());

        let arena1 = Arena::new_in(FailingAllocator::new(1));
        let mut s = Utf16String::from_utf16_str_in(utf16str!(""), &arena1);
        // Bounded so a buggy `try_push` that never reports failure cannot hang
        // the suite; `FailingAllocator` exhausts well within the cap.
        for _ in 0..1 << 20 {
            if s.try_push('x').is_err() {
                break;
            }
        }
        assert!(s.try_insert(0, 'y').is_err());
        assert!(s.try_insert_utf16_str(0, utf16str!("y")).is_err());
        assert!(s.try_replace_range(0..0, utf16str!("y")).is_err());
        assert!(s.try_extend_from_within(0..1).is_err());

        let arena0 = Arena::new_in(FailingAllocator::new(0));
        let mut s0 = Utf16String::from_utf16_str_in(utf16str!(""), &arena0);
        assert!(s0.try_split_off(0).is_ok()); // empty -> no alloc
        let s0b = Utf16String::from_utf16_str_in(utf16str!(""), &arena0);
        assert!(s0b.try_into_boxed_utf16_str().is_err());
        let arena0s = Arena::new_in(SendFailingAllocator::new(0));
        let s0c = Utf16String::from_utf16_str_in(utf16str!(""), &arena0s);
        assert!(s0c.try_into_arc_utf16_str().is_err());
    }
}
