// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the `std`-aligned API surface added to `Vec` / `String` /
//! `Utf16String`: `From`-based freezing, `leak`, `shrink_to`,
//! `extend_from_within`, `spare_capacity_mut`, `Index`/`IndexMut`,
//! `Add`/`AddAssign`, `AsRef`, `TryFrom`, `into_bytes`/`into_vec`,
//! `split_off`, and `reserve_exact`.

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::many_single_char_names, reason = "test code uses terse local bindings")]
#![allow(clippy::items_after_statements, reason = "test-local type aliases")]
#![allow(clippy::iter_on_single_items, reason = "tests exercise single-element iterators on purpose")]
#![allow(clippy::assertions_on_result_states, reason = "test code asserts Result states directly")]
#![allow(clippy::cast_possible_truncation, reason = "test code casts small, known-bounded counts")]

use multitude::vec::Vec as MVec;
use multitude::{Arc, Arena, Box as MBox};

#[test]
fn vec_freeze_via_from_traits() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3]);
    let b: MBox<[u32]> = MBox::from(v);
    assert_eq!(&*b, &[1, 2, 3]);

    let mut v2: MVec<'_, u32> = arena.alloc_vec();
    v2.extend([4, 5]);
    let a: Arc<[u32]> = Arc::from(v2);
    assert_eq!(&*a, &[4, 5]);
}

#[test]
fn vec_into_boxed_slice_and_leak() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([7, 8, 9]);
    let b = v.into_boxed_slice();
    assert_eq!(&*b, &[7, 8, 9]);

    let mut v2: MVec<'_, u32> = arena.alloc_vec();
    v2.extend([10, 20]);
    let leaked: &mut [u32] = v2.leak();
    leaked[0] = 11;
    assert_eq!(leaked, &[11, 20]);
}

#[test]
fn vec_try_into_arc_and_boxed_slice() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2]);
    let a = v.try_into_arc().unwrap();
    assert_eq!(&*a, &[1, 2]);

    let mut v2: MVec<'_, u32> = arena.alloc_vec();
    v2.extend([3, 4]);
    let b = v2.try_into_boxed_slice().unwrap();
    assert_eq!(&*b, &[3, 4]);
}

#[test]
fn vec_shrink_to_and_spare_capacity_mut() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec_with_capacity(16);
    v.extend([1, 2, 3]);
    v.shrink_to(8);
    assert!(v.capacity() >= 8);
    assert!(v.capacity() >= v.len());

    let spare = v.spare_capacity_mut();
    assert_eq!(spare.len(), v.capacity() - 3);
}

#[test]
fn vec_extend_from_within() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3, 4]);
    v.extend_from_within(1..3);
    assert_eq!(&*v, &[1, 2, 3, 4, 2, 3]);
}

#[test]
fn vec_index_and_as_ref() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([5, 6, 7]);
    assert_eq!(v[1], 6);
    assert_eq!(&v[1..], &[6, 7]);
    v[0] = 50;
    assert_eq!(v[0], 50);
    let r: &MVec<'_, u32> = v.as_ref();
    assert_eq!(r.len(), 3);
}

#[test]
fn vec_try_from_array() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3]);
    let arr: [u32; 3] = <[u32; 3]>::try_from(v).unwrap();
    assert_eq!(arr, [1, 2, 3]);

    let mut v2: MVec<'_, u32> = arena.alloc_vec();
    v2.extend([1, 2]);
    let err = <[u32; 3]>::try_from(v2);
    assert!(err.is_err());
    assert_eq!(err.unwrap_err().len(), 2);
}

#[test]
fn string_freeze_via_from_and_into_boxed_str() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hello");
    let b = MBox::<str>::from(s);
    assert_eq!(&*b, "hello");

    let mut s2 = arena.alloc_string();
    s2.push_str("world");
    let a = Arc::<str>::from(s2);
    assert_eq!(&*a, "world");

    let mut s3 = arena.alloc_string();
    s3.push_str("boxed");
    assert_eq!(&*s3.into_boxed_str(), "boxed");
}

#[test]
fn string_add_index_asref_leak() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("foo");
    s += "bar";
    assert_eq!(s.as_str(), "foobar");
    assert_eq!(&s[0..3], "foo");
    let bytes: &[u8] = s.as_ref();
    assert_eq!(bytes, b"foobar");

    let s2 = arena.alloc_string() + "leaked";
    let leaked: &mut str = s2.leak();
    assert_eq!(&*leaked, "leaked");
}

#[test]
fn string_into_bytes_split_off_reserve_exact() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hello world");
    let tail = s.split_off(5);
    assert_eq!(s.as_str(), "hello");
    assert_eq!(tail.as_str(), " world");

    let mut s2 = arena.alloc_string();
    s2.reserve_exact(32);
    assert!(s2.capacity() >= 32);
    s2.push_str("abc");
    let bytes = s2.into_bytes();
    assert_eq!(&*bytes, b"abc");
}

#[cfg(feature = "std")]
#[test]
fn string_as_ref_osstr_path() {
    use std::ffi::OsStr;
    use std::path::Path;
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("/tmp/x");
    let os: &OsStr = s.as_ref();
    assert_eq!(os, OsStr::new("/tmp/x"));
    let p: &Path = s.as_ref();
    assert_eq!(p, Path::new("/tmp/x"));
}

