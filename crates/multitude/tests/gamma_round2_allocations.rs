// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact slice metadata, lifecycle, relocation, and UTF-16 boundary oracles.

use std::cell::Cell;
use std::mem::MaybeUninit;

use multitude::Arena;

mod common;

#[cfg(feature = "stats")]
#[test]
fn gamma_nov4_unallocated_arena_reports_zero_for_every_counter_and_live_gauge() {
    let arena = Arena::new();
    assert_eq!(arena.stats(), multitude::ArenaStats::default());
}

#[cfg(feature = "utf16")]
#[test]
fn gamma_nov4_utf16_debug_propagates_the_output_sinks_formatting_failure() {
    use std::fmt::{self, Write as _};

    struct RejectOutput;
    impl fmt::Write for RejectOutput {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    let arena = Arena::new();
    let value = arena.alloc_utf16_str_box_from_str("a\n\u{10000}");
    let text = &*value;
    assert!(write!(&mut RejectOutput, "{text:?}").is_err());
}

#[test]
fn gamma_nov4_aligned_zst_tag_refill_accepts_the_exact_normal_allocation_limit() {
    #[repr(align(8192))]
    struct WideZst;

    let arena = Arena::builder().max_normal_alloc(8192).build();
    let value = arena
        .try_alloc_box(WideZst)
        .expect("a zero-sized aligned value fits the exact normal refill bound");
    assert_eq!(core::mem::size_of_val(&*value), 0);
    assert_eq!((&raw const *value).addr() % 8192, 0);
}

#[test]
fn gamma_nov4_shrink_distinguishes_the_normal_limit_from_larger_in_place_growth() {
    for capacity in [4096, 4097] {
        let arena = Arena::builder().with_capacity(16 * 1024).max_normal_alloc(4096).build();
        let mut values = arena.alloc_vec_with_capacity::<u8>(16);
        values.extend([11, 23, 37, 41]);
        let pointer = values.as_ptr();
        values.reserve_exact(capacity - values.len());
        assert_eq!(values.as_ptr(), pointer);
        assert_eq!(values.capacity(), capacity);
        values.shrink_to_fit();
        assert_eq!(values.capacity(), if capacity == 4096 { 4 } else { capacity });
        assert_eq!(values.as_slice(), [11, 23, 37, 41]);
        assert_eq!(values.as_ptr(), pointer);
    }
}

#[test]
fn gamma_nov4_nonfreezable_alignment_keeps_exact_reservation_and_relocated_payloads() {
    #[repr(align(16384))]
    struct Wide(u8);

    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let mut values = arena.alloc_vec::<Wide>();
    values.try_reserve_exact(2).unwrap();
    assert_eq!(values.capacity(), 2);
    assert_eq!(values.as_ptr().addr() % 16384, 0);
    values.push(Wide(11));
    values.push(Wide(23));
    let pointer = values.as_ptr();
    let blocker = arena.alloc(0x71_u8);
    values.try_reserve_exact(1).unwrap();
    assert_eq!(values.capacity(), 3);
    assert_ne!(values.as_ptr(), pointer);
    assert_eq!(values.as_ptr().addr() % 16384, 0);
    assert_eq!((values[0].0, values[1].0, *blocker), (11, 23, 0x71));
    values.push(Wide(37));
    assert_eq!(values.iter().map(|value| value.0).collect::<Vec<_>>(), [11, 23, 37]);
}

#[cfg(feature = "stats")]
#[test]
fn gamma_nov4_one_element_buffer_counts_its_oversized_relocation() {
    let arena = Arena::builder().max_normal_alloc(4096).build();
    let mut values = arena.alloc_vec_with_capacity::<u8>(1);
    values.push(71);
    let before = arena.stats().relocations;
    let pointer = values.as_ptr();
    values.reserve_exact(8192);
    assert_eq!(values.capacity(), 8193);
    assert_ne!(values.as_ptr(), pointer);
    assert_eq!(values.as_slice(), [71]);
    assert_eq!(arena.stats().relocations, before + 1);
}

#[test]
fn gamma_nov4_max_length_zst_iterator_reaches_its_first_item_without_length_arithmetic_overflow() {
    struct FirstItemPanics<'a>(&'a Cell<bool>);

    impl Iterator for FirstItemPanics<'_> {
        type Item = ();

        fn next(&mut self) -> Option<Self::Item> {
            self.0.set(true);
            std::panic::panic_any(());
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            (usize::MAX, Some(usize::MAX))
        }
    }

    impl ExactSizeIterator for FirstItemPanics<'_> {}

    let arena = Arena::new();
    let reached = Cell::new(false);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = arena.try_alloc_slice_fill_iter(FirstItemPanics(&reached));
    }));
    assert!(result.is_err());
    assert!(
        reached.get(),
        "the zero-sized allocation must reach the iterator before its planned panic"
    );
    assert_eq!(*arena.alloc(73_u8), 73);
}

