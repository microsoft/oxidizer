// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    reason = "test code"
)]

//! Tests for the type-erasing APIs: turning sized owners into trait objects or
//! slices that stay in the pool, including pin-preserving `Arc`/`Rc`
//! conversions.

mod common;

use std::any::Any;
use std::array::from_fn;
use std::cell::Cell;
use std::fmt::Debug;
use std::marker::PhantomPinned;
use std::ops::DerefMut;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::ptr::{NonNull, addr_eq, from_ref, null};
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::thread;

use common::DropCounter;
use plurality::{Arc, Box, Coercion, Pool, Rc, coerce};
use static_assertions::assert_not_impl_any;

trait Shape {
    fn area(&self) -> u32;
}

trait Contains<T> {
    fn value(&self) -> &T;
}

trait PinnedValue: Send + Sync {
    fn value(&self) -> u32;
}

struct PinnedOwner {
    value: u32,
    drops: StdArc<AtomicUsize>,
    _pin: PhantomPinned,
}

impl PinnedValue for PinnedOwner {
    fn value(&self) -> u32 {
        self.value
    }
}

impl Drop for PinnedOwner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

assert_not_impl_any!(PinnedOwner: Unpin);

struct GenericValue<T>(T);

impl<T> Contains<T> for GenericValue<T> {
    fn value(&self) -> &T {
        &self.0
    }
}

struct Square(u32);
impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

struct Rect(u32, u32);
impl Shape for Rect {
    fn area(&self) -> u32 {
        self.0 * self.1
    }
}

#[test]
fn unsize_to_trait_object_dispatches_and_frees() {
    let pool = Pool::<Square>::new();
    let b = pool.alloc_box(Square(4));
    let s: Box<dyn Shape> = Box::unsize::<dyn Shape>(b, coerce!(dyn Shape));
    assert_eq!(s.area(), 16);
    assert_eq!(pool.len(), 1);
    drop(s);
    assert_eq!(pool.len(), 0);
}

#[test]
fn unsize_to_trait_object_with_generic_argument() {
    fn erase<T: 'static>(pool: &Pool<GenericValue<T>>, value: T) -> Box<dyn Contains<T>> {
        Box::unsize(pool.alloc_box(GenericValue(value)), coerce!(<T> dyn Contains<T>))
    }

    let pool = Pool::<GenericValue<u32>>::new();
    let value = erase(&pool, 42);
    assert_eq!(*value.value(), 42);
}

#[test]
fn heterogeneous_trait_objects_from_separate_pools() {
    // Different concrete types live in separate (differently-sized) pools, but
    // the erased handle type unifies them in one collection.
    let squares = Pool::<Square>::new();
    let rects = Pool::<Rect>::new();

    let shapes: Vec<Box<dyn Shape>> = vec![
        Box::unsize::<dyn Shape>(squares.alloc_box(Square(3)), coerce!(dyn Shape)),
        Box::unsize::<dyn Shape>(rects.alloc_box(Rect(2, 5)), coerce!(dyn Shape)),
    ];

    let total: u32 = shapes.iter().map(|s| s.area()).sum();
    assert_eq!(total, 9 + 10);

    drop(shapes);
    assert_eq!(squares.len(), 0);
    assert_eq!(rects.len(), 0);
}

#[test]
fn unsize_runs_concrete_destructor_through_vtable() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::new();
    let b = pool.alloc_box(DropCounter(StdArc::clone(&counter)));

    // Erase to a trait object with no methods; drop must still run
    // `DropCounter::drop` via the value's metadata and reclaim the slot.
    let erased: Box<dyn Send> = Box::unsize::<dyn Send>(b, coerce!(dyn Send));
    assert_eq!(pool.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    drop(erased);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn unsize_array_to_slice() {
    let pool = Pool::<[u8; 4]>::new();
    let b = pool.alloc_box([1u8, 2, 3, 4]);
    let mut s: Box<[u8]> = Box::unsize::<[u8]>(b, Coercion::to_slice());

    assert_eq!(s.len(), 4);
    assert_eq!(&*s, &[1, 2, 3, 4]);
    s[0] = 9;
    assert_eq!(&*s, &[9, 2, 3, 4]);
    assert_eq!(pool.len(), 1);

    drop(s);
    assert_eq!(pool.len(), 0);
}

#[test]
fn unsized_slices_drop_every_element_and_reclaim_slots() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<[DropCounter; 3]>::builder().chunk_size(1).max_chunks(1).build();

    let boxed = pool.alloc_box(from_fn(|_| DropCounter(StdArc::clone(&counter))));
    let slice: Box<[DropCounter]> = Box::unsize(boxed, Coercion::to_slice());
    drop(slice);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert_eq!(pool.len(), 0);

    let arc = pool.alloc_arc(from_fn(|_| DropCounter(StdArc::clone(&counter))));
    let slice: Arc<[DropCounter]> = Arc::unsize(arc, Coercion::to_slice());
    let clone = slice.clone();
    drop(slice);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    drop(clone);
    assert_eq!(counter.load(Ordering::SeqCst), 6);
    assert_eq!(pool.len(), 0);
}

