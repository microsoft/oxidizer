// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the bytes integration module.

#![cfg(feature = "bytes")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "test assertions use direct indexing on known-length buffers"
)]

use bytes::Bytes;
use multitude::Arena;

#[test]
fn arc_bytes_into_bytes() {
    let arena = Arena::new();
    let data: &[u8] = b"hello world";
    let arc = arena.alloc_slice_copy_arc(data);
    let b: Bytes = arc.into();
    assert_eq!(&*b, b"hello world");
}

#[test]
fn arc_bytes_empty() {
    let arena = Arena::new();
    let data: &[u8] = b"";
    let arc = arena.alloc_slice_copy_arc(data);
    let b: Bytes = arc.into();
    assert!(b.is_empty());
}

#[test]
fn arc_bytes_large() {
    let arena = Arena::new();
    let data: Vec<u8> = (0u16..1024).map(|i| u8::try_from(i % 256).unwrap()).collect();
    let arc = arena.alloc_slice_copy_arc(data.as_slice());
    let b: Bytes = arc.into();
    assert_eq!(b.len(), 1024);
    assert_eq!(b[0], 0);
    assert_eq!(b[255], 255);
    assert_eq!(b[256], 0);
}

#[test]
fn arc_bytes_preserves_content() {
    let arena = Arena::new();
    let arc = arena.alloc_slice_copy_arc(b"exact content" as &[u8]);
    let b: Bytes = arc.into();
    assert_eq!(&*b, b"exact content");
}

#[test]
fn arc_bytes_slicing() {
    let arena = Arena::new();
    let arc = arena.alloc_slice_copy_arc(b"hello world" as &[u8]);
    let b: Bytes = arc.into();
    let sub = b.slice(6..11);
    assert_eq!(&*sub, b"world");
}

#[test]
fn arc_bytes_clone_is_shallow() {
    let arena = Arena::new();
    let arc = arena.alloc_slice_copy_arc(b"shared data" as &[u8]);
    let b: Bytes = arc.into();
    let b2 = b.clone();
    assert_eq!(&*b, &*b2);
    assert_eq!(b.as_ptr(), b2.as_ptr());
}

#[test]
fn arc_str_into_bytes() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("hello");
    let b: Bytes = s.into();
    assert_eq!(&*b, b"hello");
}

#[test]
fn arc_str_empty() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("");
    let b: Bytes = s.into();
    assert!(b.is_empty());
}

#[test]
fn arc_str_unicode() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("café ☕");
    let expected = "café ☕".as_bytes();
    let b: Bytes = s.into();
    assert_eq!(&*b, expected);
}

#[test]
fn arc_str_large() {
    let arena = Arena::new();
    let text = "x".repeat(4096);
    let s = arena.alloc_str_arc(&text);
    let b: Bytes = s.into();
    assert_eq!(b.len(), 4096);
    assert!(b.iter().all(|&c| c == b'x'));
}

#[test]
fn arc_str_slicing() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("hello world");
    let b: Bytes = s.into();
    let sub = b.slice(6..11);
    assert_eq!(&*sub, b"world");
}

#[test]
fn bytes_send_across_threads() {
    let arena = Arena::new();
    let arc = arena.alloc_slice_copy_arc(b"threaded" as &[u8]);
    let b: Bytes = arc.into();
    let h = std::thread::spawn(move || b.len());
    assert_eq!(h.join().unwrap(), 8);
}

#[test]
fn bytes_from_arc_str_send_across_threads() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("threaded");
    let b: Bytes = s.into();
    let h = std::thread::spawn(move || b.len());
    assert_eq!(h.join().unwrap(), 8);
}

#[test]
fn bytes_outlives_arena() {
    let b: Bytes;
    {
        let arena = Arena::new();
        let arc = arena.alloc_slice_copy_arc(b"persist" as &[u8]);
        b = arc.into();
    }
    // Arena is dropped; Bytes should still be valid
    assert_eq!(&*b, b"persist");
}

#[test]
fn bytes_from_str_outlives_arena() {
    let b: Bytes;
    {
        let arena = Arena::new();
        let s = arena.alloc_str_arc("persist");
        b = s.into();
    }
    assert_eq!(&*b, b"persist");
}