#[cfg(feature = "utf16")]
#[test]
fn gamma_nov4_prefixed_utf16_reserves_every_unit_and_aligns_after_an_odd_cursor() {
    for text in ["a", "ab", "€", "\u{10000}", "a€\u{10000}z"] {
        let arena = Arena::builder().with_capacity(4096).build();
        let leading = arena.alloc(0x71_u8);
        let value = arena.alloc_utf16_str_box_from_str(text);
        let pointer = value.as_slice().as_ptr().addr();
        let units: Vec<_> = text.encode_utf16().collect();
        assert_eq!(pointer % core::mem::align_of::<u16>(), 0);
        assert_eq!(value.as_slice(), units);
        let trailing = arena.alloc(0x93_u8);
        assert_eq!((&raw const *trailing).addr(), pointer + units.len() * core::mem::size_of::<u16>());
        assert_eq!((*leading, *trailing), (0x71, 0x93));
        assert_eq!(value.as_slice(), units);
    }
}

#[cfg(all(feature = "utf16", feature = "stats"))]
#[test]
fn gamma_nov4_prefixed_utf16_oversized_cutover_uses_prefix_and_every_unit() {
    let tracker = common::TrackingAllocator::new();
    {
        let arena = Arena::builder_in(tracker.clone()).max_normal_alloc(4097).build();
        let text = "a".repeat(2045);
        let value = arena.alloc_utf16_str_box_from_str(&text);
        assert_eq!(value.as_slice(), text.encode_utf16().collect::<Vec<_>>());
        assert_eq!(arena.stats().oversized_chunks_allocated, 1);
        let payload = core::mem::size_of::<usize>() + 2 * text.len();
        let word = core::mem::align_of::<usize>();
        let rounded_payload = (payload + word - 1).next_multiple_of(word);
        assert_eq!(arena.stats().wasted_tail_bytes, u64::try_from(rounded_payload - payload).unwrap());
        assert_eq!(tracker.live_chunks(), 1);
        drop(value);
        assert_eq!(tracker.live_chunks(), 0);
    }
    assert_eq!((tracker.live_chunks(), tracker.live_bytes()), (0, 0));
}

#[test]
fn gamma_nov4_empty_split_tail_retains_the_original_heads_spare_capacity() {
    let arena = Arena::builder().with_capacity(4096).build();
    let mut values = arena.alloc_vec_with_capacity::<u32>(8);
    values.extend([11, 23, 37]);
    let original = values.as_ptr();
    let tail = values.try_split_off(3).unwrap();
    assert_eq!(values.as_slice(), [11, 23, 37]);
    assert_eq!(values.as_ptr(), original);
    assert_eq!(values.capacity(), 8);
    assert!(tail.is_empty());
    assert_eq!(tail.capacity(), 0);
    values.extend([41, 53]);
    assert_eq!(values.as_ptr(), original);
    assert_eq!(values.as_slice(), [11, 23, 37, 41, 53]);
}

#[test]
fn gamma_round2_exact_vector_growth_retains_capacity_payload_and_later_allocations() {
    let arena = Arena::builder().with_capacity(4096).build();
    for capacity in [1, 7, 17] {
        let mut values = arena.alloc_vec::<u32>();
        values.try_reserve_exact(capacity).unwrap();
        assert_eq!(values.capacity(), capacity);
        values.extend((0..capacity).map(|index| u32::try_from(index).unwrap() + 71));
        let before = values.as_ptr();
        let blocker = arena.alloc(0x93_u8);
        values.try_reserve_exact(3).unwrap();
        assert_eq!(values.capacity(), capacity + 3);
        assert_ne!(values.as_ptr(), before);
        assert_eq!(
            &*values,
            &(0..capacity).map(|index| u32::try_from(index).unwrap() + 71).collect::<Vec<_>>()
        );
        assert_eq!(*blocker, 0x93);
    }
}

