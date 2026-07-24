// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static `Send` and `Sync` contract tests.

use core::alloc::Layout;
use core::cell::Cell;
use core::ptr::NonNull;
use std::thread;

use allocator_api2::alloc::{Allocator, Global};
use multitude::{Alloc, Arc, Arena, Box, Rc};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

// Overlapping blanket impls make `probe` ambiguous when a type implements the
// trait under test.
trait AmbiguousIfSend<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
fn assert_not_send<T: ?Sized>() {
    let _ = <T as AmbiguousIfSend<_>>::probe;
}

trait AmbiguousIfSync<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfSync<()> for T {}
impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
fn assert_not_sync<T: ?Sized>() {
    let _ = <T as AmbiguousIfSync<_>>::probe;
}

#[derive(Clone, Default)]
struct SendOnlyAllocator(Cell<()>);

// SAFETY: allocation and deallocation forward unchanged to `Global`.
unsafe impl Allocator for SendOnlyAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded under the caller's Allocator contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

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
    thread::scope(|s| {
        let h = s.spawn(move || {
            let x = arena.alloc(99_u64);
            *x
        });
        assert_eq!(h.join().unwrap(), 99);
    });
}

#[test]
fn arena_is_not_send_when_allocator_is_not_sync() {
    assert_not_send::<Arena<SendOnlyAllocator>>();
    assert_not_send::<Arc<u64, SendOnlyAllocator>>();
    assert_not_send::<Box<u64, SendOnlyAllocator>>();
}

#[test]
fn arc_is_send_sync_when_t_is() {
    assert_send::<Arc<u64>>();
    assert_sync::<Arc<u64>>();
    assert_send::<Arc<[u8]>>();
    assert_sync::<Arc<[u8]>>();
    let arena: Arena = Arena::new();
    let a = arena.alloc_arc(42_u64);
    thread::scope(|s| {
        let h = s.spawn(move || *a);
        assert_eq!(h.join().unwrap(), 42);
    });
}

#[test]
fn box_is_send_sync_when_t_is() {
    // `Box<T, A>` mirrors `std::Box`: `Send` when `T: Send` and
    // `A: Send`; `Sync` when `T: Sync` and `A: Sync`. The backing
    // storage uses an atomic-refcounted chunk, so dropping the
    // `Box` on a different thread is sound.
    assert_send::<Box<u64>>();
    assert_sync::<Box<u64>>();
    assert_send::<Box<[u8]>>();
    assert_sync::<Box<[u8]>>();

    let arena: Arena = Arena::new();
    let b = arena.alloc_box(42_u64);
    thread::scope(|s| {
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
    thread::scope(|s| {
        let h = s.spawn(move || b.len());
        assert_eq!(h.join().unwrap(), 5);
    });
}

#[test]
fn arc_str_is_send_sync() {
    assert_send::<Arc<str>>();
    assert_sync::<Arc<str>>();
}

#[test]
fn alloc_send_sync_follow_t() {
    use core::cell::Cell;
    // `Alloc<'a, T>` wraps `&'a mut T`, so it inherits the auto traits of
    // `&mut T`: `Send` when `T: Send` (`impl<T: Send + ?Sized> Send for &mut T`)
    // and `Sync` when `T: Sync` (`impl<T: Sync + ?Sized> Sync for &mut T` — a
    // shared `&&mut T` only permits *reading* through it, so sharing is sound
    // when `T: Sync`). These assertions lock that contract in.
    assert_send::<Alloc<'static, u64>>();
    assert_sync::<Alloc<'static, u64>>();
    assert_send::<Alloc<'static, [u8]>>();
    assert_sync::<Alloc<'static, [u8]>>();
    // `Cell<T>` is `Send` but `!Sync`, so `Alloc<Cell<_>>` must follow suit —
    // proving `Sync` genuinely tracks `T: Sync` rather than being unconditional.
    assert_send::<Alloc<'static, Cell<u64>>>();
    assert_not_sync::<Alloc<'static, Cell<u64>>>();
}

#[test]
fn rc_is_not_send_or_sync() {
    // `Rc<T, A>` uses a *non-atomic*, unaligned per-handle strong count
    // (read/written with `read_unaligned`/`write_unaligned`), so it must never
    // cross threads. It carries no `unsafe impl Send`/`Sync`, and its
    // `NonNull<u8>` + `PhantomData<(*const T, A)>` fields keep it `!Send` and
    // `!Sync` automatically for every `T`/`A`. This test fails to compile if a
    // future refactor accidentally makes `Rc` `Send` or `Sync` — which would
    // be unsound, since two threads could then race the non-atomic count.
    assert_not_send::<Rc<u64>>();
    assert_not_sync::<Rc<u64>>();
    assert_not_send::<Rc<[u8]>>();
    assert_not_sync::<Rc<[u8]>>();
    assert_not_send::<Rc<str>>();
    assert_not_sync::<Rc<str>>();
}

#[test]
fn drain_and_splice_are_not_send_or_sync() {
    // `Vec` is thread-affine (`!Send`/`!Sync`): its `drain`/`splice`
    // iterators borrow it mutably and restore the surviving tail in `Drop`.
    // They must inherit that `!Send`/`!Sync` — otherwise a live `Drain`
    // could migrate to another thread and run the in-place tail restore
    // from a foreign thread, which is unsound. The `PhantomData<&'d mut
    // Vec<..>>` marker locks this in; this test fails to compile if a future
    // refactor accidentally makes either iterator `Send`/`Sync`.
    use allocator_api2::alloc::Global;
    use multitude::vec::{Drain, Splice};

    assert_not_send::<Drain<'static, 'static, u64, Global>>();
    assert_not_sync::<Drain<'static, 'static, u64, Global>>();
    assert_not_send::<Splice<'static, 'static, core::iter::Empty<u64>, Global>>();
    assert_not_sync::<Splice<'static, 'static, core::iter::Empty<u64>, Global>>();
}
