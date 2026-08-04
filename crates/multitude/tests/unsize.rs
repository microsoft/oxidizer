// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Trait-object coercion and hybrid handle-layout tests.

#![cfg(feature = "dst")]
#![allow(
    clippy::allow_attributes,
    clippy::multiple_unsafe_ops_per_block,
    clippy::unwrap_used,
    reason = "test code"
)]

use core::alloc::Layout;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use multitude::{Arc, Arena, Box, Coercion, Rc, SmartPointerPointee, coerce};

#[multitude::dst::pointee]
trait Shape: Send + Sync {
    fn area(&self) -> u32;
}

#[multitude::dst::pointee]
trait Named: Send + Sync {
    fn name(&self) -> &'static str;
}

#[multitude::dst::pointee]
trait Contains<T> {
    fn value(&self) -> &T;
}

#[multitude::dst::pointee]
trait PinnedValue: Send + Sync {
    fn value(&self) -> u32;
}

struct Square {
    side: u32,
    drops: Option<StdArc<AtomicUsize>>,
}

struct UnitShape;

impl Shape for UnitShape {
    fn area(&self) -> u32 {
        1
    }
}

#[repr(align(64))]
struct AlignedShape(u32);

impl Shape for AlignedShape {
    fn area(&self) -> u32 {
        self.0
    }
}

impl Shape for Square {
    fn area(&self) -> u32 {
        self.side * self.side
    }
}

impl Named for Square {
    fn name(&self) -> &'static str {
        "square"
    }
}

impl Drop for Square {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct GenericValue<T>(T);

impl<T> Contains<T> for GenericValue<T> {
    fn value(&self) -> &T {
        &self.0
    }
}

// A handler that borrows its input. The handler owns nothing, so it is `'static`
// no matter how short-lived the values it is invoked with are.
#[multitude::dst::pointee]
trait Handler<T> {
    fn handle(&self, input: T) -> usize;
}

struct LengthHandler;

impl<'a> Handler<&'a str> for LengthHandler {
    fn handle(&self, input: &'a str) -> usize {
        input.len()
    }
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

#[repr(C)]
#[derive(ptr_meta::Pointee)]
struct HeaderAndShape {
    header: u32,
    shape: dyn Shape,
}

#[allow(dead_code, reason = "compile-time covariance assertion")]
fn assert_box_dyn_covariant<'a, 'b: 'a>(value: Box<dyn Shape + 'b>) -> Box<dyn Shape + 'a> {
    value
}

#[allow(dead_code, reason = "compile-time covariance assertion")]
fn assert_arc_dyn_covariant<'a, 'b: 'a>(value: Arc<dyn Shape + 'b>) -> Arc<dyn Shape + 'a> {
    value
}

#[allow(dead_code, reason = "compile-time covariance assertion")]
fn assert_rc_dyn_covariant<'a, 'b: 'a>(value: Rc<dyn Shape + 'b>) -> Rc<dyn Shape + 'a> {
    value
}

#[test]
fn coercion_metadata_contracts_are_observable() {
    // SAFETY: the closure performs only the compiler's pointer unsizing
    // coercion from `Square` to `dyn Shape`.
    let coercion: Coercion<Square, dyn Shape, _> = unsafe { Coercion::new(|ptr: *const Square| -> *const dyn Shape { ptr }) };
    assert_eq!(format!("{coercion:?}"), "Coercion { .. }");

    let square = Square { side: 2, drops: None };
    let erased: *const dyn Shape = &square;
    let stored = ptr_meta::metadata(erased);
    let thin = NonNull::from(&square).cast();

    // SAFETY: `thin` and `stored` came from the same live `Square`.
    let resolved = unsafe { <dyn Shape as SmartPointerPointee>::resolve_metadata(thin, stored) };
    assert_eq!(resolved, stored);

    let result = catch_unwind(|| <dyn Shape as SmartPointerPointee>::metadata_from_allocation(thin));
    let _ = result.unwrap_err();
}

#[test]
fn box_unsize_dispatches_and_drops_once() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();
    let value = arena.alloc_box(Square {
        side: 4,
        drops: Some(StdArc::clone(&drops)),
    });

    let erased: Box<dyn Shape> = Box::unsize(value, coerce!(dyn Shape));
    assert_eq!(erased.area(), 16);
    drop(erased);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn differently_coerced_arc_clones_coexist() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();
    let sized = arena.alloc_arc(Square {
        side: 3,
        drops: Some(StdArc::clone(&drops)),
    });
    let shape: Arc<dyn Shape> = Arc::unsize(sized.clone(), coerce!(dyn Shape));
    let named: Arc<dyn Named> = Arc::unsize(sized.clone(), coerce!(dyn Named));

    assert_eq!(shape.area(), 9);
    assert_eq!(named.name(), "square");
    assert_eq!(sized.side, 3);

    drop(shape);
    drop(named);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(sized);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn sized_and_erased_rc_clones_share_one_count() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();
    let sized = arena.alloc_rc(Square {
        side: 5,
        drops: Some(StdArc::clone(&drops)),
    });
    let erased: Rc<dyn Shape> = Rc::unsize(sized.clone(), coerce!(dyn Shape));

    assert_eq!(erased.area(), 25);
    drop(sized);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(erased);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn generic_trait_object_coercion_dispatches() {
    fn erase<T: 'static>(arena: &Arena, value: T) -> Box<dyn Contains<T>> {
        Box::unsize(arena.alloc_box(GenericValue(value)), coerce!(<T> dyn Contains<T>))
    }

    let arena = Arena::new();
    let value = erase(&arena, 42_u32);
    assert_eq!(*value.value(), 42);
}