#[test]
fn erased_debug_forwards_to_value() {
    let pool = Pool::<u32>::new();
    let d: Box<dyn Debug> = Box::unsize::<dyn Debug>(pool.alloc_box(7u32), coerce!(dyn Debug));
    assert_eq!(format!("{d:?}"), "7");
}

#[test]
fn coercion_debug_is_non_exhaustive() {
    assert_eq!(format!("{:?}", Coercion::<[u8; 1], [u8]>::to_slice()), "Coercion { .. }");
}

struct ImmediateReady(u32);
impl Future for ImmediateReady {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<u32> {
        Poll::Ready(self.0)
    }
}

#[test]
fn unsize_to_dyn_future_and_poll_via_as_pin_mut() {
    let pool = Pool::<ImmediateReady>::new();
    let b = pool.alloc_box(ImmediateReady(42));
    let mut fut: Box<dyn Future<Output = u32>> = Box::unsize::<dyn Future<Output = u32>>(b, coerce!(dyn Future<Output = u32>));

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match fut.as_pin_mut().poll(&mut cx) {
        Poll::Ready(v) => assert_eq!(v, 42),
        Poll::Pending => panic!("ImmediateReady must be ready"),
    }
    drop(fut);
    assert_eq!(pool.len(), 0);
}

#[test]
fn slot_reused_after_unsized_drop() {
    let pool = Pool::<u64>::builder().chunk_size(1).max_chunks(1).build();
    let b = pool.alloc_box(1u64);
    let d: Box<dyn Send + Sync> = Box::unsize::<dyn Send + Sync>(b, coerce!(dyn Send + Sync));
    drop(d);

    // The single capped slot must be reusable after the erased handle is freed.
    let b2 = pool.alloc_box(2u64);
    assert_eq!(*b2, 2);
}

#[test]
fn unsized_box_is_send_and_frees_cross_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<Box<dyn Send>>();

    let pool = Pool::<u32>::new();
    let d: Box<dyn Send> = Box::unsize::<dyn Send>(pool.alloc_box(5u32), coerce!(dyn Send));
    // Drop the erased handle on another thread (cross-thread free).
    thread::spawn(move || drop(d)).join().unwrap();
    assert_eq!(pool.len(), 0);
}

// ── Arc / Rc erasure ───────────────────────────────────────────────────────

#[test]
fn arc_unsize_shares_dispatches_and_frees() {
    let pool = Pool::<Square>::new();
    let a: Arc<dyn Shape> = Arc::unsize::<dyn Shape>(pool.alloc_arc(Square(5)), coerce!(dyn Shape));
    let a2 = a.clone();
    assert_eq!(a.area(), 25);
    assert_eq!(a2.area(), 25);
    assert_eq!(pool.len(), 1);
    drop(a);
    assert_eq!(pool.len(), 1); // still one clone alive
    drop(a2);
    assert_eq!(pool.len(), 0); // last clone freed the slot
}

#[test]
fn arc_unsize_runs_destructor_once_on_last_drop() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::new();
    let a: Arc<dyn Send> = Arc::unsize::<dyn Send>(pool.alloc_arc(DropCounter(StdArc::clone(&counter))), coerce!(dyn Send));
    let a2 = a.clone();
    drop(a);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(a2);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn arc_sized_and_erased_clones_share_one_slot() {
    // A sized `Arc<Square>` and an erased `Arc<dyn Shape>` clone can coexist on
    // one slot; whichever drops last frees it, reconstructing the layout either way.
    let pool = Pool::<Square>::new();
    let sized = pool.alloc_arc(Square(4));
    let erased: Arc<dyn Shape> = Arc::unsize::<dyn Shape>(sized.clone(), coerce!(dyn Shape));
    assert_eq!(sized.area(), 16);
    assert_eq!(erased.area(), 16);
    assert_eq!(pool.len(), 1);
    drop(erased);
    assert_eq!(pool.len(), 1);
    drop(sized);
    assert_eq!(pool.len(), 0);
}

