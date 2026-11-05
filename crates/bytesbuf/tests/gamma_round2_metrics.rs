// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Captures stable metric counts and magnitudes in an isolated executable.

#![cfg(feature = "std")]

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use nm::Report;

fn metric(name: &str) -> (u64, i64) {
    Report::collect()
        .events()
        .find(|event| event.name().as_ref() == name)
        .map_or((0, 0), |event| (event.count(), event.sum()))
}

#[test]
fn gamma_round2_pool_and_view_metrics_report_exact_counts_sizes_and_conversion_paths() {
    let instances = metric("bytesbuf_global_pool_instances_total");
    let requested = metric("bytesbuf_global_pool_reservation_requested_size");
    let rented = metric("bytesbuf_global_pool_block_rented_size");
    let memory = GlobalPool::new();
    assert_eq!(memory.reserve(0).remaining_capacity(), 0);
    assert!(
        !Report::collect()
            .events()
            .any(|event| event.name().as_ref() == "bytesbuf_global_pool_block_rented_size")
    );
    let requests = [1, 1024, 1025, 4096, 4097, 16_384, 16_385, 65_536, 65_537];
    let capacities = [1024, 1024, 4096, 4096, 16_384, 16_384, 65_536, 65_536, 131_072];
    for (request, capacity) in requests.into_iter().zip(capacities) {
        let mut buffer = memory.reserve(request);
        assert_eq!((buffer.len(), buffer.remaining_capacity()), (0, capacity));
        buffer.put_byte_repeated(0x71, capacity);
        assert_eq!(buffer.peek().to_vec(), vec![0x71; capacity]);
    }
    let actual = metric("bytesbuf_global_pool_instances_total");
    assert_eq!((actual.0 - instances.0, actual.1 - instances.1), (1, 1));
    let actual = metric("bytesbuf_global_pool_reservation_requested_size");
    assert_eq!(
        (actual.0 - requested.0, actual.1 - requested.1),
        (10, i64::try_from(requests.iter().sum::<usize>()).unwrap())
    );
    let actual = metric("bytesbuf_global_pool_block_rented_size");
    assert_eq!((actual.0 - rented.0, actual.1 - rented.1), (10, 305_152));

    let first = BytesView::from(vec![1, 2, 3]);
    let second = BytesView::from(vec![4, 5]);
    let created = metric("bytesbuf_view_created_spans");
    let combined = BytesView::from_views([first.clone(), second]);
    assert_eq!(combined.to_vec(), [1, 2, 3, 4, 5]);
    let actual = metric("bytesbuf_view_created_spans");
    assert_eq!((actual.0 - created.0, actual.1 - created.1), (1, 2));
    #[cfg(feature = "bytes-compat")]
    {
        let shared = metric("bytesbuf_view_to_bytes_shared");
        let copied = metric("bytesbuf_view_to_bytes_copied");
        assert!(BytesView::new().to_bytes().is_empty());
        let bytes = first.to_bytes();
        assert_eq!(bytes.as_ref(), [1, 2, 3]);
        assert_eq!(bytes.as_ptr(), first.first_slice().as_ptr());
        assert_eq!(combined.to_bytes().as_ref(), [1, 2, 3, 4, 5]);
        let actual = metric("bytesbuf_view_to_bytes_shared");
        assert_eq!((actual.0 - shared.0, actual.1 - shared.1), (2, 3));
        let actual = metric("bytesbuf_view_to_bytes_copied");
        assert_eq!((actual.0 - copied.0, actual.1 - copied.1), (1, 5));
    }
}
