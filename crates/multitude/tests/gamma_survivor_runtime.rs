// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public arena allocation and collection contracts.

use core::alloc::Layout;
use core::cmp::Ordering;
use std::sync::Arc as Shared;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use allocator_api2::alloc::Allocator;
use multitude::{Arena, Cow};

mod common;

#[derive(Clone)]
struct DropValue(Shared<AtomicUsize>, usize);

struct BorrowedDropCounter<'a>(&'a AtomicUsize);

#[derive(Clone, Copy)]
#[repr(align(65536))]
struct ChunkAlignedNonzero {
    _storage: [u8; 65536],
}

#[test]
fn gamma_survivor_native_alignment_rejects_empty_nonzero_element_slice() {
    let arena = Arena::new();
    let empty: &[ChunkAlignedNonzero] = &[];
    gamma_survivor_expect_error(arena.try_alloc_slice_copy(empty));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone(empty));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_box(empty));
}

#[test]
fn gamma_survivor_unit_slice_metadata_supports_usize_max_without_iteration() {
    let arena = Arena::new();
    // SAFETY: () has alignment one and occupies zero bytes for any length;
    // dangling is non-null and aligned, and no backing memory is accessed.
    let source = unsafe { core::slice::from_raw_parts(core::ptr::NonNull::<()>::dangling().as_ptr(), usize::MAX) };
    let copied = arena.try_alloc_slice_copy(source).unwrap();
    assert_eq!(copied.len(), usize::MAX);
    let boxed = arena.try_alloc_slice_copy_box(source).unwrap();
    assert_eq!(boxed.len(), usize::MAX);
}

impl Drop for BorrowedDropCounter<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, AtomicOrdering::Relaxed);
    }
}

#[test]
fn gamma_survivor_box_assume_init_transfers_chunk_ownership() {
    let drops = AtomicUsize::new(0);
    let tracker = common::TrackingAllocator::new();
    let initialized = {
        let arena = Arena::new_in(tracker.clone());
        let mut values = arena.alloc_uninit_slice_box::<BorrowedDropCounter<'_>>(4);
        for slot in values.iter_mut() {
            slot.write(BorrowedDropCounter(&drops));
        }
        // SAFETY: every MaybeUninit slot was initialized above.
        std::mem::ManuallyDrop::new(unsafe { values.assume_init() })
    };
    assert_eq!(tracker.live_chunks(), 1);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 0);
    drop(std::mem::ManuallyDrop::into_inner(initialized));
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 4);
    assert_eq!(tracker.live_chunks(), 0);
}

#[test]
fn gamma_survivor_empty_prefixed_strings_reserve_exact_interior_byte() {
    for family in 0..3 {
        let arena = Arena::builder().with_capacity(512).build();
        let payload = match family {
            0 => arena.alloc_str_box("").as_ptr(),
            1 => arena.alloc_str_arc("").as_ptr(),
            _ => arena.alloc_str_rc("").as_ptr(),
        };
        let next = arena.alloc(91_u8);
        assert_eq!((&raw const *next).addr(), payload.addr() + 1);
        assert_eq!(*next, 91);
    }
}

#[cfg(all(feature = "stats", feature = "utf16"))]
#[test]
fn gamma_survivor_cold_small_families_use_normal_not_oversized_chunks() {
    for family in 0..11 {
        let arena = Arena::new();
        match family {
            0 => drop(arena.alloc_str_box("abc")),
            1 => drop(arena.alloc_str_arc("abc")),
            2 => drop(arena.alloc_str_rc("abc")),
            3 => drop(arena.alloc_slice_copy_box([1_u16, 2, 3])),
            4 => drop(arena.alloc_slice_fill_with_box(3, |index| index)),
            5 => drop(arena.alloc_slice_fill_with_arc(3, |index| index)),
            6 => drop(arena.alloc_slice_fill_with_rc(3, |index| index)),
            7 => drop(arena.alloc_utf16_str_box(widestring::utf16str!("abc"))),
            8 => drop(arena.alloc_utf16_str_box_from_str("abc")),
            9 => drop(arena.alloc_utf16_str_arc_from_str("abc")),
            _ => drop(arena.alloc_utf16_str_rc_from_str("abc")),
        }
        assert_eq!(arena.stats().normal_chunks_allocated, 1, "family {family}");
        assert_eq!(arena.stats().oversized_chunks_allocated, 0, "family {family}");
    }
}

#[test]
fn gamma_survivor_in_place_rc_freeze_preserves_vector_address() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity::<u32>(5);
    values.extend_from_slice([11, 22, 33]);
    let pointer = values.as_ptr();
    let shared = values.try_into_rc_slice().unwrap();
    assert_eq!(shared.as_ptr().cast::<u32>(), pointer);
    assert_eq!(&*shared, &[11, 22, 33]);
}

#[test]
fn gamma_survivor_empty_string_rc_freeze_reports_fallback_allocation_errors() {
    let arena = Arena::new_in(common::FailingAllocator::new(0));
    gamma_survivor_expect_error(arena.alloc_string().try_into_rc_str());
    #[cfg(feature = "utf16")]
    gamma_survivor_expect_error(arena.alloc_utf16_string().try_into_rc_utf16_str());
}

#[test]
fn gamma_survivor_vector_integer_overflows_without_allocating_or_iterating() {
    let arena = Arena::new_in(common::FailingAllocator::new(0));
    let mut units = arena.alloc_vec::<()>();
    // SAFETY: () has no storage or invalid bit patterns; all usize::MAX ZST slots are initialized.
    unsafe { units.set_len(usize::MAX) };
    gamma_survivor_expect_error(units.try_push(()));
    gamma_survivor_expect_error(units.try_extend_from_slice([()]));
    assert_eq!(units.len(), usize::MAX);
    let mut aligned = arena.alloc_vec::<u128>();
    gamma_survivor_expect_error(aligned.try_reserve(usize::MAX / core::mem::size_of::<u128>()));
    assert_eq!(aligned.len(), 0);
    assert_eq!(aligned.capacity(), 0);
}

#[test]
fn gamma_survivor_splice_relocation_drops_empty_drain_without_old_pointer_arithmetic() {
    let arena = Arena::new();
    let drops = AtomicUsize::new(0);
    let mut values = arena.alloc_vec_with_capacity(3);
    values.extend([1_u8, 2, 3].map(|value| (value, BorrowedDropCounter(&drops))));
    let before = values.as_ptr().addr();
    let blocker = arena.alloc(197_u8);
    drop(values.splice(1..2, [9_u8, 10, 11, 12].map(|value| (value, BorrowedDropCounter(&drops)))));
    assert!(values.as_ptr().addr() > before);
    assert_eq!(values.iter().map(|(value, _)| *value).collect::<Vec<_>>(), [1, 9, 10, 11, 12, 3]);
    assert_eq!(*blocker, 197);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 1);
    drop(values);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 7);
}