#[test]
fn arc_unsize_to_slice() {
    let pool = Pool::<[u8; 3]>::new();
    let a: Arc<[u8]> = Arc::unsize::<[u8]>(pool.alloc_arc([7u8, 8, 9]), Coercion::to_slice());
    assert_eq!(&*a, &[7, 8, 9]);
    assert_eq!(a.len(), 3);
    drop(a);
    assert_eq!(pool.len(), 0);
}

#[test]
fn arc_erased_is_send_sync_and_frees_cross_thread() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn Send + Sync>>();

    let pool = Pool::<u32>::new();
    let a: Arc<dyn Send + Sync> = Arc::unsize::<dyn Send + Sync>(pool.alloc_arc(9u32), coerce!(dyn Send + Sync));
    let a2 = a.clone();
    thread::spawn(move || drop(a2)).join().unwrap();
    drop(a);
    assert_eq!(pool.len(), 0);
}

#[test]
fn rc_unsize_shares_dispatches_and_frees() {
    let pool = Pool::<Square>::new();
    let r: Rc<dyn Shape> = Rc::unsize::<dyn Shape>(pool.alloc_rc(Square(6)), coerce!(dyn Shape));
    let r2 = r.clone();
    assert_eq!(r.area(), 36);
    assert_eq!(r2.area(), 36);
    assert_eq!(pool.len(), 1);
    drop(r);
    assert_eq!(pool.len(), 1);
    drop(r2);
    assert_eq!(pool.len(), 0);
}

#[test]
fn rc_unsize_runs_destructor_once_on_last_drop() {
    let counter = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<DropCounter>::new();
    let r: Rc<dyn Any> = Rc::unsize::<dyn Any>(pool.alloc_rc(DropCounter(StdArc::clone(&counter))), coerce!(dyn Any));
    let r2 = r.clone();
    drop(r2);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(r);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn arc_unsize_pin_preserves_address_and_pinned_clones() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<PinnedOwner>::new();
    let pinned = pool.alloc_arc_pin(PinnedOwner {
        value: 11,
        drops: StdArc::clone(&drops),
        _pin: PhantomPinned,
    });
    let sized_clone = pinned.clone();
    let address = from_ref(pinned.as_ref().get_ref());

    let erased: Pin<Arc<dyn PinnedValue>> = Arc::unsize_pin(pinned, coerce!(dyn PinnedValue));
    let erased_clone = erased.clone();

    assert!(addr_eq(address, from_ref(erased.as_ref().get_ref())));
    assert!(addr_eq(address, from_ref(erased_clone.as_ref().get_ref())));
    assert!(addr_eq(address, from_ref(sized_clone.as_ref().get_ref())));
    assert_eq!(erased.value(), 11);
    assert_eq!(pool.len(), 1);

    drop((sized_clone, erased));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(erased_clone);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn rc_unsize_pin_preserves_address_and_pinned_clones() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let pool = Pool::<PinnedOwner>::new();
    let pinned = pool.alloc_rc_pin(PinnedOwner {
        value: 12,
        drops: StdArc::clone(&drops),
        _pin: PhantomPinned,
    });
    let sized_clone = pinned.clone();
    let address = from_ref(pinned.as_ref().get_ref());

    let erased: Pin<Rc<dyn PinnedValue>> = Rc::unsize_pin(pinned, coerce!(dyn PinnedValue));
    let erased_clone = erased.clone();

    assert!(addr_eq(address, from_ref(erased.as_ref().get_ref())));
    assert!(addr_eq(address, from_ref(erased_clone.as_ref().get_ref())));
    assert!(addr_eq(address, from_ref(sized_clone.as_ref().get_ref())));
    assert_eq!(erased.value(), 12);
    assert_eq!(pool.len(), 1);

    drop((sized_clone, erased));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(erased_clone);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len(), 0);
}

