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
    let mut slot = arena.alloc_uninit::<u64>();
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
    let mut s = arena.alloc_uninit_slice::<u32>(4);
    assert_eq!(s.len(), 4);
    for (i, e) in s.iter_mut().enumerate() {
        e.write(i as u32);
    }
    // Read each element back through the stable per-element `assume_init_ref`
    // (the slice-level `MaybeUninit::slice_assume_init_ref` is still unstable),
    // avoiding a slice type-punning cast and keeping the safety scope per slot.
    for (i, e) in s.iter().enumerate() {
        // SAFETY: every slot was initialized by the loop above.
        assert_eq!(unsafe { *e.assume_init_ref() }, i as u32);
    }
}

#[test]
fn alloc_zeroed_slice_is_zero() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice::<u16>(5);
    assert_eq!(s.len(), 5);
    for e in s.iter() {
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
        assert_eq!(boxed.as_widestring_utf16_str(), utf16str!("HI"));
    }

    #[test]
    fn utf16_into_arc_ok_and_try() {
        let arena = Arena::new();
        let s = Utf16String::from_utf16_str_in(utf16str!("shared"), &arena);
        let a = s.into_arc_utf16_str();
        assert_eq!(a.as_widestring_utf16_str(), utf16str!("shared"));

        let s2 = Utf16String::from_utf16_str_in(utf16str!("again"), &arena);
        let a2 = s2.try_into_arc_utf16_str().unwrap();
        assert_eq!(a2.as_widestring_utf16_str(), utf16str!("again"));
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

mod alloc_panics_on_failing_allocator {
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;
    #[cfg(feature = "utf16")]
    use widestring::utf16str;

    use crate::common::{FailingAllocator, SyncFailingAllocator};

    fn fa() -> Arena<FailingAllocator> {
        Arena::new_in(FailingAllocator::new(0))
    }
    fn sfa() -> Arena<SyncFailingAllocator> {
        Arena::new_in(SyncFailingAllocator::new(0))
    }

    #[test]
    fn alloc_value_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_arc(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_arc_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_arc_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_box(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_box_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_box_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone_arc(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy_arc(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with_arc::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter_arc::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone_box(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy_box(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with_box::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter_box::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str_arc("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str_box("abc");
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_arc(utf16str!("abc"));
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_box(utf16str!("abc"));
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_from_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_arc_from_str("abc");
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_from_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_box_from_str("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_arc::<D>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_arc::<D>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_slice_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_slice_arc::<D>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_slice_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_slice_arc::<D>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_box::<u32>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_box::<u32>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_slice_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_slice_box::<u32>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_slice_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_slice_box::<u32>(2);
        }));
        assert!(r.is_err());
    }
}

mod try_alloc_returns_err_on_failing_allocator {
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

    use crate::common::SyncFailingAllocator;

    fn fa() -> Arena<SyncFailingAllocator> {
        Arena::new_in(SyncFailingAllocator::new(0))
    }

    #[test]
    fn try_alloc_value_err() {
        let a = fa();
        assert!(a.try_alloc(0_u32).is_err());
    }
    #[test]
    fn try_alloc_with_err() {
        let a = fa();
        assert!(a.try_alloc_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_arc_err() {
        let a = fa();
        assert!(a.try_alloc_arc(0_u32).is_err());
    }
    #[test]
    fn try_alloc_arc_with_err() {
        let a = fa();
        assert!(a.try_alloc_arc_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_box_err() {
        let a = fa();
        assert!(a.try_alloc_box(0_u32).is_err());
    }
    #[test]
    fn try_alloc_box_with_err() {
        let a = fa();
        assert!(a.try_alloc_box_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_arc_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone_arc(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy_arc(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with_arc::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter_arc::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_box_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone_box(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy_box(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with_box::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter_box::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_str_err() {
        let a = fa();
        assert!(a.try_alloc_str("abc").is_err());
    }
    #[test]
    fn try_alloc_str_arc_err() {
        let a = fa();
        assert!(a.try_alloc_str_arc("abc").is_err());
    }
    #[test]
    fn try_alloc_str_box_err() {
        let a = fa();
        assert!(a.try_alloc_str_box("abc").is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_arc_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_arc(utf16str!("abc")).is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_box_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_box(utf16str!("abc")).is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_arc_from_str_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_arc_from_str("abc").is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_box_from_str_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_box_from_str("abc").is_err());
    }
    #[test]
    fn try_alloc_uninit_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_uninit_arc::<D>().is_err());
    }
    #[test]
    fn try_alloc_zeroed_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_zeroed_arc::<D>().is_err());
    }
    #[test]
    fn try_alloc_uninit_slice_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_uninit_slice_arc::<D>(2).is_err());
    }
    #[test]
    fn try_alloc_zeroed_slice_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_zeroed_slice_arc::<D>(2).is_err());
    }
    #[test]
    fn try_alloc_uninit_box_err() {
        let a = fa();
        assert!(a.try_alloc_uninit_box::<u32>().is_err());
    }
    #[test]
    fn try_alloc_zeroed_box_err() {
        let a = fa();
        assert!(a.try_alloc_zeroed_box::<u32>().is_err());
    }
    #[test]
    fn try_alloc_uninit_slice_box_err() {
        let a = fa();
        assert!(a.try_alloc_uninit_slice_box::<u32>(2).is_err());
    }
    #[test]
    fn try_alloc_zeroed_slice_box_err() {
        let a = fa();
        assert!(a.try_alloc_zeroed_slice_box::<u32>(2).is_err());
    }
}

mod uninit_drop_init_from_iter {
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
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[derive(Clone)]
    struct Counted(StdArc<AtomicUsize>);
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn alloc_slice_fill_iter_drop_type_runs_drop_when_handle_drops() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let a = Arena::new();
            let _handle = a.alloc_slice_fill_iter((0..4).map(|_| Counted(counter.clone())));
            assert_eq!(counter.load(Ordering::Relaxed), 0);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 4);
    }
}

mod zeroed_slice_arc_zeroes_payload {
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
    use core::mem::MaybeUninit;

    use multitude::Arena;

    #[test]
    fn zeroed_slice_arc_drop_type_zeroes_payload() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let slice = a.alloc_zeroed_slice_arc::<D>(3);
        // SAFETY: the slice is freshly zeroed; reading the raw bytes is
        // well defined for any zero pattern.
        unsafe {
            let bytes = core::slice::from_raw_parts(slice.as_ptr() as *const MaybeUninit<u8>, core::mem::size_of::<D>() * 3);
            for b in bytes {
                assert_eq!(b.assume_init(), 0);
            }
        }
    }
}