#[cfg(all(feature = "stats", feature = "utf16"))]
fn gamma_survivor_check_allocation_hint_boundary<const N: usize>() {
    use std::boxed::Box as HostBox;

    for family in (0..if cfg!(feature = "dst") { 31 } else { 25 }).chain([31]) {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let word = core::mem::size_of::<usize>();
        let hint = match family {
            0..=3 | 6..=9 | 28 => N + 1,
            4 | 29 => N + 5,
            5 | 30 => N + 8,
            16 => N + word,
            10..=11 | 25 => N + word + 1,
            12 | 14 | 17 | 26 => N + word + 5,
            13 | 15 | 18 | 27 => N + word + 8,
            19 | 22 => 2 * N + word,
            20 | 23 => 2 * N + word + 6,
            31 => 16 * N + 32,
            _ => 2 * N + word + 8,
        };
        let source = [7_u8; N];
        let utf8 = std::str::from_utf8(&source).expect("the fixture contains only ASCII byte 7");
        let units = [7_u16; N];
        let wide = widestring::Utf16Str::from_slice(&units).expect("unit 7 is a valid non-surrogate scalar");
        #[cfg(feature = "dst")]
        let slice_layout = Layout::array::<u8>(N).expect("every fixture length is at most 4109 bytes");
        #[cfg(feature = "dst")]
        let initialize_slice = |pointer: *mut [u8]| {
            // SAFETY: each caller supplies a fresh N-byte reservation disjoint from source.
            unsafe { pointer.cast::<u8>().copy_from_nonoverlapping(source.as_ptr(), N) };
        };
        #[cfg(feature = "dst")]
        let initialize_sized = |pointer: *mut [u8; N]| {
            // SAFETY: each caller supplies fresh exclusive storage of the matching array layout.
            unsafe { pointer.write(source) };
        };
        let allocation: HostBox<dyn core::fmt::Debug + '_> = match family {
            0 => HostBox::new(arena.alloc(source)),
            1 => HostBox::new(arena.alloc_with(|| source)),
            2 => HostBox::new(arena.alloc_box(source)),
            3 => HostBox::new(arena.alloc_box_with(|| source)),
            4 => HostBox::new(arena.alloc_rc(source)),
            5 => HostBox::new(arena.alloc_arc(source)),
            6 => HostBox::new(arena.alloc_slice_copy(source)),
            7 => HostBox::new(arena.alloc_slice_clone(source)),
            8 => HostBox::new(arena.alloc_slice_fill_with(N, |_| 7_u8)),
            9 => HostBox::new(arena.alloc_slice_fill_iter((0..N).map(|_| 7_u8))),
            10 => HostBox::new(arena.alloc_slice_copy_box(source)),
            11 => HostBox::new(arena.alloc_slice_fill_with_box(N, |_| 7_u8)),
            12 => HostBox::new(arena.alloc_slice_copy_rc(source)),
            13 => HostBox::new(arena.alloc_slice_copy_arc(source)),
            14 => HostBox::new(arena.alloc_slice_fill_with_rc(N, |_| 7_u8)),
            15 => HostBox::new(arena.alloc_slice_fill_with_arc(N, |_| 7_u8)),
            16 => HostBox::new(arena.alloc_str_box(utf8)),
            17 => HostBox::new(arena.alloc_str_rc(utf8)),
            18 => HostBox::new(arena.alloc_str_arc(utf8)),
            19 => HostBox::new(arena.alloc_utf16_str_box_from_str(utf8)),
            20 => HostBox::new(arena.alloc_utf16_str_rc_from_str(utf8)),
            21 => HostBox::new(arena.alloc_utf16_str_arc_from_str(utf8)),
            22 => HostBox::new(arena.alloc_utf16_str_box(wide)),
            23 => HostBox::new(arena.alloc_utf16_str_rc(wide)),
            24 => HostBox::new(arena.alloc_utf16_str_arc(wide)),
            #[cfg(feature = "dst")]
            25 => {
                // SAFETY: layout and metadata describe N bytes; the initializer writes all of them.
                HostBox::new(unsafe { arena.alloc_dst_box::<[u8]>(slice_layout, N, initialize_slice) })
            }
            #[cfg(feature = "dst")]
            26 => {
                // SAFETY: layout and metadata describe N bytes; the initializer writes all of them.
                HostBox::new(unsafe { arena.alloc_dst_rc::<[u8]>(slice_layout, N, initialize_slice) })
            }
            #[cfg(feature = "dst")]
            27 => {
                // SAFETY: layout and metadata describe N bytes; the initializer writes all of them.
                HostBox::new(unsafe { arena.alloc_dst_arc::<[u8]>(slice_layout, N, initialize_slice) })
            }
            #[cfg(feature = "dst")]
            28 => {
                // SAFETY: sized metadata is (); writing source initializes the complete matching layout.
                HostBox::new(unsafe { arena.alloc_dst_box::<[u8; N]>(Layout::new::<[u8; N]>(), (), initialize_sized) })
            }
            #[cfg(feature = "dst")]
            29 => {
                // SAFETY: sized metadata is (); writing source initializes the complete matching layout.
                HostBox::new(unsafe { arena.alloc_dst_rc::<[u8; N]>(Layout::new::<[u8; N]>(), (), initialize_sized) })
            }
            #[cfg(feature = "dst")]
            30 => {
                // SAFETY: sized metadata is (); writing source initializes the complete matching layout.
                HostBox::new(unsafe { arena.alloc_dst_arc::<[u8; N]>(Layout::new::<[u8; N]>(), (), initialize_sized) })
            }
            31 => HostBox::new(arena.alloc_vec_with_capacity::<u128>(N)),
            _ => unreachable!("the family range includes only the allocation cases above"),
        };
        let stats = arena.stats();
        let oversized = hint > 4096;
        assert_eq!(stats.normal_chunks_allocated, u64::from(!oversized), "family {family}, N {N}");
        assert_eq!(stats.oversized_chunks_allocated, u64::from(oversized), "family {family}, N {N}");
        let header = 5 * word + core::mem::size_of::<u32>();
        let expected = if oversized {
            (header + (hint + word - 1).div_ceil(word) * word).div_ceil(word) * word
        } else {
            (header + hint).next_power_of_two().max(512)
        };
        let expected_bytes = u64::try_from(expected).expect("all fixture backing allocations fit in u64");
        assert_eq!(stats.total_bytes_allocated, expected_bytes, "family {family}, N {N}");
        drop(allocation);
    }
}

#[cfg(all(feature = "stats", feature = "utf16"))]
#[test]
fn gamma_survivor_allocation_hint_routing_and_word_rounding_boundaries() {
    gamma_survivor_check_allocation_hint_boundary::<28>();
    gamma_survivor_check_allocation_hint_boundary::<29>();
    gamma_survivor_check_allocation_hint_boundary::<226>();
    gamma_survivor_check_allocation_hint_boundary::<227>();
    gamma_survivor_check_allocation_hint_boundary::<230>();
    gamma_survivor_check_allocation_hint_boundary::<254>();
    gamma_survivor_check_allocation_hint_boundary::<255>();
    gamma_survivor_check_allocation_hint_boundary::<256>();
    gamma_survivor_check_allocation_hint_boundary::<452>();
    gamma_survivor_check_allocation_hint_boundary::<453>();
    gamma_survivor_check_allocation_hint_boundary::<455>();
    gamma_survivor_check_allocation_hint_boundary::<456>();
    gamma_survivor_check_allocation_hint_boundary::<459>();
    gamma_survivor_check_allocation_hint_boundary::<460>();
    gamma_survivor_check_allocation_hint_boundary::<461>();
    gamma_survivor_check_allocation_hint_boundary::<463>();
    gamma_survivor_check_allocation_hint_boundary::<464>();
    gamma_survivor_check_allocation_hint_boundary::<467>();
    gamma_survivor_check_allocation_hint_boundary::<468>();
    gamma_survivor_check_allocation_hint_boundary::<2040>();
    gamma_survivor_check_allocation_hint_boundary::<2041>();
    gamma_survivor_check_allocation_hint_boundary::<2042>();
    gamma_survivor_check_allocation_hint_boundary::<2044>();
    gamma_survivor_check_allocation_hint_boundary::<2045>();
    gamma_survivor_check_allocation_hint_boundary::<4080>();
    gamma_survivor_check_allocation_hint_boundary::<4081>();
    gamma_survivor_check_allocation_hint_boundary::<4083>();
    gamma_survivor_check_allocation_hint_boundary::<4084>();
    gamma_survivor_check_allocation_hint_boundary::<4087>();
    gamma_survivor_check_allocation_hint_boundary::<4088>();
    gamma_survivor_check_allocation_hint_boundary::<4089>();
    gamma_survivor_check_allocation_hint_boundary::<4091>();
    gamma_survivor_check_allocation_hint_boundary::<4092>();
    gamma_survivor_check_allocation_hint_boundary::<4095>();
    gamma_survivor_check_allocation_hint_boundary::<4096>();
    gamma_survivor_check_allocation_hint_boundary::<4104>();
    gamma_survivor_check_allocation_hint_boundary::<4105>();
    gamma_survivor_check_allocation_hint_boundary::<4106>();
    gamma_survivor_check_allocation_hint_boundary::<4108>();
    gamma_survivor_check_allocation_hint_boundary::<4109>();
}