#[test]
fn panicking_coercion_does_not_leak_any_handle() {
    fn coercion() -> Coercion<u32, dyn Debug, impl FnOnce(*const u32) -> *const dyn Debug> {
        // SAFETY: this deliberately panicking token never returns a pointer; it
        // exercises unwind behavior before any conversion can occur.
        unsafe { Coercion::new(|_| -> *const dyn Debug { panic!("coercion panic") }) }
    }

    let box_pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = Box::unsize::<dyn Debug>(box_pool.alloc_box(1), coercion());
        }))
        .is_err()
    );
    assert_eq!(box_pool.len(), 0);
    drop(box_pool.try_alloc_box(2).unwrap());

    let arc_pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = Arc::unsize::<dyn Debug>(arc_pool.alloc_arc(1), coercion());
        }))
        .is_err()
    );
    assert_eq!(arc_pool.len(), 0);
    drop(arc_pool.try_alloc_arc(2).unwrap());

    let rc_pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = Rc::unsize::<dyn Debug>(rc_pool.alloc_rc(1), coercion());
        }))
        .is_err()
    );
    assert_eq!(rc_pool.len(), 0);
    drop(rc_pool.try_alloc_rc(2).unwrap());
}

struct AddressSensitive {
    pinned_at: Cell<*const Self>,
    _pin: PhantomPinned,
}

impl AddressSensitive {
    fn new() -> Self {
        Self {
            pinned_at: Cell::new(null()),
            _pin: PhantomPinned,
        }
    }

    fn initialize(self: Pin<&mut Self>) {
        self.pinned_at.set(from_ref(self.as_ref().get_ref()));
    }
}

impl Drop for AddressSensitive {
    fn drop(&mut self) {
        assert_eq!(self.pinned_at.get(), from_ref(self));
    }
}

assert_not_impl_any!(Box<AddressSensitive>: DerefMut);

#[test]
fn pin_borrow_keeps_address_stable() {
    let pool = Pool::<AddressSensitive>::new();
    let mut value = pool.alloc_box(AddressSensitive::new());
    value.as_pin_mut().initialize();
    let first = NonNull::from(&*value).as_ptr();
    let second = NonNull::from(&*value.as_pin_mut()).as_ptr();
    assert_eq!(first, second);
    drop(value);
    assert_eq!(pool.len(), 0);
}

#[repr(align(64))]
struct OverAligned(u8);

trait ByteValue {
    fn value(&self) -> u8;
}

impl ByteValue for OverAligned {
    fn value(&self) -> u8 {
        self.0
    }
}

struct ZeroSized;

impl ByteValue for ZeroSized {
    fn value(&self) -> u8 {
        0
    }
}

#[test]
fn erased_zst_and_overaligned_values_reclaim_correctly() {
    let aligned_pool = Pool::<OverAligned>::builder().chunk_size(1).max_chunks(1).build();
    let aligned: Box<dyn ByteValue> = Box::unsize::<dyn ByteValue>(aligned_pool.alloc_box(OverAligned(7)), coerce!(dyn ByteValue));
    assert_eq!(aligned.value(), 7);
    drop(aligned);
    drop(aligned_pool.try_alloc_box(OverAligned(8)).unwrap());

    let zst_pool = Pool::<ZeroSized>::builder().chunk_size(1).max_chunks(1).build();
    let zst: Box<dyn ByteValue> = Box::unsize::<dyn ByteValue>(zst_pool.alloc_box(ZeroSized), coerce!(dyn ByteValue));
    assert_eq!(zst.value(), 0);
    drop(zst);
    drop(zst_pool.try_alloc_box(ZeroSized).unwrap());
}

#[test]
fn unsized_raw_round_trips_preserve_metadata() {
    let shape_pool = Pool::<Square>::new();
    let shape: Box<dyn Shape> = Box::unsize::<dyn Shape>(shape_pool.alloc_box(Square(4)), coerce!(dyn Shape));
    let shape_raw = Box::into_raw(shape);
    // SAFETY: `shape_raw` is the unchanged pointer just returned by `into_raw`.
    let shape: Box<dyn Shape> = unsafe { Box::from_raw(shape_raw) };
    assert_eq!(shape.area(), 16);
    drop(shape);

    let slice_pool = Pool::<[u8; 3]>::new();
    let slice: Box<[u8]> = Box::unsize::<[u8]>(slice_pool.alloc_box([1, 2, 3]), Coercion::to_slice());
    let slice_raw = Box::into_raw(slice);
    // SAFETY: `slice_raw` is the unchanged pointer just returned by `into_raw`.
    let slice: Box<[u8]> = unsafe { Box::from_raw(slice_raw) };
    assert_eq!(&*slice, &[1, 2, 3]);
    drop(slice);
}
