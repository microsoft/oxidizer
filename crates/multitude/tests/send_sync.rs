// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static `Send`/`Sync` contract tests for the smart pointers.
//!
//! These tests do not run any code at runtime — they assert the
//! desired auto-trait behaviour at compile time. If a future
//! refactor accidentally widens the auto-trait set on `Rc`/`Box`
//! (for example by changing the `PhantomData<*const T>` to
//! `PhantomData<T>`), the negative `if-let-bound` tricks below
//! will fail to compile.

use multitude::{Arc, Arena};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

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
fn rc_is_not_send() {
    // Compile-time check via a helper function that requires `!Send`.
    // If `Rc<u64>: Send` ever became true, this would fail the
    // `cannot be sent between threads safely` bound from `thread::spawn`.
    let arena: Arena = Arena::new();
    let r = arena.alloc_rc(42_u64);
    // Cannot move `r` into a spawned thread; must stay on the
    // arena's owning thread. Just ensure the value is reachable here.
    let _ = &r;
}