#[test]
fn gamma_round2_allocator_alignment_changes_relocate_and_zero_shrink_releases_ownership() {
    use std::alloc::Layout;

    use allocator_api2::alloc::Allocator;

    let tracker = common::TrackingAllocator::new();
    {
        let arena = Arena::new_in(tracker.clone());
        let allocator = &arena;
        let old_layout = Layout::from_size_align(8, 1).unwrap();
        let original = allocator.allocate(old_layout).unwrap().cast::<u8>();
        // SAFETY: The allocation exclusively owns eight writable bytes.
        unsafe { original.as_ptr().write_bytes(0x71, 8) };
        let grown_layout = Layout::from_size_align(16, 64).unwrap();
        // SAFETY: The initialized allocation has old_layout; the valid new layout grows its size.
        let grown = unsafe { allocator.grow(original, old_layout, grown_layout) }.unwrap();
        assert_ne!(grown.cast::<u8>(), original);
        assert_eq!(grown.cast::<u8>().as_ptr().addr() % 64, 0);
        // SAFETY: Successful grow preserves the eight initialized original bytes.
        assert_eq!(unsafe { std::slice::from_raw_parts(grown.cast::<u8>().as_ptr(), 8) }, [0x71; 8]);
        let one = Layout::from_size_align(1, 64).unwrap();
        // SAFETY: The live grown allocation supports shrinking to one byte with unchanged alignment.
        let shrunk = unsafe { allocator.shrink(grown.cast(), grown_layout, one) }.unwrap();
        assert_eq!(shrunk.cast::<u8>(), grown.cast::<u8>());
        let looser = Layout::from_size_align(1, 1).unwrap();
        // SAFETY: The one-byte initialized allocation remains live and the new layout fits it.
        let relocated = unsafe { allocator.shrink(shrunk.cast(), one, looser) }.unwrap();
        assert_ne!(relocated.cast::<u8>(), shrunk.cast::<u8>());
        // SAFETY: The successful shrink preserves its one initialized byte.
        assert_eq!(unsafe { *relocated.cast::<u8>().as_ptr() }, 0x71);
        let zero = Layout::from_size_align(0, 1).unwrap();
        // SAFETY: The live one-byte allocation is surrendered exactly once by shrinking to zero.
        let empty = unsafe { allocator.shrink(relocated.cast(), looser, zero) }.unwrap();
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.cast::<u8>().as_ptr().addr(), 1);
        // SAFETY: Zero-sized deallocation is a no-op for the exact layout returned above.
        unsafe { allocator.deallocate(empty.cast(), zero) };
    }
    assert_eq!((tracker.live_chunks(), tracker.live_bytes()), (0, 0));
}

#[test]
fn gamma_round2_pinned_rc_slice_calls_initializer_once_per_requested_element() {
    let arena = Arena::new();
    for len in [0, 1, 3, 17] {
        let mut calls = Vec::new();
        let values = arena.alloc_slice_fill_with_rc_pin(len, |index| {
            calls.push(index);
            index * 7 + 3
        });
        assert_eq!(calls, (0..len).collect::<Vec<_>>());
        assert_eq!(&*values, &(0..len).map(|index| index * 7 + 3).collect::<Vec<_>>());
        let mut calls = Vec::new();
        let values = arena
            .try_alloc_slice_fill_with_rc_pin(len, |index| {
                calls.push(index);
                index + 29
            })
            .unwrap();
        assert_eq!(calls, (0..len).collect::<Vec<_>>());
        assert_eq!(&*values, &(0..len).map(|index| index + 29).collect::<Vec<_>>());
    }
}

fn initialized(values: &[MaybeUninit<u32>]) -> Vec<u32> {
    values
        .iter()
        .map(|value| {
            // SAFETY: Callers pass only zeroed slots or slots explicitly written before this call.
            unsafe { value.assume_init() }
        })
        .collect()
}

#[test]
fn gamma_round2_uninitialized_slice_wrappers_preserve_exact_lengths() {
    let arena = Arena::new();
    for len in [0, 1, 3, 17] {
        let values = arena.try_alloc_zeroed_slice::<u32>(len).unwrap();
        assert_eq!(values.len(), len);
        assert_eq!(initialized(&values), vec![0; len]);
        let mut values = arena.try_alloc_uninit_slice_rc::<u32>(len).unwrap();
        assert_eq!(values.len(), len);
        let writable = multitude::Rc::get_mut(&mut values).unwrap();
        for (index, slot) in writable.iter_mut().enumerate() {
            slot.write(u32::try_from(index).unwrap() + 71);
        }
        assert_eq!(
            initialized(&values),
            (0..len).map(|index| u32::try_from(index).unwrap() + 71).collect::<Vec<_>>()
        );
        let values = arena.try_alloc_zeroed_slice_rc::<u32>(len).unwrap();
        assert_eq!(values.len(), len);
        assert_eq!(initialized(&values), vec![0; len]);
        let mut values = arena.try_alloc_uninit_slice_box::<u32>(len).unwrap();
        assert_eq!(values.len(), len);
        for slot in values.iter_mut() {
            slot.write(93);
        }
        assert_eq!(initialized(&values), vec![93; len]);
        let values = arena.try_alloc_zeroed_slice_box::<u32>(len).unwrap();
        assert_eq!(values.len(), len);
        assert_eq!(initialized(&values), vec![0; len]);
    }
}