#[test]
fn string_extend_variants() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.extend([&'a', &'b']);
    s.extend([std::string::String::from("cd")]);
    s.extend([std::borrow::Cow::Borrowed("ef")]);
    assert_eq!(s.as_str(), "abcdef");
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_freeze_add_into_vec() {
    use multitude::strings::{ArcUtf16Str, BoxUtf16Str};
    use widestring::utf16str;
    let arena = Arena::new();

    let mut s = arena.alloc_utf16_string();
    s.push_str(utf16str!("hi"));
    let b = BoxUtf16Str::from(s);
    assert_eq!(&*b, utf16str!("hi"));

    let mut s2 = arena.alloc_utf16_string();
    s2.push_str(utf16str!("yo"));
    let a = ArcUtf16Str::from(s2);
    assert_eq!(&*a, utf16str!("yo"));

    let mut s3 = arena.alloc_utf16_string();
    s3.push_str(utf16str!("ab"));
    s3 += utf16str!("cd");
    assert_eq!(s3.as_utf16_str(), utf16str!("abcd"));
    let units = s3.into_vec();
    assert_eq!(&*units, utf16str!("abcd").as_slice());
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_split_off_index_extend() {
    use widestring::utf16str;
    let arena = Arena::new();

    let mut s = arena.alloc_utf16_string();
    s.push_str(utf16str!("hello world"));
    let tail = s.split_off(5);
    assert_eq!(s.as_utf16_str(), utf16str!("hello"));
    assert_eq!(tail.as_utf16_str(), utf16str!(" world"));

    // Range indexing (Output = Utf16Str).
    assert_eq!(&s[0..3], utf16str!("hel"));

    let mut e = arena.alloc_utf16_string();
    e.extend([&'a', &'b']);
    e.extend([std::string::String::from("cd")]);
    e.extend([std::borrow::Cow::Borrowed("ef")]);
    e.extend([std::boxed::Box::<str>::from("gh")]);
    assert_eq!(e.as_utf16_str(), utf16str!("abcdefgh"));

    // reserve_exact / shrink_to parity (shared shell).
    let mut r = arena.alloc_utf16_string();
    r.reserve_exact(32);
    assert!(r.capacity() >= 32);
}

// ---- Drop-correctness edge cases (Miri-validated) ----

use std::sync::atomic::{AtomicUsize, Ordering};

struct CountDrop<'c>(&'c AtomicUsize);
impl Drop for CountDrop<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn try_from_vec_array_drop_no_double_free() {
    let drops = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut v: MVec<'_, CountDrop<'_>> = arena.alloc_vec();
        v.push(CountDrop(&drops));
        v.push(CountDrop(&drops));
        let Ok(arr): Result<[CountDrop<'_>; 2], _> = <[CountDrop<'_>; 2]>::try_from(v) else {
            panic!("try_from should succeed for matching length");
        };
        assert_eq!(drops.load(Ordering::SeqCst), 0, "no drops while owned by the array");
        drop(arr);
        assert_eq!(drops.load(Ordering::SeqCst), 2, "exactly two drops from the array");
    }
    // Arena teardown must not re-drop the moved-out elements.
    assert_eq!(drops.load(Ordering::SeqCst), 2, "no double free at arena teardown");
}

#[test]
fn try_from_vec_array_length_mismatch_preserves_elements() {
    let drops = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut v: MVec<'_, CountDrop<'_>> = arena.alloc_vec();
        v.push(CountDrop(&drops));
        let Err(v) = <[CountDrop<'_>; 3]>::try_from(v) else {
            panic!("try_from should fail for length mismatch");
        };
        // The original element is still owned by the returned Vec.
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(v.len(), 1);
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1, "single element dropped exactly once");
}

#[test]
fn extend_from_within_clone_type_no_leak() {
    let arena = Arena::new();
    let mut v: MVec<'_, std::string::String> = arena.alloc_vec();
    v.push("a".to_string());
    v.push("b".to_string());
    v.extend_from_within(..);
    assert_eq!(v.len(), 4);
    assert_eq!(&v[2], "a");
    assert_eq!(&v[3], "b");
}

#[test]
#[should_panic(expected = "char boundary")]
fn string_split_off_non_char_boundary_panics() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push('é'); // 2 bytes
    let _ = s.split_off(1);
}

#[test]
fn vec_spare_capacity_mut_write_then_set_len() {
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec_with_capacity(4);
    {
        let spare = v.spare_capacity_mut();
        assert!(spare.len() >= 3);
        spare[0].write(10);
        spare[1].write(20);
        spare[2].write(30);
    }
    // SAFETY: 3 elements initialized above.
    unsafe { v.set_len(3) };
    assert_eq!(&*v, &[10, 20, 30]);
}

// ---- Coverage of the remaining new surface ----

#[test]
fn vec_as_mut_and_extend_from_within_bound_variants() {
    use core::ops::Bound;
    let arena = Arena::new();
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3, 4]);

    // AsMut<Self>.
    let m: &mut MVec<'_, u32> = v.as_mut();
    m.push(5);
    assert_eq!(v.len(), 5);

    // Inclusive-end bound (hits the `Included` end arm).
    let mut a: MVec<'_, u32> = arena.alloc_vec();
    a.extend([10, 20, 30]);
    a.extend_from_within(0..=1);
    assert_eq!(&*a, &[10, 20, 30, 10, 20]);

    // Excluded-start bound (hits the `Excluded` start arm).
    let mut b: MVec<'_, u32> = arena.alloc_vec();
    b.extend([7, 8, 9]);
    b.extend_from_within((Bound::Excluded(0usize), Bound::Unbounded));
    assert_eq!(&*b, &[7, 8, 9, 8, 9]);
}

