// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! System-backed storage construction and unwinding regressions.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::SystemSlice;

#[test]
fn gamma_survivor_system_storage_preserves_values_and_initialized_drop_prefix() {
    struct Count<'a>(&'a Cell<usize>);
    impl Drop for Count<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _slice = SystemSlice::from_fn(5, |index| {
            assert!(index != 3, "scripted initialization failure");
            Count(&drops)
        });
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 3);
    let slice = SystemSlice::from_fn(2, |_| Count(&drops));
    drop(slice);
    assert_eq!(drops.get(), 5);
    let calls = Cell::new(0);
    let single = SystemSlice::from_fn(1, |_| {
        calls.set(calls.get() + 1);
        Count(&drops)
    });
    assert_eq!(calls.get(), 1);
    drop(single);
    assert_eq!(drops.get(), 6);
    let mut bytes = SystemSlice::try_zeroed(17).unwrap();
    assert_eq!(&*bytes, &[0; 17]);
    bytes[7] = 23;
    assert_eq!(bytes[7], 23);
    assert!(SystemSlice::try_zeroed(usize::MAX).is_none());
    assert!(SystemSlice::try_zeroed(0).unwrap().is_empty());
    assert!(SystemSlice::<u8>::from_fn(0, |_| panic!("empty initializer")).is_empty());
}

#[test]
fn gamma_survivor_zero_sized_storage_initializes_and_drops_each_element() {
    static DROPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Zero;
    impl Drop for Zero {
        fn drop(&mut self) {
            DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let initialized = Cell::new(0);
    let values = SystemSlice::from_fn(7, |_| {
        initialized.set(initialized.get() + 1);
        Zero
    });
    assert_eq!(initialized.get(), 7);
    assert_eq!(values.len(), 7);
    drop(values);
    assert_eq!(DROPS.load(std::sync::atomic::Ordering::Relaxed), 7);
}
