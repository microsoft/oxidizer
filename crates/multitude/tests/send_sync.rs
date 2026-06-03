// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static `Send`/`Sync` contract tests for the smart pointers.
//!
//! These tests do not run any code at runtime — they assert the
//! desired auto-trait behaviour at compile time. They will fail to
//! compile if a future refactor either narrows the auto-trait set
//! (e.g. by introducing a `!Send` field) or widens it past the
//! intended bounds.

use multitude::{Arc, Arena, Box};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

#[test]
fn arena_is_send() {
    // `Arena: Send` is intended to be auto-derived from its fields.
    // Lock that contract here so a future field addition that
    // accidentally introduces a `!Send` type fails to compile.
    // `Arena` is intentionally `!Sync` (interior `RefCell` /
    // `UnsafeCell`), so only `Send` is asserted.
    assert_send::<Arena>();

    // And a runtime cross-thread move:
    let arena: Arena = Arena::new();
    let _ = arena.alloc(7_u64);
    std::thread::scope(|s| {
        let h = s.spawn(move || {
            let x = arena.alloc(99_u64);
            *x
        });
        assert_eq!(h.join().unwrap(), 99);
    });
}

#[test]
fn arc_is_send_sync_when_t_is() {
    assert_send::<Arc<u64>>();
    assert_sync::<Arc<u64>>();
    assert_send::<Arc<[u8]>>();
    assert_sync::<Arc<[u8]>>();
    let arena: Arena = Arena::new();
    let a = arena.alloc_arc(42_u64);
    std::thread::scope(|s| {
        let h = s.spawn(move || *a);
        assert_eq!(h.join().unwrap(), 42);
    });
}

#[test]
fn box_is_send_sync_when_t_is() {
    // `Box<T, A>` mirrors `std::Box`: `Send` when `T: Send` and
    // `A: Send`; `Sync` when `T: Sync` and `A: Sync`. The backing
    // storage uses an atomic-refcounted shared chunk, so dropping the
    // `Box` on a different thread is sound.
    assert_send::<Box<u64>>();
    assert_sync::<Box<u64>>();
    assert_send::<Box<[u8]>>();
    assert_sync::<Box<[u8]>>();

    let arena: Arena = Arena::new();
    let b = arena.alloc_box(42_u64);
    std::thread::scope(|s| {
        let h = s.spawn(move || *b);
        assert_eq!(h.join().unwrap(), 42);
    });
}

#[test]
fn box_str_is_send_sync() {
    // `Box<str, A>` is `Send` when `A: Send` and `Sync` when `A: Sync`
    // (`str` is itself `Send + Sync`).
    assert_send::<Box<str>>();
    assert_sync::<Box<str>>();

    let arena: Arena = Arena::new();
    let b = arena.alloc_str_box("hello");
    std::thread::scope(|s| {
        let h = s.spawn(move || b.len());
        assert_eq!(h.join().unwrap(), 5);
    });
}

#[test]
fn arc_str_is_send_sync() {
    assert_send::<Arc<str>>();
    assert_sync::<Arc<str>>();
}