#[test]
fn vec_extend_from_within_empty_range_does_not_reserve() {
    // An empty source range clones nothing, so `extend_from_within` must not
    // grow the backing buffer. This pins down the reserved `count = end - start`
    // (an inflated count would reallocate). The range and assertion are derived
    // from the *observed* capacity, so this is independent of growth policy.
    let arena = Arena::new();
    let mut b: MVec<'_, u32> = arena.alloc_vec();
    b.extend(0..16u32);
    let len = b.len();
    let cap = b.capacity();
    // `start == end` (empty), chosen so a count of `end + start` would exceed
    // the current capacity while the correct count of `0` leaves it untouched.
    let start = (cap - len) / 2 + 1;
    assert!(start <= len, "test precondition: spare capacity is small");
    let before: std::vec::Vec<u32> = b.as_slice().to_vec();
    b.extend_from_within(start..start);
    assert_eq!(b.capacity(), cap, "empty extend_from_within must not reserve");
    assert_eq!(b.as_slice(), &*before);
}

#[test]
fn string_index_mut_as_mut_vec_extend_box_reserve_shrink() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hello");

    // IndexMut → &mut str.
    let sub: &mut str = &mut s[0..2];
    sub.make_ascii_uppercase();
    assert_eq!(s.as_str(), "HEllo");

    // as_mut_vec (unsafe): append a valid ASCII byte.
    // SAFETY: pushing an ASCII byte keeps the buffer valid UTF-8.
    unsafe { s.as_mut_vec() }.push(b'!');
    assert_eq!(s.as_str(), "HEllo!");

    // Extend<Box<str>>.
    s.extend([std::boxed::Box::<str>::from("XY")]);
    assert_eq!(s.as_str(), "HEllo!XY");

    // try_reserve_exact + shrink_to (shared shell).
    s.try_reserve_exact(64).unwrap();
    assert!(s.capacity() >= s.len() + 64);
    s.shrink_to(0);
    assert!(s.capacity() >= s.len());
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_leak_as_mut_vec_asref_add_indexmut() {
    use widestring::utf16str;
    let arena = Arena::new();

    // Add (`+`, not `+=`).
    let s = arena.alloc_utf16_string() + utf16str!("ab");
    let added = s + utf16str!("cd");
    assert_eq!(added.as_utf16_str(), utf16str!("abcd"));

    // AsRef<[u16]>.
    let r: &[u16] = added.as_ref();
    assert_eq!(r, utf16str!("abcd").as_slice());

    // IndexMut → &mut Utf16Str.
    let mut m = arena.alloc_utf16_string();
    m.push_str(utf16str!("hello"));
    {
        let _sub: &mut widestring::Utf16Str = &mut m[0..2];
    }

    // as_mut_vec (unsafe): append a BMP unit.
    // SAFETY: U+0021 ('!') is a standalone BMP code unit, keeping UTF-16 valid.
    unsafe { m.as_mut_vec() }.push(0x0021);
    assert_eq!(m.len(), 6);

    // leak.
    let leaked: &mut widestring::Utf16Str = m.leak();
    assert_eq!(leaked.len(), 6);
}

// ---- FromIn / IntoIn ----

use multitude::{FromIn, IntoIn};

#[test]
fn vec_from_in_all_variants() {
    let arena = Arena::new();

    // &[T]
    let src = [1_u32, 2, 3];
    let a: MVec<'_, u32> = MVec::from_in(&src[..], &arena);
    assert_eq!(&*a, &[1, 2, 3]);

    // &mut [T]
    let mut srcm = [4_u32, 5];
    let b: MVec<'_, u32> = MVec::from_in(&mut srcm[..], &arena);
    assert_eq!(&*b, &[4, 5]);

    // [T; N]
    let c: MVec<'_, u32> = MVec::from_in([6_u32, 7, 8], &arena);
    assert_eq!(&*c, &[6, 7, 8]);

    // Box<[T]>
    let boxed: std::boxed::Box<[u32]> = std::boxed::Box::from([9_u32, 10]);
    let d: MVec<'_, u32> = MVec::from_in(boxed, &arena);
    assert_eq!(&*d, &[9, 10]);

    // Cow<[T]> borrowed + owned
    let e: MVec<'_, u32> = MVec::from_in(std::borrow::Cow::Borrowed(&src[..]), &arena);
    assert_eq!(&*e, &[1, 2, 3]);
    let owned: std::vec::Vec<u32> = vec![11, 12];
    let f: MVec<'_, u32> = MVec::from_in(std::borrow::Cow::Owned(owned), &arena);
    assert_eq!(&*f, &[11, 12]);

    // IntoIn companion (target type drives inference).
    let g: MVec<'_, u32> = [13_u32, 14].into_in(&arena);
    assert_eq!(&*g, &[13, 14]);
}