// A trait object's lifetime bound constrains the erased concrete type, not the
// trait's type arguments. `'a` is universally quantified here, so this compiles
// only if coercion imposes no `'static` requirement on the type argument.
fn erase_handler<'a>(arena: &Arena) -> Box<dyn Handler<&'a str>> {
    fn erase<H: Handler<T> + 'static, T>(arena: &Arena, handler: H) -> Box<dyn Handler<T>> {
        Box::unsize(arena.alloc_box(handler), coerce!(<T> dyn Handler<T>))
    }

    erase::<LengthHandler, &'a str>(arena, LengthHandler)
}

#[test]
fn generic_trait_object_coercion_accepts_borrowed_type_argument() {
    let arena = Arena::new();
    let owned = String::from("borrowed");
    let handler = erase_handler(&arena);

    assert_eq!(handler.handle(owned.as_str()), 8);
}

#[test]
fn pinned_arc_and_rc_coercions_preserve_pin() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();

    let arc = arena.alloc_arc_pin(PinnedOwner {
        value: 7,
        drops: StdArc::clone(&drops),
        _pin: PhantomPinned,
    });
    let arc: Pin<Arc<dyn PinnedValue>> = Arc::unsize_pin(arc, coerce!(dyn PinnedValue));
    assert_eq!(arc.value(), 7);

    let rc = arena.alloc_rc_pin(PinnedOwner {
        value: 11,
        drops: StdArc::clone(&drops),
        _pin: PhantomPinned,
    });
    let rc: Pin<Rc<dyn PinnedValue>> = Rc::unsize_pin(rc, coerce!(dyn PinnedValue));
    assert_eq!(rc.value(), 11);

    drop(arc);
    drop(rc);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
