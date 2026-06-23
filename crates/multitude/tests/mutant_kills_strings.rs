// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mutation-test kills for the string smart-pointer trait impls
//! (`Arc<str>`, `Box<str>`, `Arc<Utf16Str>`, `Box<Utf16Str>`, `String`,
//! `Utf16String`). Each block targets one or more mutants flagged by
//! `cargo mutants` as previously surviving — the asserts here are
//! specifically chosen to fail when the trait body is replaced with
//! its default-value or boolean-flipped form.

#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use multitude::{Arc, Arena, Box};

fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

// --- Arc<str> ---------------------------------------------------------------

#[test]
fn arc_str_partial_eq_ref_str_true_and_false() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("hello");
    // PartialEq<&str>: eq(&s, &"hello") == true
    assert!(s == "hello");
    // Mutated to `true` would still pass here; we need a `false` case too.
    assert!((s != "world"));
}

#[test]
fn arc_str_partial_eq_str_returns_actual_compare() {
    let arena = Arena::new();
    let s: Arc<str> = arena.alloc_str_arc("alpha");
    let alpha: std::string::String = "alpha".to_owned();
    let beta: std::string::String = "beta".to_owned();
    // PartialEq<str> using deref to coerce
    assert!(s == *alpha.as_str());
    assert!((s != *beta.as_str()));
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
    assert!(a == b);
    assert!((a != c));
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
    // Pointer formatting can produce either `0x…` (`*const T`) or
    // the bare hex form (`NonNull`/`Box`); both have non-empty
    // content with at least one hex digit.
    assert!(!rendered.is_empty());
    assert!(rendered.chars().any(|c| c.is_ascii_hexdigit()));
}

// --- Box<str> ---------------------------------------------------------------

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
    assert!(a == b);
    assert!((a != c));
}

#[test]
fn box_str_partial_eq_str_true_and_false() {
    let arena = Arena::new();
    let s: Box<str> = arena.alloc_str_box("hi");
    let hi: std::string::String = "hi".to_owned();
    let bye: std::string::String = "bye".to_owned();
    assert!(s == *hi.as_str());
    assert!((s != *bye.as_str()));
}

#[test]
fn box_str_partial_eq_ref_str_true_and_false() {
    let arena = Arena::new();
    let s: Box<str> = arena.alloc_str_box("ok");
    assert!(s == "ok");
    assert!((s != "no"));
}

// --- multitude::String ---------------------------------------------------

#[test]
fn string_partial_eq_str_true_and_false() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("equal");
    let equal: std::string::String = "equal".to_owned();
    let other: std::string::String = "other".to_owned();
    assert!(s == *equal.as_str());
    assert!((s != *other.as_str()));
}

#[test]
fn string_partial_eq_ref_str_true_and_false() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("eq");
    assert!(s == "eq");
    assert!((s != "neq"));
}

// --- Arc<Utf16Str> / Box<Utf16Str> / Utf16String -----------------------------

#[cfg(feature = "utf16")]
mod utf16_kills {
    use multitude::strings::Utf16String;
    use multitude::{Arc, Arena, Box, FromIn as _};
    use widestring::{Utf16Str, utf16str};

    use super::hash_of;

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