#[test]
fn into_in_allows_manual_impl_for_orphan_blocked_target() {
    // `std` lets you `impl Into<Foreign> for Local` when the corresponding
    // `From` is blocked by the orphan rule; the conditional blanket gives
    // `IntoIn` the same escape hatch for a concrete allocator. No one can
    // implement `std::string::String: FromIn<Celsius, Global>` (orphan rule),
    // so the blanket cannot apply to `(Celsius, String, Global)` and this
    // hand-written `IntoIn` impl does not collide with it.
    use allocator_api2::alloc::Global;

    struct Celsius(f64);
    impl<'a> IntoIn<'a, std::string::String, Global> for Celsius {
        fn into_in(self, _arena: &'a Arena<Global>) -> std::string::String {
            std::format!("{}F", self.0 * 9.0 / 5.0 + 32.0)
        }
    }

    let arena: Arena<Global> = Arena::new();
    let s: std::string::String = Celsius(100.0).into_in(&arena);
    assert_eq!(s, "212F");
}

#[test]
fn string_from_in_all_variants() {
    let arena = Arena::new();

    let a: multitude::strings::String<'_> = FromIn::from_in("abc", &arena);
    assert_eq!(a.as_str(), "abc");

    let c: multitude::strings::String<'_> = FromIn::from_in('Z', &arena);
    assert_eq!(c.as_str(), "Z");

    let mut owned = std::string::String::from("mut");
    let m: multitude::strings::String<'_> = FromIn::from_in(owned.as_mut_str(), &arena);
    assert_eq!(m.as_str(), "mut");

    let d: multitude::strings::String<'_> = FromIn::from_in(std::borrow::Cow::Borrowed("cow"), &arena);
    assert_eq!(d.as_str(), "cow");

    let e: multitude::strings::String<'_> = FromIn::from_in(std::boxed::Box::<str>::from("boxed"), &arena);
    assert_eq!(e.as_str(), "boxed");

    // IntoIn
    let f: multitude::strings::String<'_> = "into".into_in(&arena);
    assert_eq!(f.as_str(), "into");
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_from_in_all_variants() {
    use widestring::utf16str;
    let arena = Arena::new();

    let a: multitude::strings::Utf16String<'_> = FromIn::from_in(utf16str!("hi"), &arena);
    assert_eq!(a.as_utf16_str(), utf16str!("hi"));

    let b: multitude::strings::Utf16String<'_> = FromIn::from_in("u8", &arena);
    assert_eq!(b.as_utf16_str(), utf16str!("u8"));

    let c: multitude::strings::Utf16String<'_> = FromIn::from_in('Q', &arena);
    assert_eq!(c.as_utf16_str(), utf16str!("Q"));

    let d: multitude::strings::Utf16String<'_> = FromIn::from_in(std::borrow::Cow::Borrowed("cow"), &arena);
    assert_eq!(d.as_utf16_str(), utf16str!("cow"));

    let cow16: std::borrow::Cow<'_, widestring::Utf16Str> = std::borrow::Cow::Borrowed(utf16str!("w16"));
    let g: multitude::strings::Utf16String<'_> = FromIn::from_in(cow16, &arena);
    assert_eq!(g.as_utf16_str(), utf16str!("w16"));

    let e: multitude::strings::Utf16String<'_> = FromIn::from_in(std::boxed::Box::<str>::from("bx"), &arena);
    assert_eq!(e.as_utf16_str(), utf16str!("bx"));

    // IntoIn
    let f: multitude::strings::Utf16String<'_> = utf16str!("yo").into_in(&arena);
    assert_eq!(f.as_utf16_str(), utf16str!("yo"));
}

// ---- FromIteratorIn breadth (matching std's FromIterator set) ----

use multitude::vec::CollectIn;

#[test]
fn string_collect_in_all_item_types() {
    let arena = Arena::new();
    type S<'a> = multitude::strings::String<'a>;

    let a: S<'_> = ['a', 'b', 'c'].into_iter().collect_in(&arena);
    assert_eq!(a.as_str(), "abc");

    let chars = ['x', 'y'];
    let b: S<'_> = chars.iter().collect_in(&arena); // &char
    assert_eq!(b.as_str(), "xy");

    let c: S<'_> = ["fo", "ob", "ar"].into_iter().collect_in(&arena); // &str
    assert_eq!(c.as_str(), "foobar");

    let d: S<'_> = [std::string::String::from("hi"), std::string::String::from("!")]
        .into_iter()
        .collect_in(&arena);
    assert_eq!(d.as_str(), "hi!");

    let e: S<'_> = [std::boxed::Box::<str>::from("bo"), std::boxed::Box::<str>::from("x")]
        .into_iter()
        .collect_in(&arena);
    assert_eq!(e.as_str(), "box");

    let f: S<'_> = [
        std::borrow::Cow::Borrowed("co"),
        std::borrow::Cow::Owned(std::string::String::from("w")),
    ]
    .into_iter()
    .collect_in(&arena);
    assert_eq!(f.as_str(), "cow");
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_collect_in_all_item_types() {
    use widestring::utf16str;
    let arena = Arena::new();
    type U<'a> = multitude::strings::Utf16String<'a>;

    let a: U<'_> = ['a', 'b'].into_iter().collect_in(&arena);
    assert_eq!(a.as_utf16_str(), utf16str!("ab"));

    let chars = ['x', 'y'];
    let b: U<'_> = chars.iter().collect_in(&arena); // &char
    assert_eq!(b.as_utf16_str(), utf16str!("xy"));

    let c: U<'_> = ["fo", "ob"].into_iter().collect_in(&arena); // &str (transcode)
    assert_eq!(c.as_utf16_str(), utf16str!("foob"));

    let d: U<'_> = [utf16str!("u1"), utf16str!("6s")].into_iter().collect_in(&arena); // &Utf16Str
    assert_eq!(d.as_utf16_str(), utf16str!("u16s"));

    let e: U<'_> = [std::string::String::from("hi")].into_iter().collect_in(&arena);
    assert_eq!(e.as_utf16_str(), utf16str!("hi"));

    let f: U<'_> = [std::boxed::Box::<str>::from("bx")].into_iter().collect_in(&arena);
    assert_eq!(f.as_utf16_str(), utf16str!("bx"));

    let g: U<'_> = [std::borrow::Cow::Borrowed("cw")].into_iter().collect_in(&arena);
    assert_eq!(g.as_utf16_str(), utf16str!("cw"));
}

// ---- Arena string byte/unit constructors ----

#[test]
fn arena_string_from_utf8_variants() {
    let arena = Arena::new();

    let ok = arena.alloc_string_from_utf8(b"h\xC3\xA9llo").unwrap();
    assert_eq!(ok.as_str(), "héllo");

    let err = arena.alloc_string_from_utf8(&[0x66, 0xFF, 0x6F]);
    assert!(err.is_err());

    let lossy = arena.alloc_string_from_utf8_lossy(&[0x66, 0xFF, 0x6F]);
    assert_eq!(lossy.as_str(), "f\u{FFFD}o");

    // SAFETY: the literal is valid UTF-8.
    let unchecked = unsafe { arena.alloc_string_from_utf8_unchecked(b"abc") };
    assert_eq!(unchecked.as_str(), "abc");
}

#[test]
fn arena_string_from_utf16_variants() {
    let arena = Arena::new();

    // "ab" + U+10000 (surrogate pair D800 DC00).
    let units = [0x0061_u16, 0x0062, 0xD800, 0xDC00];
    let ok = arena.alloc_string_from_utf16(&units).unwrap();
    assert_eq!(ok.as_str(), "ab\u{10000}");

    // Unpaired high surrogate.
    let bad = [0x0061_u16, 0xD800, 0x0062];
    assert!(arena.alloc_string_from_utf16(&bad).is_err());

    let lossy = arena.alloc_string_from_utf16_lossy(&bad);
    assert_eq!(lossy.as_str(), "a\u{FFFD}b");
}

#[test]
fn arena_string_from_utf16_endian_variants() {
    let arena = Arena::new();

    // "ab" + U+10000 (D800 DC00) in little-endian bytes.
    let le = [0x61, 0x00, 0x62, 0x00, 0x00, 0xD8, 0x00, 0xDC];
    let s = arena.alloc_string_from_utf16le(&le).unwrap();
    assert_eq!(s.as_str(), "ab\u{10000}");

    // Same in big-endian.
    let be = [0x00, 0x61, 0x00, 0x62, 0xD8, 0x00, 0xDC, 0x00];
    let s = arena.alloc_string_from_utf16be(&be).unwrap();
    assert_eq!(s.as_str(), "ab\u{10000}");

    // Odd length → error.
    assert!(arena.alloc_string_from_utf16le(&[0x61, 0x00, 0x62]).is_err());
    // Unpaired surrogate (lone high surrogate 0xD800 LE) → error.
    let err = arena.alloc_string_from_utf16le(&[0x00, 0xD8]).unwrap_err();
    assert!(!err.to_string().is_empty());

    // Lossy: odd trailing byte and unpaired surrogate both → U+FFFD.
    let lossy = arena.alloc_string_from_utf16le_lossy(&[0x61, 0x00, 0x00, 0xD8, 0x62]);
    assert_eq!(lossy.as_str(), "a\u{FFFD}\u{FFFD}");
    let lossy_be = arena.alloc_string_from_utf16be_lossy(&[0x00, 0x61]);
    assert_eq!(lossy_be.as_str(), "a");
}

// ---- extend_from_within / into_flattened / drain / splice ----

#[test]
fn string_extend_from_within() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("abcd");
    s.extend_from_within(1..3);
    assert_eq!(s.as_str(), "abcdbc");
    s.extend_from_within(..);
    assert_eq!(s.as_str(), "abcdbcabcdbc");

    // Explicit Excluded-start / Included-end bounds.
    let mut s2 = arena.alloc_string();
    s2.push_str("abcd");
    s2.extend_from_within((core::ops::Bound::Excluded(0), core::ops::Bound::Included(2))); // 1..3 => "bc"
    assert_eq!(s2.as_str(), "abcdbc");
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_extend_from_within() {
    use widestring::utf16str;
    let arena = Arena::new();
    let mut s = arena.alloc_utf16_string();
    s.push_str(utf16str!("abcd"));
    s.extend_from_within(1..3);
    assert_eq!(s.as_utf16_str(), utf16str!("abcdbc"));

    // Unbounded start + Included end.
    let mut s2 = arena.alloc_utf16_string();
    s2.push_str(utf16str!("abcd"));
    s2.extend_from_within(..=1); // 0..2 => "ab"
    assert_eq!(s2.as_utf16_str(), utf16str!("abcdab"));

    // Excluded start + Unbounded end.
    let mut s3 = arena.alloc_utf16_string();
    s3.push_str(utf16str!("abcd"));
    s3.extend_from_within((core::ops::Bound::Excluded(1), core::ops::Bound::Unbounded)); // 2..4 => "cd"
    assert_eq!(s3.as_utf16_str(), utf16str!("abcdcd"));
}

#[test]
fn vec_into_flattened() {
    let arena = Arena::new();
    let mut v: MVec<'_, [u32; 3]> = arena.alloc_vec();
    v.push([1, 2, 3]);
    v.push([4, 5, 6]);
    let flat: MVec<'_, u32> = v.into_flattened();
    assert_eq!(&*flat, &[1, 2, 3, 4, 5, 6]);

    // ZST element type exercises the `usize::MAX` capacity branch.
    let mut zv: MVec<'_, [(); 2]> = arena.alloc_vec();
    zv.push([(), ()]);
    zv.push([(), ()]);
    let flatz: MVec<'_, ()> = zv.into_flattened();
    assert_eq!(flatz.len(), 4);

    // Drop type: every element dropped exactly once.
    let drops = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut v: MVec<'_, [CountDrop<'_>; 2]> = arena.alloc_vec();
        v.push([CountDrop(&drops), CountDrop(&drops)]);
        let flat = v.into_flattened();
        assert_eq!(flat.len(), 2);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

/// Collects a `char`-yielding drain iterator, panicking if it fails to
/// terminate within `max` items. Bounding the iteration means a mutated
/// `next`/`next_back` that never returns `None` (e.g. replaced with
/// `Some(Default::default())`) is caught as a fast failure instead of
/// hanging the test process.
fn bounded_chars<I: Iterator<Item = char>>(it: I, max: usize) -> std::string::String {
    let mut out = std::string::String::new();
    let mut n = 0usize;
    for c in it {
        n += 1;
        assert!(n <= max, "drain iterator failed to terminate within {max} items");
        out.push(c);
    }
    out
}

#[test]
fn string_drain_forward_back_and_removal() {
    let arena = Arena::new();

    let mut s = arena.alloc_string();
    s.push_str("héllo wörld");
    let drained = bounded_chars(s.drain(0..7), 16); // "héllo " (é is 2 bytes)
    assert_eq!(drained, "héllo ");
    assert_eq!(s.as_str(), "wörld");

    // Double-ended (reverse) consumption.
    let mut s2 = arena.alloc_string();
    s2.push_str("abçd");
    assert_eq!(bounded_chars(s2.drain(..).rev(), 16), "dçba");

    // Explicit forward `next` values (kills a `next -> Some(Default)` mutant
    // without iterating to completion); partial consumption still removes the
    // whole range.
    let mut s3 = arena.alloc_string();
    s3.push_str("0123456789");
    {
        let mut d = s3.drain(2..8);
        assert_eq!(d.next(), Some('2'));
        assert_eq!(d.next(), Some('3'));
    }
    assert_eq!(s3.as_str(), "0189");

    // Explicit backward `next_back` over a 4-byte char: exercises the
    // continuation-buffer fill (`buf[4 - n]`) at `n == 4`.
    let mut s4 = arena.alloc_string();
    s4.push_str("x\u{1F600}"); // 'x' + a 4-byte char
    {
        let mut d = s4.drain(..);
        assert_eq!(d.next_back(), Some('\u{1F600}'));
        assert_eq!(d.next_back(), Some('x'));
        assert_eq!(d.next_back(), None);
    }

    // 3-byte and 4-byte chars, full forward drain, plus `Debug`.
    let mut s5 = arena.alloc_string();
    s5.push_str("a€b\u{1F600}c"); // € is 3 bytes, U+1F600 is 4 bytes
    let d5 = s5.drain(..);
    let _ = format!("{d5:?}");
    assert_eq!(bounded_chars(d5, 16), "a€b\u{1F600}c");
    assert!(s5.as_str().is_empty());

    // `size_hint`: 8 remaining bytes => lower bound ceil(8/4)=2, upper 8.
    let mut s6 = arena.alloc_string();
    s6.push_str("abcdefgh");
    let d6 = s6.drain(..);
    assert_eq!(d6.size_hint(), (2, Some(8)));

    // Included-end (`..=`) and Excluded-start bounds (removal is eager).
    let mut s7 = arena.alloc_string();
    s7.push_str("0123456789");
    assert_eq!(bounded_chars(s7.drain(1..=3), 8), "123"); // bytes 1..4
    let mut s8 = arena.alloc_string();
    s8.push_str("0123456789");
    assert_eq!(
        bounded_chars(s8.drain((core::ops::Bound::Excluded(0), core::ops::Bound::Unbounded)), 16),
        "123456789"
    );
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_drain_forward_back() {
    use widestring::utf16str;
    let arena = Arena::new();

    let mut s = arena.alloc_utf16_string();
    s.push_str(utf16str!("ab")); // BMP
    s.push('\u{10000}'); // surrogate pair (2 units)
    s.push_str(utf16str!("cd"));
    // units: a b [hi lo] c d  => indices 0..6
    assert_eq!(bounded_chars(s.drain(2..4), 8), "\u{10000}"); // the U+10000 char
    assert_eq!(s.as_utf16_str(), utf16str!("abcd"));

    // Reverse over a string containing a surrogate pair.
    let mut s2 = arena.alloc_utf16_string();
    s2.push('x');
    s2.push('\u{1F600}'); // pair
    s2.push('y');
    assert_eq!(bounded_chars(s2.drain(..).rev(), 8), "y\u{1F600}x");

    // Explicit forward `next` over BMP chars (kills a `next -> Some(Default)`
    // mutant) plus `Debug`.
    let mut s3 = arena.alloc_utf16_string();
    s3.push_str(utf16str!("abc"));
    let mut d3 = s3.drain(..);
    let _ = format!("{d3:?}");
    assert_eq!(d3.next(), Some('a'));
    assert_eq!(d3.next(), Some('b'));
    assert_eq!(d3.next(), Some('c'));
    assert_eq!(d3.next(), None);

    // Explicit backward `next_back`.
    let mut s3b = arena.alloc_utf16_string();
    s3b.push_str(utf16str!("pq"));
    {
        let mut d = s3b.drain(..);
        assert_eq!(d.next_back(), Some('q'));
        assert_eq!(d.next_back(), Some('p'));
        assert_eq!(d.next_back(), None);
    }

    // `size_hint`: 6 remaining units => lower ceil(6/2)=3, upper 6.
    let mut s4 = arena.alloc_utf16_string();
    s4.push_str(utf16str!("abcdef"));
    let d4 = s4.drain(..);
    assert_eq!(d4.size_hint(), (3, Some(6)));

    // Included-end / Excluded-start bounds.
    let mut s5 = arena.alloc_utf16_string();
    s5.push_str(utf16str!("0123456789"));
    assert_eq!(bounded_chars(s5.drain(1..=3), 8), "123");
    let mut s6 = arena.alloc_utf16_string();
    s6.push_str(utf16str!("0123456789"));
    assert_eq!(
        bounded_chars(s6.drain((core::ops::Bound::Excluded(0), core::ops::Bound::Unbounded)), 16),
        "123456789"
    );
}

#[test]
fn vec_splice() {
    let arena = Arena::new();

    // Replace a middle range with more elements.
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3, 4, 5]);
    let removed: std::vec::Vec<u32> = v.splice(1..4, [20, 30, 40, 50]).collect();
    assert_eq!(removed, vec![2, 3, 4]);
    assert_eq!(&*v, &[1, 20, 30, 40, 50, 5]);

    // Empty range = pure insert; fewer replacements = net removal.
    let mut v2: MVec<'_, u32> = arena.alloc_vec();
    v2.extend([1, 2, 3]);
    assert!(v2.splice(1..1, [9]).next().is_none());
    assert_eq!(&*v2, &[1, 9, 2, 3]);

    let mut v3: MVec<'_, u32> = arena.alloc_vec();
    v3.extend([1, 2, 3, 4]);
    let r3: std::vec::Vec<u32> = v3.splice(1..3, core::iter::empty()).collect();
    assert_eq!(r3, vec![2, 3]);
    assert_eq!(&*v3, &[1, 4]);

    // Bound variants: Unbounded start + Included end.
    let mut v4: MVec<'_, u32> = arena.alloc_vec();
    v4.extend([1, 2, 3, 4, 5]);
    let r4: std::vec::Vec<u32> = v4.splice(..=1, [9]).collect(); // 0..2
    assert_eq!(r4, vec![1, 2]);
    assert_eq!(&*v4, &[9, 3, 4, 5]);

    // Excluded start + Unbounded end.
    let mut v5: MVec<'_, u32> = arena.alloc_vec();
    v5.extend([1, 2, 3, 4]);
    let r5: std::vec::Vec<u32> = v5
        .splice((core::ops::Bound::Excluded(0), core::ops::Bound::Unbounded), [7, 8])
        .collect(); // 1..4
    assert_eq!(r5, vec![2, 3, 4]);
    assert_eq!(&*v5, &[1, 7, 8]);

    // `Debug` + double-ended consumption.
    let mut v6: MVec<'_, u32> = arena.alloc_vec();
    v6.extend([1, 2, 3, 4]);
    let mut sp = v6.splice(0..3, core::iter::empty());
    let _ = format!("{sp:?}");
    assert_eq!(sp.next_back(), Some(3));
    assert_eq!(sp.next(), Some(1));
    drop(sp);
    assert_eq!(&*v6, &[4]);
}