#[test]
fn gamma_survivor_oversized_failures_return_errors_without_initialization() {
    let arena = Arena::builder_in(common::SendFailingAllocator::new(0))
        .max_normal_alloc(4096)
        .build();
    let source = [7_u8; 8192];
    gamma_survivor_expect_error(arena.try_alloc(source));
    gamma_survivor_expect_error(arena.try_alloc_with(|| -> [u8; 8192] { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_box(source));
    gamma_survivor_expect_error(arena.try_alloc_box_with(|| -> [u8; 8192] { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_rc(source));
    gamma_survivor_expect_error(arena.try_alloc_arc(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with(8192, |_| -> u8 { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_iter((0..8192).map(|_| -> u8 { panic!("iterator consumed") })));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_box(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_rc(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_arc(source));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with_box(8192, |_| -> u8 { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with_rc(8192, |_| -> u8 { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with_arc(8192, |_| -> u8 { panic!("initializer ran") }));
    gamma_survivor_expect_error(arena.try_alloc_string_from_utf8_lossy(b"x"));
    #[cfg(feature = "utf16")]
    gamma_survivor_expect_error(multitude::strings::Utf16String::try_from_utf16_str_in(
        widestring::utf16str!("x"),
        &arena,
    ));
}

#[test]
fn gamma_survivor_utf16_lossy_growth_errors_include_odd_trailing_unit() {
    let invalid = [0xd800_u16; 400];
    for family in 0..5 {
        let arena = Arena::builder().byte_budget(512).with_capacity(512).build();
        let result = match family {
            0 => arena.try_alloc_string_from_utf16_lossy(invalid),
            1 => arena.try_alloc_string_from_utf16le_lossy(invalid.into_iter().flat_map(u16::to_le_bytes).collect::<Vec<_>>()),
            2 => arena.try_alloc_string_from_utf16be_lossy(invalid.into_iter().flat_map(u16::to_be_bytes).collect::<Vec<_>>()),
            3 => arena.try_alloc_string_from_utf16le_lossy(
                [0x0041_u16; 400]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .chain([0xff])
                    .collect::<Vec<_>>(),
            ),
            _ => arena.try_alloc_string_from_utf16be_lossy(
                [0x0041_u16; 400]
                    .into_iter()
                    .flat_map(u16::to_be_bytes)
                    .chain([0xff])
                    .collect::<Vec<_>>(),
            ),
        };
        gamma_survivor_expect_error(result);
    }
}

impl Drop for DropValue {
    fn drop(&mut self) {
        self.0.fetch_add(self.1, AtomicOrdering::Relaxed);
    }
}

#[test]
fn gamma_survivor_allocator_resize_zero_alignment_and_lifetimes() {
    let tracker = common::TrackingAllocator::new();
    {
        let arena = Arena::new_in(tracker.clone());
        let handle = &arena;
        let empty = Layout::from_size_align(0, 64).unwrap();
        let empty_ptr = handle.allocate(empty).unwrap();
        assert_eq!(empty_ptr.len(), 0);
        assert_eq!(empty_ptr.cast::<u8>().as_ptr().addr() % 64, 0);
        assert_eq!(tracker.live_chunks(), 0);
        // SAFETY: this is the zero-sized allocation returned by this handle.
        unsafe { handle.deallocate(empty_ptr.cast(), empty) };

        for relocate in [false, true] {
            let old = Layout::from_size_align(3, 1).unwrap();
            let new = Layout::from_size_align(17, if relocate { 64 } else { 1 }).unwrap();
            let ptr = handle.allocate(old).unwrap().cast::<u8>();
            // SAFETY: the allocation contains three writable bytes.
            unsafe { ptr.as_ptr().copy_from_nonoverlapping([11, 22, 33].as_ptr(), 3) };
            // SAFETY: ptr owns old; new is larger and has a valid alignment.
            let grown = unsafe { handle.grow_zeroed(ptr, old, new) }.unwrap();
            assert_eq!(grown.len(), 17);
            assert_eq!(grown.cast::<u8>().as_ptr().addr() % new.align(), 0);
            // SAFETY: grow_zeroed preserves the initialized prefix and zeros its tail.
            let bytes = unsafe { grown.as_ref() };
            assert_eq!(&bytes[..3], &[11, 22, 33]);
            assert_eq!(&bytes[3..], &[0; 14]);
            if !relocate {
                assert_eq!(grown.cast::<u8>(), ptr);
            }
            let shrunk_layout = Layout::from_size_align(2, 1).unwrap();
            // SAFETY: grown owns new and the requested size is smaller.
            let shrunk = unsafe { handle.shrink(grown.cast(), new, shrunk_layout) }.unwrap();
            // SAFETY: the first two bytes are preserved by shrink.
            assert_eq!(unsafe { shrunk.as_ref() }, &[11, 22]);
            // SAFETY: shrunk owns shrunk_layout; shrinking to zero is valid.
            let zero = unsafe { handle.shrink(shrunk.cast(), shrunk_layout, empty) }.unwrap();
            assert_eq!(zero.len(), 0);
            // SAFETY: zero is returned by this allocator for empty.
            unsafe { handle.deallocate(zero.cast(), empty) };
        }
        let old = Layout::from_size_align(1, 1).unwrap();
        let new = Layout::from_size_align(4096, 8).unwrap();
        let ptr = handle.allocate(old).unwrap().cast::<u8>();
        // SAFETY: ptr has one writable byte.
        unsafe { ptr.as_ptr().write(197) };
        let blocker = arena.alloc(55_u64);
        // SAFETY: ptr owns old; new is larger, so allocation and copying are valid.
        let grown = unsafe { handle.grow(ptr, old, new) }.unwrap();
        // SAFETY: grow preserves the initialized first byte.
        assert_eq!(unsafe { grown.cast::<u8>().as_ptr().read() }, 197);
        assert_eq!(*blocker, 55);
        // SAFETY: grown is returned for new by this handle.
        unsafe { handle.deallocate(grown.cast(), new) };
        let legacy = allocator_api2_02::alloc::Allocator::allocate(&handle, old).unwrap();
        // SAFETY: the legacy handle returned this pointer for old.
        unsafe { allocator_api2_02::alloc::Allocator::deallocate(&handle, legacy.cast(), old) };
    }
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
}

#[test]
fn gamma_survivor_allocator_failures_do_not_panic_or_run_initializers() {
    let arena = Arena::new_in(common::FailingAllocator::new(0));
    gamma_survivor_expect_error(<&Arena<_> as Allocator>::allocate(&&arena, Layout::new::<u64>()));
    gamma_survivor_expect_error(arena.try_alloc_with(|| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_box_with(|| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_rc_with(|| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with(3, |_| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with_box(3, |_| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_with_rc(3, |_| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone([String::from("one")]));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_box([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone_box([String::from("one")]));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy_rc([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone_rc([String::from("one")]));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_iter([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_iter_box([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_slice_fill_iter_rc([1_u32, 2]));
    gamma_survivor_expect_error(arena.try_alloc_str_box("nonempty"));
    gamma_survivor_expect_error(arena.try_alloc_str_rc("nonempty"));
    gamma_survivor_expect_error(arena.try_alloc_uninit::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_zeroed::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_uninit_box::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_zeroed_box::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_uninit_rc::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_zeroed_rc::<u64>());
    gamma_survivor_expect_error(arena.try_alloc_uninit_slice::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc_zeroed_slice::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc_uninit_slice_box::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc_zeroed_slice_box::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc_uninit_slice_rc::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc_zeroed_slice_rc::<u64>(3));
    let arc_arena = Arena::new_in(common::SendFailingAllocator::new(0));
    gamma_survivor_expect_error(arc_arena.try_alloc_arc_with(|| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arc_arena.try_alloc_slice_fill_with_arc(3, |_| -> u64 { panic!("initializer ran before reservation") }));
    gamma_survivor_expect_error(arc_arena.try_alloc_slice_copy_arc([1_u32, 2]));
    gamma_survivor_expect_error(arc_arena.try_alloc_slice_clone_arc([String::from("one")]));
    gamma_survivor_expect_error(arc_arena.try_alloc_slice_fill_iter_arc([1_u32, 2]));
    gamma_survivor_expect_error(arc_arena.try_alloc_str_arc("nonempty"));
    gamma_survivor_expect_error(arc_arena.try_alloc_uninit_arc::<u64>());
    gamma_survivor_expect_error(arc_arena.try_alloc_zeroed_arc::<u64>());
    gamma_survivor_expect_error(arc_arena.try_alloc_uninit_slice_arc::<u64>(3));
    gamma_survivor_expect_error(arc_arena.try_alloc_zeroed_slice_arc::<u64>(3));
    gamma_survivor_expect_error(arena.try_alloc([7_u8; 9000]));
    gamma_survivor_expect_error(arena.try_alloc_box([7_u8; 9000]));
    gamma_survivor_expect_error(arena.try_alloc_rc([7_u8; 9000]));
    gamma_survivor_expect_error(arc_arena.try_alloc_arc([7_u8; 9000]));
    gamma_survivor_expect_error(arena.try_alloc_slice_copy([7_u8; 9000]));
    gamma_survivor_expect_error(arena.try_alloc_slice_clone([7_u8; 9000]));
    gamma_survivor_expect_error(arena.try_alloc_str_box("a".repeat(9000)));
    let expansion_arena = Arena::new_in(common::FailingAllocator::new(1));
    gamma_survivor_expect_error(expansion_arena.try_alloc_string_from_utf8_lossy([0xff; 5000]));
}

#[test]
fn gamma_survivor_all_slice_paths_keep_values_lengths_and_ownership() {
    let tracker = common::SendTrackingAllocator::new();
    for len in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 257, 20_000] {
        let arena = Arena::new_in(tracker.clone());
        let expected: Vec<u32> = (0..len).map(|index| u32::try_from(index * 7 + 3).unwrap()).collect();
        assert_eq!(&*arena.alloc_slice_copy(&expected), &expected);
        assert_eq!(&*arena.alloc_slice_clone(&expected), &expected);
        assert_eq!(&*arena.alloc_slice_fill_with(len, |index| expected[index]), &expected);
        assert_eq!(&*arena.alloc_slice_fill_iter(expected.iter().copied()), &expected);
        let boxed = arena.alloc_slice_copy_box(&expected);
        let arc = arena.alloc_slice_copy_arc(&expected);
        let rc = arena.alloc_slice_copy_rc(&expected);
        let fill_box = arena.alloc_slice_fill_with_box(len, |index| expected[index]);
        let fill_arc = arena.alloc_slice_fill_with_arc(len, |index| expected[index]);
        let filled_local = arena.alloc_slice_fill_with_rc(len, |index| expected[index]);
        let iter_box = arena.alloc_slice_fill_iter_box(expected.iter().copied());
        let iter_arc = arena.alloc_slice_fill_iter_arc(expected.iter().copied());
        let iter_rc = arena.alloc_slice_fill_iter_rc(expected.iter().copied());
        drop(arena);
        for actual in [
            &*boxed,
            &*arc,
            &*rc,
            &*fill_box,
            &*fill_arc,
            &*filled_local,
            &*iter_box,
            &*iter_arc,
            &*iter_rc,
        ] {
            assert_eq!(actual.len(), len);
            assert_eq!(actual, expected);
        }
    }
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
    let arena = Arena::new();
    assert_eq!(arena.alloc_slice_copy([(); 7]).len(), 7);
    assert_eq!(arena.alloc_slice_copy_box([(); 7]).len(), 7);
    assert_eq!(arena.alloc_slice_copy_rc([(); 7]).len(), 7);
    assert_eq!(arena.alloc_slice_copy_arc([(); 7]).len(), 7);
    let unsupported_alignment = Layout::from_size_align(32768, 32768).unwrap();
    gamma_survivor_expect_error(<&Arena<_> as Allocator>::allocate(&&arena, unsupported_alignment));
}

#[test]
fn gamma_survivor_partial_slice_initialization_drops_only_completed_values() {
    let arena = Arena::new();
    for owner in 0..4 {
        let drops = Shared::new(AtomicUsize::new(0));
        let initializer = |index| {
            assert_ne!(index, 3, "planned initializer failure");
            DropValue(Shared::clone(&drops), 1)
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match owner {
            0 => drop(arena.alloc_slice_fill_with(5, initializer)),
            1 => drop(arena.alloc_slice_fill_with_box(5, initializer)),
            2 => drop(arena.alloc_slice_fill_with_rc(5, initializer)),
            _ => drop(arena.alloc_slice_fill_with_arc(5, initializer)),
        }));
        gamma_survivor_expect_error(result);
        assert_eq!(drops.load(AtomicOrdering::Relaxed), 3);
    }
    let drops = Shared::new(AtomicUsize::new(0));
    let boxed = arena.alloc_box(DropValue(Shared::clone(&drops), 1));
    let rc = arena.alloc_rc(DropValue(Shared::clone(&drops), 2));
    let arc = arena.alloc_arc(DropValue(Shared::clone(&drops), 4));
    let rc_clone = multitude::Rc::clone(&rc);
    let arc_clone = multitude::Arc::clone(&arc);
    drop(boxed);
    drop(rc);
    drop(arc);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 1);
    drop(rc_clone);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 3);
    drop(arc_clone);
    assert_eq!(drops.load(AtomicOrdering::Relaxed), 7);
}

#[test]
fn gamma_survivor_zeroed_and_uninitialized_storage() {
    let arena = Arena::new();
    let mut scalar = arena.alloc_uninit::<u64>();
    assert_eq!(*scalar.write(0x1234_5678_9abc_def0), 0x1234_5678_9abc_def0);
    for len in [0, 1, 7, 9000] {
        let refs = arena.alloc_zeroed_slice::<u64>(len);
        let boxed = arena.alloc_zeroed_slice_box::<u64>(len);
        let arc = arena.alloc_zeroed_slice_arc::<u64>(len);
        let rc = arena.alloc_zeroed_slice_rc::<u64>(len);
        let uninit = arena.alloc_uninit_slice::<u64>(len);
        let uninit_box = arena.alloc_uninit_slice_box::<u64>(len);
        let uninit_arc = arena.alloc_uninit_slice_arc::<u64>(len);
        let local_uninitialized = arena.alloc_uninit_slice_rc::<u64>(len);
        for slots in [&*uninit, &*uninit_box, &*uninit_arc, &*local_uninitialized] {
            assert_eq!(slots.len(), len);
        }
        for slots in [&*refs, &*boxed, &*arc, &*rc] {
            assert_eq!(slots.len(), len);
            for slot in slots {
                // SAFETY: all slots were zero-filled; zero is a valid u64.
                assert_eq!(unsafe { slot.assume_init() }, 0);
            }
        }
    }
}

#[test]
fn gamma_survivor_comparisons_and_box_iterator_forwarding() {
    let arena = Arena::new();
    let first = arena.alloc(1_u8);
    let same = arena.alloc(1_u8);
    let last = arena.alloc(2_u8);
    assert_eq!(first.cmp(&same), Ordering::Equal);
    assert_eq!(last.cmp(&first), Ordering::Greater);
    let first: Cow<'_, str> = Cow::Borrowed("a");
    let same: Cow<'_, str> = Cow::Borrowed("a");
    let last: Cow<'_, str> = Cow::Borrowed("z");
    assert_eq!(first.cmp(&same), Ordering::Equal);
    assert_eq!(last.cmp(&first), Ordering::Greater);
    let mut iterator = arena.alloc_box(0..10);
    assert_eq!(iterator.nth_back(2), Some(7));
    assert_eq!(iterator.next(), Some(0));
    assert_eq!(iterator.nth_back(0), Some(6));
    assert_eq!(iterator.len(), 5);
}

#[test]
fn gamma_survivor_vector_mutations_drains_splices_and_zst() {
    let arena = Arena::new();
    let mut actual = arena.alloc_vec();
    let mut expected = Vec::new();
    for value in 0..40_u32 {
        actual.push(value);
        expected.push(value);
    }
    assert_eq!(AsRef::<[u32]>::as_ref(&actual), expected);
    actual.insert(0, 99);
    expected.insert(0, 99);
    actual.insert(17, 88);
    expected.insert(17, 88);
    assert_eq!(actual.remove(3), expected.remove(3));
    assert_eq!(actual.swap_remove(7), expected.swap_remove(7));
    actual.retain_mut(|value| {
        *value += 3;
        *value % 2 == 0
    });
    expected.retain_mut(|value| {
        *value += 3;
        *value % 2 == 0
    });
    assert_eq!(actual.as_slice(), expected);
    actual.extend_from_within(2..5);
    expected.extend_from_within(2..5);
    actual.resize_with(actual.len(), || panic!("same-length resize called initializer"));
    actual.resize(actual.len() + 3, 77);
    expected.resize(expected.len() + 3, 77);
    actual.dedup();
    expected.dedup();
    assert_eq!(actual.as_slice(), expected);
    for (start, end) in [(0, 0), (2, 5), (1, 1)] {
        let mut left = actual.drain(start..end);
        let mut right = expected.drain(start..end);
        assert_eq!(left.next(), right.next());
        assert_eq!(left.next_back(), right.next_back());
        drop(left);
        drop(right);
        assert_eq!(actual.as_slice(), expected);
    }
    let mut splice = actual.splice(1..3, [90, 91, 92, 93]);
    assert_eq!(format!("{splice:?}"), "Splice { remaining: 2 }");
    assert_eq!(splice.next(), expected.get(1).copied());
    drop(splice);
    expected.splice(1..3, [90, 91, 92, 93]);
    assert_eq!(actual.as_slice(), expected);
    let tail = actual.split_off(3);
    let expected_tail = expected.split_off(3);
    assert_eq!(tail.as_slice(), expected_tail);
    assert_eq!(actual.as_slice(), expected);
    let boxed = tail.into_boxed_slice();
    assert_eq!(&*boxed, expected_tail);
    let mut zst = arena.alloc_vec::<()>();
    zst.extend([(); 5]);
    assert_eq!(zst.split_off(2).len(), 3);
    assert_eq!(zst.len(), 2);
    zst.resize_with(2, || panic!("equal-length resize called initializer"));
    zst.shrink_to_fit();
    assert_eq!(zst.len(), 2);
    let mut a = arena.alloc_vec();
    a.extend([1_u8, 2]);
    let mut b = arena.alloc_vec();
    b.extend([1_u8, 3]);
    assert_eq!(a.cmp(&a), Ordering::Equal);
    assert_eq!(b.cmp(&a), Ordering::Greater);
    let mut hint_text = arena.alloc_string();
    hint_text.push_str("a😀");
    let mut hint_drain = hint_text.drain(..);
    assert_eq!(hint_drain.size_hint(), (2, Some(5)));
    assert_eq!(hint_drain.next(), Some('a'));
    assert_eq!(hint_drain.size_hint(), (1, Some(4)));
    assert_eq!(hint_drain.next_back(), Some('😀'));
    assert_eq!(hint_drain.size_hint(), (0, Some(0)));
}

#[test]
fn gamma_survivor_string_unicode_editing_and_drain() {
    let arena = Arena::new();
    let mut actual = arena.alloc_string();
    let mut expected = String::new();
    actual.extend(['a', 'é', '😀', 'z']);
    expected.extend(['a', 'é', '😀', 'z']);
    actual.truncate(actual.len());
    assert_eq!(actual.as_str(), expected);
    actual.insert(1, '中');
    expected.insert(1, '中');
    actual.insert_str(0, "");
    actual.insert_str(0, "ß");
    expected.insert(0, 'ß');
    actual.retain(|ch| ch != 'é');
    expected.retain(|ch| ch != 'é');
    assert_eq!(actual.as_str(), expected);
    actual.replace_range(0..2, "Q");
    expected.replace_range(0..2, "Q");
    let mut left = actual.drain(1..5);
    let mut right = expected.drain(1..5);
    assert_eq!(format!("{left:?}"), "Drain { .. }");
    assert_eq!(left.next(), right.next());
    assert_eq!(left.next_back(), right.next_back());
    assert_eq!(left.next(), right.next());
    assert_eq!(left.next_back(), right.next_back());
    drop(left);
    drop(right);
    assert_eq!(actual.as_str(), expected);
    actual.push('\0');
    expected.push('\0');
    assert_eq!(actual.pop(), expected.pop());
    let tail = actual.split_off(1);
    let expected_tail = expected.split_off(1);
    assert_eq!(actual.as_str(), expected);
    assert_eq!(&*tail.into_rc_str(), expected_tail);
    let a = arena.alloc_string_from_utf8_lossy(b"same");
    let b = arena.alloc_string_from_utf8_lossy(b"z");
    assert_eq!(a.cmp(&a), Ordering::Equal);
    assert_eq!(b.cmp(&a), Ordering::Greater);
}

#[cfg(feature = "utf16")]
#[test]
fn gamma_survivor_utf16_surrogate_boundaries_and_editing() {
    let arena = Arena::new();
    let units = [0xd7ff, 0xd800, 0xdc00, 0xdbff, 0xdfff, 0xe000];
    let source = widestring::Utf16Str::from_slice(&units).unwrap();
    let mut value = arena.alloc_utf16_string();
    value.push_str(source);
    assert_eq!(value.as_slice(), units);
    value.truncate(value.len());
    let mut drain = value.drain(..);
    assert_eq!(format!("{drain:?}"), "Utf16Drain { .. }");
    assert_eq!(drain.next(), Some('\u{d7ff}'));
    assert_eq!(drain.next(), Some('\u{10000}'));
    assert_eq!(drain.next_back(), Some('\u{e000}'));
    assert_eq!(drain.next_back(), Some('\u{10ffff}'));
    assert_eq!(drain.next(), None);
    assert_eq!(drain.next_back(), None);
    drop(drain);
    assert!(value.is_empty());
    value.extend(['a', '\u{10000}', '\u{10ffff}', 'z']);
    value.retain(|ch| ch != 'a');
    value.insert(0, '中');
    value.insert_utf16_str(0, widestring::utf16str!(""));
    value.push_from_str("é");
    assert_eq!(value.to_string(), "中\u{10000}\u{10ffff}zé");
    let tail = value.split_off(1);
    assert_eq!(value.to_string(), "中");
    assert_eq!(tail.to_string(), "\u{10000}\u{10ffff}zé");
    assert_eq!(
        format!("{:?}", tail.as_utf16_str()),
        format!("{:?}", widestring::utf16str!("\u{10000}\u{10ffff}zé"))
    );
    assert_eq!(tail.into_rc_utf16_str().to_string(), "\u{10000}\u{10ffff}zé");
    assert_eq!(
        arena.alloc_string_from_utf16le([0x00, 0xd8]).unwrap_err().to_string(),
        "invalid UTF-16: odd byte length or unpaired surrogate"
    );
}

#[test]
fn gamma_survivor_builder_budgets_debug_and_resize_failures() {
    let debug = format!("{:?}", Arena::builder().max_normal_alloc(4096).byte_budget(123).with_capacity(512));
    assert_eq!(
        debug,
        "ArenaBuilder { max_normal_alloc: 4096, byte_budget: Some(123), capacity: 512 }"
    );
    let arena = Arena::builder_in(common::FailingAllocator::new(0))
        .with_capacity(0)
        .try_build()
        .unwrap();
    assert_eq!(format!("{arena:?}"), "Arena { .. }");
    gamma_survivor_expect_error(Arena::builder_in(common::FailingAllocator::new(0)).with_capacity(512).try_build());
    gamma_survivor_expect_error(Arena::builder().byte_budget(511).with_capacity(512).try_build());
    gamma_survivor_expect_error(arena.try_alloc(17_u64));
    let arena = Arena::builder().byte_budget(0).build();
    gamma_survivor_expect_error(arena.try_alloc_slice_copy([3_u64; 100]));
    let mut vec = arena.alloc_vec::<u64>();
    gamma_survivor_expect_error(vec.try_push(1));
    gamma_survivor_expect_error(vec.try_insert(0, 1));
    gamma_survivor_expect_error(vec.try_resize(3, 1));
    gamma_survivor_expect_error(vec.try_resize_with(3, || 1));
    gamma_survivor_expect_error(vec.try_extend_from_slice([1, 2]));
    assert_eq!(vec.len(), 0);
    let mut text = arena.alloc_string();
    gamma_survivor_expect_error(text.try_push('é'));
    gamma_survivor_expect_error(text.try_push_str("owned"));
    gamma_survivor_expect_error(text.try_insert(0, 'é'));
    gamma_survivor_expect_error(text.try_insert_str(0, "owned"));
    gamma_survivor_expect_error(text.try_replace_range(.., "owned"));
    assert!(text.is_empty());
    for threshold in [0, 4095, usize::MAX] {
        let result = std::panic::catch_unwind(|| drop(Arena::builder().max_normal_alloc(threshold).build()));
        gamma_survivor_expect_error(result);
    }
    for capacity in [1, 511] {
        let result = std::panic::catch_unwind(|| drop(Arena::builder().with_capacity(capacity).build()));
        gamma_survivor_expect_error(result);
    }
    let arena = Arena::new();
    let mut values = arena.alloc_vec::<u64>();
    values.push(1);
    gamma_survivor_expect_error(values.try_reserve(usize::MAX));
    gamma_survivor_expect_error(values.try_reserve_exact(usize::MAX));
    assert_eq!(values.as_slice(), &[1]);
    gamma_survivor_expect_error(arena.try_alloc_vec_with_capacity::<u64>(usize::MAX));
    gamma_survivor_expect_error(arena.try_alloc_uninit_slice::<u64>(usize::MAX));
    gamma_survivor_expect_error(arena.try_alloc_uninit_slice_box::<u64>(usize::MAX));
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_cache_reset_relocation_and_oversized_accounting() {
    let tracker = common::TrackingAllocator::new();
    {
        let mut arena = Arena::builder_in(tracker.clone()).max_normal_alloc(4096).with_capacity(512).build();
        assert_eq!(arena.stats().normal_chunks_allocated, 1);
        let mut values = arena.alloc_vec_with_capacity(3);
        values.extend([1_u64, 2, 3]);
        let blocker = arena.alloc(0x7654_u64);
        values.reserve(100);
        assert_eq!(values.as_slice(), &[1, 2, 3]);
        assert_eq!(*blocker, 0x7654);
        assert_eq!(arena.stats().relocations, 1);
        drop(values);
        drop(blocker);
        let oversized = arena.alloc_slice_copy([77_u8; 9000]);
        assert_eq!(oversized[8999], 77);
        assert_eq!(arena.stats().oversized_chunks_allocated, 1);
        drop(oversized);
        arena.reset();
        let stats = arena.stats();
        assert_eq!(stats.resets, 1);
        assert_eq!(stats.relocations_since_reset, 0);
        assert_eq!(stats.normal_chunks_allocated_since_reset, 0);
        assert_eq!(stats.oversized_chunks_allocated_since_reset, 0);
        let value = arena.alloc(15_u64);
        assert_eq!(*value, 15);
        assert!(arena.stats().normal_chunks_reused > 0);
    }
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
}

#[cfg(feature = "dst")]
#[test]
fn gamma_survivor_dst_metadata_layouts_and_lifetimes() {
    let tracker = common::SendTrackingAllocator::new();
    for len in [0, 1, 3, 8192] {
        let arena = Arena::new_in(tracker.clone());
        let source: Vec<u64> = (0..len).map(|index| u64::try_from(index + 17).unwrap()).collect();
        let layout = Layout::array::<u64>(len).unwrap();
        let initialize = |ptr: *mut [u64]| {
            // SAFETY: each caller below supplies the exact slice layout and metadata.
            unsafe { ptr.cast::<u64>().copy_from_nonoverlapping(source.as_ptr(), len) };
        };
        // SAFETY: layout, metadata, and initializer describe exactly source.
        let boxed = unsafe { arena.alloc_dst_box::<[u64]>(layout, len, initialize) };
        // SAFETY: layout, metadata, and initializer describe exactly source.
        let rc = unsafe { arena.alloc_dst_rc::<[u64]>(layout, len, initialize) };
        // SAFETY: layout, metadata, and initializer describe exactly source.
        let arc = unsafe { arena.alloc_dst_arc::<[u64]>(layout, len, initialize) };
        let initialize_scalar = |ptr: *mut u64| {
            // SAFETY: the caller supplies a fresh exclusive u64 reservation.
            unsafe { ptr.write(913) };
        };
        // SAFETY: u64 uses unit metadata; the initializer writes one valid u64.
        let scalar = unsafe { arena.alloc_dst_box::<u64>(Layout::new::<u64>(), (), initialize_scalar) };
        drop(arena);
        assert_eq!(*scalar, 913);
        assert_eq!(&*boxed, source);
        assert_eq!(&*rc, source);
        assert_eq!(&*arc, source);
    }
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
    let failing = Arena::new_in(common::SendFailingAllocator::new(0));
    let layout = Layout::array::<u64>(3).unwrap();
    // SAFETY: all requests describe a [u64] of length three; no reservation succeeds.
    let boxed = unsafe { failing.try_alloc_dst_box::<[u64]>(layout, 3, |_| panic!("initializer ran on failure")) };
    // SAFETY: all requests describe a [u64] of length three; no reservation succeeds.
    let rc = unsafe { failing.try_alloc_dst_rc::<[u64]>(layout, 3, |_| panic!("initializer ran on failure")) };
    // SAFETY: all requests describe a [u64] of length three; no reservation succeeds.
    let arc = unsafe { failing.try_alloc_dst_arc::<[u64]>(layout, 3, |_| panic!("initializer ran on failure")) };
    gamma_survivor_expect_error(boxed);
    gamma_survivor_expect_error(rc);
    gamma_survivor_expect_error(arc);
}

#[cfg(feature = "bytesbuf")]
#[test]
fn gamma_survivor_bytesbuf_reservation_and_shared_lifetime() {
    use bytesbuf::mem::Memory;

    let tracker = common::SendTrackingAllocator::new();
    let arena = Arena::new_in(tracker.clone());
    let mut empty = arena.reserve(0);
    assert!(empty.consume_all().is_empty());
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
    let mut buffer = arena.reserve(7);
    buffer.put_slice(*b"payload");
    let value = buffer.consume_all();
    let clone = value.clone();
    drop(value);
    drop(arena);
    assert_eq!(clone, b"payload");
    drop(clone);
    drop(buffer);
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_exact_chunk_tail_boundary() {
    let tracker = common::TrackingAllocator::new();
    let arena = Arena::builder_in(tracker.clone()).byte_budget(512).with_capacity(512).build();
    let first = arena.alloc(19_u8);
    let remaining = usize::try_from(arena.stats().wasted_tail_bytes).unwrap();
    assert!(remaining > 0);
    let tail = arena.try_alloc_uninit_slice::<u8>(remaining).unwrap();
    assert_eq!(tail.len(), remaining);
    assert_eq!(arena.stats().wasted_tail_bytes, 0);
    assert_eq!(*first, 19);
    gamma_survivor_expect_error(arena.try_alloc(20_u8));
    assert_eq!(tracker.live_bytes(), 512);
}

#[test]
fn gamma_survivor_local_references_survive_refill_and_retirement() {
    let tracker = common::TrackingAllocator::new();
    for family in 0..5 {
        let arena = Arena::builder_in(tracker.clone()).max_normal_alloc(4096).build();
        match family {
            0 => {
                let value = arena.alloc([17_u8; 128]);
                check_refill_retention(&arena, &*value);
            }
            1 => {
                let value = arena.alloc_slice_copy([17_u8; 128]);
                check_refill_retention(&arena, &value);
            }
            2 => {
                let value = arena.alloc_slice_fill_with(128, |_| 17_u8);
                check_refill_retention(&arena, &value);
            }
            3 => {
                let value = arena.alloc_slice_copy([17_u8; 9000]);
                check_refill_retention(&arena, &value);
            }
            _ => {
                let value = arena.alloc_str("\u{11}".repeat(128));
                check_refill_retention(&arena, value.as_bytes());
            }
        }
    }
    assert_eq!(tracker.live_chunks(), 0);
    assert_eq!(tracker.live_bytes(), 0);
}

fn check_refill_retention(arena: &Arena<common::TrackingAllocator>, first: &[u8]) {
    for marker in 0..20_u8 {
        let scratch = arena.alloc_slice_copy([marker; 1000]);
        assert_eq!(scratch[999], marker);
        assert!(first.iter().all(|byte| *byte == 17));
    }
}

#[test]
fn gamma_survivor_warm_chunk_budget_reservation_families() {
    for family in 0..17 {
        let arena = Arena::builder().byte_budget(512).with_capacity(512).build();
        match family {
            0 => assert_eq!(*arena.try_alloc(17_u64).unwrap(), 17),
            1 => assert_eq!(*arena.try_alloc_box(17_u64).unwrap(), 17),
            2 => assert_eq!(*arena.try_alloc_arc(17_u64).unwrap(), 17),
            3 => assert_eq!(*arena.try_alloc_rc(17_u64).unwrap(), 17),
            4 => assert_eq!(&*arena.try_alloc_slice_copy([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            5 => assert_eq!(&*arena.try_alloc_slice_clone([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            6 => assert_eq!(&*arena.try_alloc_slice_fill_with(3, |index| index + 17).unwrap(), &[17, 18, 19]),
            7 => assert_eq!(&*arena.try_alloc_slice_fill_iter([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            8 => assert_eq!(&*arena.try_alloc_slice_copy_box([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            9 => assert_eq!(&*arena.try_alloc_slice_copy_arc([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            10 => assert_eq!(&*arena.try_alloc_slice_copy_rc([17_u64, 18, 19]).unwrap(), &[17, 18, 19]),
            11 => assert_eq!(&*arena.try_alloc_str_box("text").unwrap(), "text"),
            12 => assert_eq!(&*arena.try_alloc_str_arc("text").unwrap(), "text"),
            13 => assert_eq!(&*arena.try_alloc_str_rc("text").unwrap(), "text"),
            14 => {
                let mut values = arena.try_alloc_vec_with_capacity::<u64>(3).unwrap();
                values.extend([17, 18, 19]);
                assert_eq!(values.as_slice(), &[17, 18, 19]);
            }

            15 => {
                let mut values = arena.try_alloc_vec_with_capacity::<u128>(3).unwrap();
                values.extend([17, 18, 19]);
                assert_eq!(&*values.try_into_arc_slice().unwrap(), &[17, 18, 19]);
            }
            _ => assert_eq!(&*arena.try_alloc_str("text").unwrap(), "text"),
        }
    }
    #[cfg(feature = "dst")]
    {
        let arena = Arena::builder().byte_budget(512).with_capacity(512).build();
        let initialize_scalar = |ptr: *mut u64| {
            // SAFETY: the caller supplies a fresh exclusive u64 reservation.
            unsafe { ptr.write(17) };
        };
        // SAFETY: layout and unit metadata describe one u64, fully written by init.
        let scalar = unsafe { arena.try_alloc_dst_rc::<u64>(Layout::new::<u64>(), (), initialize_scalar) }.unwrap();
        assert_eq!(*scalar, 17);
        let layout = Layout::array::<u64>(3).unwrap();
        let initialize = |ptr: *mut [u64]| {
            // SAFETY: the caller supplies the exact [u64] layout and length three.
            unsafe { ptr.cast::<u64>().copy_from_nonoverlapping([17, 18, 19].as_ptr(), 3) };
        };
        // SAFETY: layout, metadata and initializer describe exactly three u64s.
        let slice = unsafe { arena.try_alloc_dst_box::<[u64]>(layout, 3, initialize) }.unwrap();
        assert_eq!(&*slice, &[17, 18, 19]);
    }
}

#[test]
fn gamma_survivor_failed_allocator_resize_preserves_old_block() {
    let arena = Arena::builder().byte_budget(512).with_capacity(512).build();
    let handle = &arena;
    let old = Layout::from_size_align(1, 1).unwrap();
    let oversized = Layout::from_size_align(9000, 1).unwrap();
    let ptr = handle.allocate(old).unwrap().cast::<u8>();
    // SAFETY: ptr owns one writable byte.
    unsafe { ptr.as_ptr().write(73) };
    // SAFETY: ptr owns old; the requested layout grows it. Failure retains ptr.
    gamma_survivor_expect_error(unsafe { handle.grow(ptr, old, oversized) });
    // SAFETY: the preceding failed grow retained ptr and its initialized byte.
    assert_eq!(unsafe { ptr.as_ptr().read() }, 73);
    // SAFETY: ptr still owns old; failure again retains that allocation.
    gamma_survivor_expect_error(unsafe { handle.grow_zeroed(ptr, old, oversized) });
    let unsupported_alignment = Layout::from_size_align(1, 32768).unwrap();
    // SAFETY: ptr owns old; the new layout requests the same size with stricter alignment.
    gamma_survivor_expect_error(unsafe { handle.shrink(ptr, old, unsupported_alignment) });
    // SAFETY: both failed resizes retained the original initialized byte.
    assert_eq!(unsafe { ptr.as_ptr().read() }, 73);
    // SAFETY: ptr still owns old because all resize attempts failed.
    unsafe { handle.deallocate(ptr, old) };
}

#[cfg(test)]
fn gamma_survivor_expect_error<T, E>(result: Result<T, E>) {
    let Err(_) = result else {
        panic!("the fixture requires this operation to return its injected error");
    };
    drop(result);
}

#[cfg(feature = "dst")]
fn gamma_survivor_empty_dst_owner(arena: &Arena, family: usize, address: &std::cell::Cell<usize>) -> std::boxed::Box<dyn core::fmt::Debug> {
    let sized = |pointer: *mut ()| address.set(pointer.addr());
    let slice = |pointer: *mut [u8]| address.set(pointer.cast::<u8>().addr());
    match family {
        // SAFETY: () is a zero-sized inhabited type and needs no storage initialization.
        0 => std::boxed::Box::new(unsafe { arena.alloc_dst_box::<()>(Layout::new::<()>(), (), sized) }),
        // SAFETY: () is a zero-sized inhabited type and needs no storage initialization.
        1 => std::boxed::Box::new(unsafe { arena.alloc_dst_rc::<()>(Layout::new::<()>(), (), sized) }),
        // SAFETY: () is a zero-sized inhabited type and needs no storage initialization.
        2 => std::boxed::Box::new(unsafe { arena.alloc_dst_arc::<()>(Layout::new::<()>(), (), sized) }),
        // SAFETY: the zero-length u8 slice has no elements to initialize.
        3 => std::boxed::Box::new(unsafe { arena.alloc_dst_box::<[u8]>(Layout::new::<()>(), 0, slice) }),
        // SAFETY: the zero-length u8 slice has no elements to initialize.
        4 => std::boxed::Box::new(unsafe { arena.alloc_dst_rc::<[u8]>(Layout::new::<()>(), 0, slice) }),
        // SAFETY: the zero-length u8 slice has no elements to initialize.
        _ => std::boxed::Box::new(unsafe { arena.alloc_dst_arc::<[u8]>(Layout::new::<()>(), 0, slice) }),
    }
}

#[cfg(feature = "dst")]
#[test]
fn gamma_survivor_empty_dst_exact_interior_byte_alignment_and_cold_routing() {
    #[derive(Debug)]
    #[repr(align(2048))]
    struct AlignedNonzero {
        _storage: [u8; 2048],
    }

    let word = core::mem::size_of::<usize>();
    for family in 0..6 {
        let arena = Arena::builder().with_capacity(512).build();
        let first = arena.alloc(3_u8);
        let cursor = (&raw const *first).addr() + 1;
        let address = std::cell::Cell::new(0);
        let allocation = gamma_survivor_empty_dst_owner(&arena, family, &address);
        let expected = match family {
            0 => cursor,
            1 => cursor + 4,
            2 => cursor.div_ceil(4) * 4 + 4,
            3 => cursor + word,
            4 => cursor + 4 + word,
            _ => cursor.div_ceil(4) * 4 + 4 + word,
        };
        assert_eq!(address.get(), expected, "family {family}");
        let next = arena.alloc(5_u8);
        assert_eq!((&raw const *next).addr(), expected + 1);
        drop(allocation);
    }
    for family in 0..3 {
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let init = |pointer: *mut [AlignedNonzero]| {
            assert_eq!(pointer.cast::<u8>().addr() % 2048, 0);
        };
        let layout = Layout::from_size_align(0, 2048).unwrap();
        let allocation: std::boxed::Box<dyn core::fmt::Debug> = match family {
            // SAFETY: the layout/metadata describe an empty aligned slice, with no elements to initialize.
            0 => std::boxed::Box::new(unsafe { arena.alloc_dst_box::<[AlignedNonzero]>(layout, 0, init) }),
            // SAFETY: the layout/metadata describe an empty aligned slice, with no elements to initialize.
            1 => std::boxed::Box::new(unsafe { arena.alloc_dst_rc::<[AlignedNonzero]>(layout, 0, init) }),
            // SAFETY: the layout/metadata describe an empty aligned slice, with no elements to initialize.
            _ => std::boxed::Box::new(unsafe { arena.alloc_dst_arc::<[AlignedNonzero]>(layout, 0, init) }),
        };
        #[cfg(feature = "stats")]
        {
            assert_eq!(arena.stats().normal_chunks_allocated, 0);
            assert_eq!(arena.stats().oversized_chunks_allocated, 1);
        }
        drop(allocation);
    }
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_nonfreezable_nonzero_vector_exact_reservation() {
    #[repr(align(32768))]
    struct AlignedNonzero {
        _storage: [u8; 32768],
    }

    let arena = Arena::builder().max_normal_alloc(4096).build();
    let values = arena.alloc_vec_with_capacity::<AlignedNonzero>(1);
    assert_eq!(values.len(), 0);
    assert_eq!(values.capacity(), 1);
    assert_eq!(values.as_ptr().addr() % 32768, 0);
    let word = core::mem::size_of::<usize>();
    let header = 5 * word + core::mem::size_of::<u32>();
    let hint = 65536_usize;
    let expected = (header + (hint + word - 1).div_ceil(word) * word).div_ceil(word) * word;
    assert_eq!(arena.stats().total_bytes_allocated, u64::try_from(expected).unwrap());
    assert_eq!(arena.stats().oversized_chunks_allocated, 1);
    let mut empty = arena.alloc_vec::<AlignedNonzero>();
    gamma_survivor_expect_error(empty.try_reserve(usize::MAX / core::mem::size_of::<AlignedNonzero>()));
    assert_eq!(empty.capacity(), 0);
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_vector_relocation_counts_exclude_initial_and_in_place_reservations() {
    for block_growth in [false, true] {
        let arena = Arena::builder().with_capacity(4096).build();
        let mut values = arena.alloc_vec_with_capacity::<u64>(1);
        values.push(17);
        assert_eq!(arena.stats().relocations, 0);
        assert_eq!(arena.stats().relocations_since_reset, 0);
        let before = values.as_ptr();
        let blocker = block_growth.then(|| arena.alloc(197_u8));
        values.try_reserve_exact(1).unwrap();
        assert_eq!(values.capacity(), 2);
        assert_eq!(values.as_ptr() != before, block_growth);
        assert_eq!(arena.stats().relocations, u64::from(block_growth));
        assert_eq!(arena.stats().relocations_since_reset, u64::from(block_growth));
        assert_eq!(values.as_slice(), &[17]);
        if let Some(blocker) = blocker {
            assert_eq!(*blocker, 197);
        }
    }
}

#[test]
fn gamma_survivor_empty_append_preserves_split_storage_capacities() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity::<u8>(8);
    values.extend_from_slice([1, 2, 3, 4, 5, 6]);
    let mut tail = values.split_off(3);
    assert_eq!(tail.as_slice(), &[4, 5, 6]);
    tail.clear();
    assert_eq!(values.capacity(), 3);
    assert_eq!(tail.capacity(), 5);
    values.try_append(&mut tail).unwrap();
    assert_eq!(values.as_slice(), &[1, 2, 3]);
    assert_eq!(values.capacity(), 3);
    assert_eq!(tail.capacity(), 5);
    assert!(tail.is_empty());
}

#[test]
fn gamma_survivor_split_routing_preserves_empty_tail_and_zst_capacities() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity::<u8>(8);
    values.extend_from_slice([1, 2, 3]);
    let tail = values.try_split_off(3).unwrap();
    assert_eq!(values.capacity(), 8);
    assert_eq!(values.as_slice(), &[1, 2, 3]);
    assert!(tail.is_empty());
    assert_eq!(tail.capacity(), 0);
    let mut single = arena.alloc_vec_with_capacity::<u8>(1);
    single.push(7);
    let tail = single.try_split_off(0).unwrap();
    assert_eq!(single.capacity(), 0);
    assert_eq!(tail.as_slice(), &[7]);
    let mut units = arena.alloc_vec::<()>();
    units.extend([(); 7]);
    let tail = units.try_split_off(2).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(tail.len(), 5);
    assert_eq!(units.capacity(), usize::MAX);
    assert_eq!(tail.capacity(), usize::MAX);
}

#[cfg(feature = "dst")]
#[test]
fn gamma_survivor_dst_nonzero_alignment_is_checked_before_initialization() {
    for family in 0..3 {
        let arena = Arena::builder().with_capacity(512).build();
        let _first = arena.alloc(1_u8);
        let init = |pointer: *mut u128| {
            assert_eq!(pointer.addr() % core::mem::align_of::<u128>(), 0);
            // SAFETY: the preceding assertion establishes alignment and the callback owns a full u128 slot.
            unsafe { pointer.write(197) };
        };
        let allocation: std::boxed::Box<dyn core::fmt::Debug> = match family {
            // SAFETY: the layout matches u128 and init fully initializes the fresh reservation.
            0 => std::boxed::Box::new(unsafe { arena.alloc_dst_box::<u128>(Layout::new::<u128>(), (), init) }),
            // SAFETY: the layout matches u128 and init fully initializes the fresh reservation.
            1 => std::boxed::Box::new(unsafe { arena.alloc_dst_rc::<u128>(Layout::new::<u128>(), (), init) }),
            // SAFETY: the layout matches u128 and init fully initializes the fresh reservation.
            _ => std::boxed::Box::new(unsafe { arena.alloc_dst_arc::<u128>(Layout::new::<u128>(), (), init) }),
        };
        drop(allocation);
    }
}