#[test]
fn gamma_round2_slice_iter_fill_preserves_indices_and_payload() {
    let arena = Arena::new();
    for len in 0..=17 {
        let source: Vec<u16> = (0..len).map(|index| u16::try_from(index * 19 + 1).unwrap()).collect();
        let filled = arena.alloc_slice_fill_iter(source.iter().copied());
        assert_eq!(filled.len(), len);
        assert_eq!(&*filled, source.as_slice());
    }
}

#[test]
fn gamma_round2_byte_layout_keeps_sentinel_allocation_after_copied_payload() {
    let arena = Arena::builder().with_capacity(4096).build();
    for len in [1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 31] {
        let source: Vec<u8> = (0..len).map(|index| u8::try_from(index * 7 + 1).unwrap()).collect();
        let copied = arena.alloc_slice_copy(&source);
        let sentinel = arena.alloc(0xb7_u8);
        assert_eq!(&*copied, source.as_slice());
        assert_eq!(*sentinel, 0xb7);
        assert_eq!((&raw const *sentinel).addr(), copied.as_ptr().addr() + len);
    }
}

#[test]
fn gamma_round2_vector_splice_drop_and_tail_compaction_keep_identity_order() {
    struct Value<'a>(usize, &'a Cell<[u8; 4]>);
    impl Drop for Value<'_> {
        fn drop(&mut self) {
            let mut counts = self.1.get();
            counts[self.0] += 1;
            self.1.set(counts);
        }
    }
    let arena = Arena::new();
    for (start, end) in [(0, 0), (1, 1), (1, 4), (4, 4), (0, 4)] {
        let dropped = Cell::new([0; 4]);
        let mut values = arena.alloc_vec_with_capacity(8);
        values.extend((0..4).map(|index| Value(index, &dropped)));
        {
            let mut drain = values.drain(start..end);
            if start != end {
                assert_eq!(drain.next().unwrap().0, start);
            }
        }
        assert_eq!(
            values.iter().map(|value| value.0).collect::<Vec<_>>(),
            (0..start).chain(end..4).collect::<Vec<_>>()
        );
        assert_eq!(dropped.get(), std::array::from_fn(|index| u8::from((start..end).contains(&index))));
        drop(values);
        assert_eq!(dropped.get(), [1; 4]);
    }
}

#[cfg(feature = "stats")]
#[test]
fn gamma_round2_oversized_vector_relocation_counts_only_existing_buffers() {
    let arena = Arena::builder().max_normal_alloc(4096).build();
    let mut values = arena.alloc_vec::<u32>();
    let before = arena.stats().relocations;
    values.reserve_exact(2048);
    assert_eq!(values.capacity(), 2048);
    assert_eq!(arena.stats().relocations, before);
    values.extend([11, 22, 33]);
    let old = values.as_ptr();
    let blocker = arena.alloc(99_u8);
    values.reserve_exact(4096);
    assert_ne!(values.as_ptr(), old);
    assert_eq!(values.capacity(), 4099);
    assert_eq!(&*values, &[11, 22, 33]);
    assert_eq!(*blocker, 99);
    assert_eq!(arena.stats().relocations, before + 1);
}

#[cfg(feature = "utf16")]
#[test]
fn gamma_round2_utf16_drain_includes_surrogate_range_endpoints() {
    let arena = Arena::new();
    let text = "\u{10000}\u{103ff}\u{10fc00}\u{10ffff}";
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut value = arena.alloc_utf16_string();
    value.push_from_str(text);
    assert_eq!(value.as_slice(), units.as_slice());
    assert_eq!(value.drain(..).collect::<String>(), text);
    assert!(value.is_empty());
    value.push_from_str(text);
    assert_eq!(value.drain(..).rev().collect::<String>(), text.chars().rev().collect::<String>());
    assert!(value.is_empty());
    value.push_from_str(text);
    let mut drain = value.drain(..);
    assert_eq!(drain.next(), Some('\u{10000}'));
    assert_eq!(drain.next_back(), Some('\u{10ffff}'));
    assert_eq!(drain.next(), Some('\u{103ff}'));
    assert_eq!(drain.next_back(), Some('\u{10fc00}'));
    assert_eq!(drain.next(), None);
    assert_eq!(drain.next_back(), None);
}