#[test]
fn vec_splice_size_hint_and_reserve() {
    let arena = Arena::new();

    // `size_hint` reflects the number of removed elements.
    let mut v: MVec<'_, u32> = arena.alloc_vec();
    v.extend([1, 2, 3, 4, 5]);
    let sp = v.splice(1..4, core::iter::empty()); // removes 3
    assert_eq!(sp.size_hint(), (3, Some(3)));
    drop(sp);
    assert_eq!(&*v, &[1, 5]);

    // Lazy splice fills the replacement within the vector's own storage and
    // must not over-grow: replacing `capacity - 2` elements while keeping `2`
    // re-uses the existing capacity exactly (no realloc). Counts are derived
    // from the observed capacity, so this holds for any growth policy.
    let mut w: MVec<'_, u32> = arena.alloc_vec();
    w.extend(0..8u32);
    let cap = w.capacity();
    assert!(cap > 4);
    let r = cap - 2; // replacement count
    let end = w.len() - 2; // keep the last 2 elements (kept == 2)
    let removed: std::vec::Vec<u32> = w.splice(0..end, 0..r as u32).collect();
    assert_eq!(removed, (0..end as u32).collect::<std::vec::Vec<u32>>());
    let mut expected: std::vec::Vec<u32> = (0..r as u32).collect();
    expected.extend([6u32, 7]);
    assert_eq!(&*w, &*expected);
    assert_eq!(w.len(), cap); // start(0) + r + kept(2) == cap
    assert_eq!(w.capacity(), cap, "lazy splice must reuse capacity, not over-grow");
}

