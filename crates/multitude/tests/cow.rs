// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API tests for arena-aware copy-on-write values.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use common::FailingAllocator;
use multitude::{Arena, Cow};

#[test]
fn sized_cow_converts_mutates_and_clones_in() {
    let arena = Arena::new();
    let mut value: Cow<'_, u64> = Cow::Borrowed(&41);

    *value.to_mut(&arena) += 1;
    assert!(value.is_owned());
    assert_eq!(*value, 42);

    let clone = value.try_clone_in(&arena).unwrap();
    assert!(clone.is_owned());
    assert_eq!(*clone, 42);
}

#[test]
fn string_cow_preserves_owned_storage_and_copies_borrowed_storage() {
    let arena = Arena::new();
    let borrowed: Cow<'_, str> = "borrowed".into();
    let owned = borrowed.try_into_owned(&arena).unwrap();
    assert_eq!(&*owned, "borrowed");

    let pointer = owned.as_ptr();
    let owned: Cow<'_, str> = owned.into();
    let same = owned.into_owned(&arena);
    assert_eq!(same.as_ptr(), pointer);
}

#[test]
fn slice_cow_supports_copy_on_write() {
    let arena = Arena::new();
    let mut values: Cow<'_, [u64]> = (&[1, 2, 3][..]).into();

    values.try_to_mut(&arena).unwrap()[1] = 4;
    assert_eq!(&*values, &[1, 4, 3]);
}

#[test]
fn borrowed_clone_remains_borrowed() {
    let arena = Arena::new();
    let value: Cow<'_, str> = "borrowed".into();

    let clone = value.clone_in(&arena);
    assert!(clone.is_borrowed());
    assert!(!clone.is_owned());
    assert_eq!(&*clone, "borrowed");
}

#[test]
fn sized_cow_owned_paths_preserve_or_copy_values() {
    let arena = Arena::new();
    assert_eq!(*Cow::Borrowed(&6_u64).into_owned(&arena), 6);

    let owned = arena.alloc_box(7_u64);
    let pointer = core::ptr::from_ref(&*owned);
    let value: Cow<'_, u64> = owned.into();
    assert!(value.is_owned());
    assert_eq!(value.as_ref(), &7);

    let clone = value.clone_in(&arena);
    assert_eq!(*clone, 7);
    assert_ne!(core::ptr::from_ref(&*clone), pointer);
    let same = value.into_owned(&arena);
    assert_eq!(core::ptr::from_ref(&*same), pointer);

    let owned: Cow<'_, u64> = arena.alloc_box(8).into();
    assert_eq!(*owned.try_into_owned(&arena).unwrap(), 8);

    let borrowed: Cow<'_, u64> = Cow::Borrowed(&9);
    assert!(borrowed.clone_in(&arena).is_borrowed());
    assert!(borrowed.try_clone_in(&arena).unwrap().is_borrowed());
}

#[test]
fn string_and_slice_owned_clones_use_the_target_arena() {
    let source = Arena::new();
    let target = Arena::new();

    let string: Cow<'_, str> = source.alloc_str_box("text").into();
    assert_eq!(&*Cow::Borrowed("borrowed").into_owned(&target), "borrowed");
    assert!(Cow::Borrowed("borrowed").try_clone_in(&target).unwrap().is_borrowed());
    assert_eq!(&*string.clone_in(&target), "text");
    let string_clone = string.try_clone_in(&target).unwrap();
    assert_eq!(&*string_clone, "text");

    let slice: Cow<'_, [u64]> = source.alloc_slice_copy_box([1, 2]).into();
    let slice_clone = slice.clone_in(&target);
    assert_eq!(&*slice_clone, &[1, 2]);

    let borrowed: Cow<'_, [u64]> = Cow::Borrowed(&[3, 4]);
    assert!(borrowed.clone_in(&target).is_borrowed());
    assert!(borrowed.try_clone_in(&target).unwrap().is_borrowed());
    assert_eq!(&*borrowed.into_owned(&target), &[3, 4]);

    let owned = source.alloc_slice_copy_box([5, 6]);
    let pointer = owned.as_ptr();
    let owned: Cow<'_, [u64]> = owned.into();
    assert_eq!(owned.into_owned(&target).as_ptr(), pointer);
}

#[test]
fn borrowed_and_owned_mutation_paths_are_equivalent() {
    let arena = Arena::new();

    let mut sized: Cow<'_, u64> = arena.alloc_box(1).into();
    *sized.try_to_mut(&arena).unwrap() = 2;
    assert_eq!(*sized, 2);
    let mut borrowed_sized: Cow<'_, u64> = Cow::Borrowed(&3);
    *borrowed_sized.try_to_mut(&arena).unwrap() = 4;
    assert_eq!(*borrowed_sized, 4);

    let mut text: Cow<'_, str> = "abc".into();
    text.to_mut(&arena).make_ascii_uppercase();
    assert_eq!(&*text, "ABC");
    text.try_to_mut(&arena).unwrap().make_ascii_lowercase();
    assert_eq!(&*text, "abc");
    let mut fallible_text: Cow<'_, str> = Cow::Borrowed("def");
    fallible_text.try_to_mut(&arena).unwrap().make_ascii_uppercase();
    assert_eq!(&*fallible_text, "DEF");

    let mut slice: Cow<'_, [u64]> = arena.alloc_slice_copy_box([1, 2]).into();
    slice.to_mut(&arena)[0] = 3;
    slice.try_to_mut(&arena).unwrap()[1] = 4;
    assert_eq!(&*slice, &[3, 4]);

    let mut borrowed_slice: Cow<'_, [u64]> = Cow::Borrowed(&[5, 6]);
    borrowed_slice.to_mut(&arena)[0] = 7;
    assert_eq!(&*borrowed_slice, &[7, 6]);

    let owned_slice: Cow<'_, [u64]> = arena.alloc_slice_copy_box([8, 9]).into();
    assert_eq!(&*owned_slice.try_clone_in(&arena).unwrap(), &[8, 9]);
    assert_eq!(&*owned_slice.try_into_owned(&arena).unwrap(), &[8, 9]);
}

#[test]
fn fallible_copy_paths_report_allocation_failure() {
    let arena = Arena::new_in(FailingAllocator::new(0));

    Cow::Borrowed(&1_u64).try_into_owned(&arena).unwrap_err();
    Cow::Borrowed("text").try_into_owned(&arena).unwrap_err();
    Cow::Borrowed(&[1_u64][..]).try_into_owned(&arena).unwrap_err();

    let mut sized = Cow::Borrowed(&1_u64);
    let mut text = Cow::Borrowed("text");
    let mut slice = Cow::Borrowed(&[1_u64][..]);
    sized.try_to_mut(&arena).unwrap_err();
    text.try_to_mut(&arena).unwrap_err();
    slice.try_to_mut(&arena).unwrap_err();
}

#[test]
fn cow_delegates_common_value_traits() {
    let left: Cow<'_, str> = "a".into();
    let right: Cow<'_, str> = "b".into();

    assert!(left < right);
    assert_eq!(Ord::cmp(&left, &right), Ordering::Less);
    let borrowed: &str = std::borrow::Borrow::borrow(&left);
    assert_eq!(borrowed, "a");
    assert_eq!(format!("{left}"), "a");
    assert_eq!(format!("{left:?}"), "\"a\"");

    let mut left_hasher = DefaultHasher::new();
    let mut str_hasher = DefaultHasher::new();
    left.hash(&mut left_hasher);
    "a".hash(&mut str_hasher);
    assert_eq!(left_hasher.finish(), str_hasher.finish());
}