#[expect(clippy::cast_ptr_alignment, reason = "the supplied layout guarantees Square alignment")]
fn direct_trait_object_allocations_use_handle_metadata() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let metadata = ptr_meta::metadata(&Square { side: 0, drops: None } as &dyn Shape);
    let layout = Layout::new::<Square>();
    let arena = Arena::new();

    // SAFETY: `layout` and `metadata` describe `Square` as `dyn Shape`, and
    // the initializer writes one valid `Square` into the supplied storage.
    let arc: Arc<dyn Shape> = unsafe {
        arena.alloc_dst_arc(layout, metadata, |ptr: *mut dyn Shape| {
            ptr.cast::<Square>().write(Square {
                side: 6,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };
    let clone = arc.clone();
    assert_eq!(clone.area(), 36);
    drop(arc);
    drop(clone);

    // SAFETY: the same matching layout, metadata, and complete initializer
    // satisfy `alloc_dst_rc`'s contract.
    let rc: Rc<dyn Shape> = unsafe {
        arena.alloc_dst_rc(layout, metadata, |ptr: *mut dyn Shape| {
            ptr.cast::<Square>().write(Square {
                side: 7,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };
    assert_eq!(rc.area(), 49);
    drop(rc);

    // SAFETY: the same matching layout, metadata, and complete initializer
    // satisfy `alloc_dst_box`'s contract.
    let boxed: Box<dyn Shape> = unsafe {
        arena.alloc_dst_box(layout, metadata, |ptr: *mut dyn Shape| {
            ptr.cast::<Square>().write(Square {
                side: 8,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };
    assert_eq!(boxed.area(), 64);
    drop(boxed);

    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
#[expect(clippy::cast_ptr_alignment, reason = "Layout::extend guarantees both field alignments")]
fn trailing_trait_object_dst_uses_handle_metadata() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let metadata = ptr_meta::metadata(&Square { side: 0, drops: None } as &dyn Shape);
    let (layout, shape_offset) = Layout::new::<u32>().extend(Layout::new::<Square>()).unwrap();
    let layout = layout.pad_to_align();
    let arena = Arena::new();

    // SAFETY: `layout`, `shape_offset`, and `metadata` describe
    // `HeaderAndShape`, and the initializer writes both fields.
    let arc: Arc<HeaderAndShape> = unsafe {
        arena.alloc_dst_arc(layout, metadata, |ptr: *mut HeaderAndShape| {
            let base = ptr.cast::<u8>();
            base.cast::<u32>().write(15);
            base.add(shape_offset).cast::<Square>().write(Square {
                side: 7,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };
    let arc_clone = arc.clone();
    assert_eq!(arc.header, 15);
    assert_eq!(arc_clone.shape.area(), 49);
    drop(arc);
    drop(arc_clone);

    // SAFETY: the same matching layout, metadata, and complete field
    // initialization satisfy `alloc_dst_rc`'s contract.
    let rc: Rc<HeaderAndShape> = unsafe {
        arena.alloc_dst_rc(layout, metadata, |ptr: *mut HeaderAndShape| {
            let base = ptr.cast::<u8>();
            base.cast::<u32>().write(16);
            base.add(shape_offset).cast::<Square>().write(Square {
                side: 8,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };
    let rc_clone = rc.clone();
    assert_eq!(rc.header, 16);
    assert_eq!(rc_clone.shape.area(), 64);
    drop(rc);
    drop(rc_clone);

    // SAFETY: the same matching layout, metadata, and complete field
    // initialization satisfy `alloc_dst_box`'s contract.
    let boxed: Box<HeaderAndShape> = unsafe {
        arena.alloc_dst_box(layout, metadata, |ptr: *mut HeaderAndShape| {
            let base = ptr.cast::<u8>();
            base.cast::<u32>().write(17);
            base.add(shape_offset).cast::<Square>().write(Square {
                side: 9,
                drops: Some(StdArc::clone(&drops)),
            });
        })
    };

    assert_eq!(boxed.header, 17);
    assert_eq!(boxed.shape.area(), 81);
    drop(boxed);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
fn zst_and_overaligned_trait_objects_recover_their_prefixes() {
    let arena = Arena::new();

    let unit: Arc<dyn Shape> = Arc::unsize(arena.alloc_arc(UnitShape), coerce!(dyn Shape));
    let unit_clone = unit.clone();
    assert_eq!(unit_clone.area(), 1);
    drop(unit);
    drop(unit_clone);

    let aligned: Rc<dyn Shape> = Rc::unsize(arena.alloc_rc(AlignedShape(23)), coerce!(dyn Shape));
    let aligned_clone = aligned.clone();
    assert_eq!(aligned_clone.area(), 23);
    drop(aligned);
    drop(aligned_clone);

    let boxed: Box<dyn Shape> = Box::unsize(arena.alloc_box(AlignedShape(29)), coerce!(dyn Shape));
    assert_eq!(boxed.area(), 29);
}

#[test]
fn coercion_panic_retains_original_ownership() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();
    let value = arena.alloc_box(Square {
        side: 2,
        drops: Some(StdArc::clone(&drops)),
    });
    // SAFETY: the closure cannot return an invalid pointer because it always
    // panics; this test exercises ownership cleanup along that unwind path.
    let coercion: Coercion<Square, dyn Shape, _> = unsafe {
        Coercion::new(|_| -> *const dyn Shape {
            panic!("coercion panic");
        })
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        let _: Box<dyn Shape> = Box::unsize(value, coercion);
    }));
    let _ = result.unwrap_err();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn coercion_that_changes_address_panics_and_retains_original_ownership() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();
    let value = arena.alloc_box(Square {
        side: 2,
        drops: Some(StdArc::clone(&drops)),
    });
    let other = AlignedShape(17);
    let other_ptr = core::ptr::from_ref::<dyn Shape>(&other);
    // SAFETY: this deliberately violates the address-preservation contract to
    // exercise the defensive check before the mismatched metadata can be used.
    let coercion: Coercion<Square, dyn Shape, _> = unsafe { Coercion::new(move |_| other_ptr) };

    let result = catch_unwind(AssertUnwindSafe(|| {
        let _: Box<dyn Shape> = Box::unsize(value, coercion);
    }));
    let payload = result.unwrap_err();
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(message.contains("coercion function changed the pointer address"));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn only_vtable_metadata_handles_pay_for_metadata() {
    let word = size_of::<usize>();

    assert_eq!(size_of::<Box<u32>>(), word);
    assert_eq!(size_of::<Box<str>>(), word);
    assert_eq!(size_of::<Box<[u8]>>(), word);
    assert_eq!(size_of::<Arc<u32>>(), word);
    assert_eq!(size_of::<Arc<str>>(), word);
    assert_eq!(size_of::<Arc<[u8]>>(), word);
    assert_eq!(size_of::<Rc<u32>>(), word);
    assert_eq!(size_of::<Rc<str>>(), word);
    assert_eq!(size_of::<Rc<[u8]>>(), word);

    assert_eq!(size_of::<Box<dyn Shape>>(), word * 2);
    assert_eq!(size_of::<Arc<dyn Shape>>(), word * 2);
    assert_eq!(size_of::<Rc<dyn Shape>>(), word * 2);
    assert_eq!(size_of::<Box<HeaderAndShape>>(), word * 2);
    assert_eq!(size_of::<Arc<HeaderAndShape>>(), word * 2);
    assert_eq!(size_of::<Rc<HeaderAndShape>>(), word * 2);
}