#[test]
fn arena_string_from_utf16_bytes_capacity_hints() {
    let arena = Arena::new();
    // 10 ASCII chars => 20 little-endian bytes, 10 code units, 10 UTF-8 bytes.
    let bytes: std::vec::Vec<u8> = "ABCDEFGHIJ".bytes().flat_map(|b| [b, 0]).collect();

    // Non-lossy: capacity hint is `bytes.len() / 2` == 10; the decoded ASCII
    // fills it exactly, so the (exact) preallocation leaves capacity == 10.
    let s = arena.alloc_string_from_utf16le(&bytes).unwrap();
    assert_eq!(s.as_str(), "ABCDEFGHIJ");
    assert_eq!(s.capacity(), 10);

    // Lossy: capacity hint is `bytes.len() / 2 + 1` == 11.
    let s_lossy = arena.alloc_string_from_utf16le_lossy(&bytes);
    assert_eq!(s_lossy.as_str(), "ABCDEFGHIJ");
    assert_eq!(s_lossy.capacity(), 11);
}

// ---- cross-type PartialEq (std-aligned bidirectional matrix) ----

#[test]
#[expect(clippy::op_ref, reason = "intentionally exercises the `&[U; N]` PartialEq impl")]
fn vec_cross_type_partial_eq() {
    use std::borrow::Cow;
    let arena = Arena::new();
    let mut v: MVec<'_, i32> = arena.alloc_vec();
    v.extend([1, 2, 3]);

    // Vec == Vec (cross-allocator/element impl, also serves as Self == Self).
    let mut v_eq: MVec<'_, i32> = arena.alloc_vec();
    v_eq.extend([1, 2, 3]);
    let mut v_ne: MVec<'_, i32> = arena.alloc_vec();
    v_ne.extend([9, 9]);
    assert!(v == v_eq);
    assert!(v != v_ne);

    // vs `[U]`, both directions.
    let s: &[i32] = &[1, 2, 3];
    let s_ne: &[i32] = &[1, 2];
    assert!(v == *s);
    assert!(*s == v);
    assert!(v != *s_ne);
    assert!(*s_ne != v);

    // vs `&[U]`, both directions.
    assert!(v == s);
    assert!(s == v);
    assert!(v != s_ne);
    assert!(s_ne != v);

    // vs `&mut [U]`, both directions.
    let mut buf_eq = [1, 2, 3];
    let mut buf_ne = [0, 0];
    {
        let m: &mut [i32] = &mut buf_eq;
        assert!(v == m);
        assert!(m == v);
    }
    {
        let m: &mut [i32] = &mut buf_ne;
        assert!(v != m);
        assert!(m != v);
    }

    // vs arrays `[U; N]` and `&[U; N]` (Vec-LHS only, mirroring std).
    assert!(v == [1, 2, 3]);
    assert!(v != [1, 2, 4]);
    assert!(v == &[1, 2, 3]);
    assert!(v != &[1, 2, 4]);

    // `Cow<[U]>` == Vec.
    let cow_eq: Cow<'_, [i32]> = Cow::Borrowed(&[1, 2, 3]);
    let cow_ne: Cow<'_, [i32]> = Cow::Owned(vec![7, 7]);
    assert!(cow_eq == v);
    assert!(cow_ne != v);
}

