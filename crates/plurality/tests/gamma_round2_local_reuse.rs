// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowed allocations use the local free-list path, not the owned-handle path.

#[test]
fn gamma_round2_multi_pool_debug_identifies_the_layout_count() {
    let layout_count = |pool: &plurality::MultiPool| {
        let diagnostic = format!("{pool:?}");
        let (_, fields) = diagnostic.split_once(" { ").unwrap();
        fields
            .split(", ")
            .find_map(|field| field.strip_prefix("layouts: "))
            .unwrap()
            .parse::<usize>()
            .unwrap()
    };
    let pool = plurality::MultiPool::new();
    assert_eq!(layout_count(&pool), 0);
    let _first = pool.alloc(1_u32);
    let _second = pool.alloc(2_u128);
    assert_eq!(layout_count(&pool), 2);
}

#[test]
fn gamma_round2_borrowed_slots_from_nonzero_chunks_keep_distinct_free_list_links() {
    let pool = plurality::Pool::<u32>::builder().chunk_size(2).max_chunks(3).build();
    let mut held = (10..16).map(|value| Some(pool.try_alloc(value).unwrap())).collect::<Vec<_>>();
    for index in [3, 5, 1, 4, 2, 0] {
        assert_eq!(**held[index].as_ref().unwrap(), 10 + u32::try_from(index).unwrap());
        drop(held[index].take());
    }
    let reused = (20..26).map(|value| pool.try_alloc(value).unwrap()).collect::<Vec<_>>();
    assert_eq!(pool.chunks_allocated(), 3);
    for (index, value) in reused.iter().enumerate() {
        assert_eq!(**value, 20 + u32::try_from(index).unwrap());
        for other in &reused[..index] {
            assert!(!std::ptr::eq(&raw const **value, &raw const **other));
        }
    }
    pool.try_alloc(99).unwrap_err();
}