#[test]
fn gamma_round2_fallible_collection_operations_propagate_exhaustion() {
    let arena = Arena::new_in(common::FailingAllocator::new(0));
    let other_arena = Arena::new_in(common::FailingAllocator::new(1));
    arena.try_alloc_string_from_utf8_lossy([0xff, b'a']).unwrap_err();
    arena.try_alloc_box(()).unwrap_err();
    let oversized = "x".repeat(100_000);
    arena.try_alloc_str(oversized).unwrap_err();
    let mut values = arena.alloc_vec::<u32>();
    let mut other = other_arena.alloc_vec_with_capacity(3);
    other.extend([11, 22, 33]);
    assert!(values.try_append(&mut other).is_err());
    assert!(values.is_empty());
    assert_eq!(&*other, &[11, 22, 33]);
}

#[cfg(feature = "utf16")]
#[test]
fn gamma_round2_utf16_debug_includes_content_and_quotes() {
    let arena = Arena::new();
    let mut value = arena.alloc_utf16_string();
    value.push_from_str("A😀\"Z");
    let expected = format!("{:?}", "A😀\"Z");
    assert_eq!(format!("{value:?}"), expected);
    assert_eq!(format!("{:?}", value.as_utf16_str()), expected);
    assert!(!expected.is_empty());
}

#[cfg(feature = "serde_json")]
#[test]
fn gamma_round2_json_errors_retain_successful_callback_prefix_and_reject_trailing_input() {
    use multitude::de::{DeserializationLimits, JsonEachError};

    let arena = Arena::new();
    let mut seen = Vec::new();
    let error = arena
        .try_deserialize_json_each::<u32, _, _, ()>("[7,11,broken]", |value| {
            seen.push(value);
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(error, JsonEachError::Json(_)));
    assert_eq!(seen, [7, 11]);
    let error = arena
        .deserialize_json_alloc_with_limits::<u32, _>("17 false", DeserializationLimits::unlimited())
        .unwrap_err();
    assert!(error.as_json_error().to_string().contains("trailing characters"));
    let error = arena.deserialize_json::<multitude::Rc<[u32]>, _>(r#"[7,"wrong"]"#).unwrap_err();
    assert!(error.to_string().contains("invalid type"));
    let failing = Arena::new_in(common::FailingAllocator::new(0));
    let mut values = failing.alloc_vec::<u32>();
    let mut deserializer = serde_json::Deserializer::from_str("[7]");
    values.deserialize_reusing(&mut deserializer).unwrap_err();
    assert!(values.is_empty());
}

#[test]
fn gamma_round2_exhausted_allocator_still_allows_zero_copy_split() {
    let allocator = common::FailingAllocator::new(1);
    let arena = Arena::new_in(allocator);
    let mut values = arena.alloc_vec_with_capacity(3);
    values.extend([7_u32, 11, 19]);
    let other = arena.alloc(23_u8);
    let tail = values.try_split_off(1).unwrap();
    assert_eq!(&*values, &[7]);
    assert_eq!(&*tail, &[11, 19]);
    assert_eq!(*other, 23);
    arena.try_alloc_box([13_u8; 8192]).unwrap_err();
    assert_eq!(&*values, &[7]);
    assert_eq!(&*tail, &[11, 19]);
}

#[test]
fn gamma_round2_lossy_string_suffix_growth_failure_is_returned() {
    let mut input = vec![0xff; 1500];
    input.extend(std::iter::repeat_n(b'a', 3500));
    let arena = Arena::builder_in(common::FailingAllocator::new(1)).max_normal_alloc(4096).build();
    arena.try_alloc_string_from_utf8_lossy(&input).unwrap_err();
    #[cfg(feature = "stats")]
    assert_eq!(arena.stats().oversized_chunks_allocated, 1);
    let arena = Arena::builder().max_normal_alloc(4096).build();
    let value = arena.try_alloc_string_from_utf8_lossy(&input).unwrap();
    assert_eq!(&*value, "\u{fffd}".repeat(1500) + &"a".repeat(3500));
}