#[test]
fn string_cross_type_partial_eq() {
    use std::borrow::Cow;
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("hello");

    // vs `str`, both directions.
    assert!(s == *"hello");
    assert!(*"hello" == s);
    assert!(s != *"nope");
    assert!(*"nope" != s);

    // vs `&str`, both directions.
    assert!(s == "hello");
    assert!("hello" == s);
    assert!(s != "nope");
    assert!("nope" != s);

    // vs `Cow<str>`, both directions.
    let cow_eq: Cow<'_, str> = Cow::Borrowed("hello");
    let cow_ne: Cow<'_, str> = Cow::Owned(std::string::String::from("nope"));
    assert!(s == cow_eq);
    assert!(cow_eq == s);
    assert!(s != cow_ne);
    assert!(cow_ne != s);
}

#[cfg(feature = "utf16")]
#[test]
fn utf16_string_cross_type_partial_eq() {
    use widestring::utf16str;
    let arena = Arena::new();
    let mut s = arena.alloc_utf16_string();
    s.push_str(utf16str!("hello"));

    let eq = utf16str!("hello");
    let ne = utf16str!("nope");

    // vs `Utf16Str`, both directions.
    assert!(s == *eq);
    assert!(*eq == s);
    assert!(s != *ne);
    assert!(*ne != s);

    // vs `&Utf16Str`, both directions.
    assert!(s == eq);
    assert!(eq == s);
    assert!(s != ne);
    assert!(ne != s);
}
