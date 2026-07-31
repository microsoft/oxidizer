// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared synthetic wide-record batch workload and hot paths.

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;

#[cfg(feature = "stats")]
use multitude::ArenaStats;
use multitude::de::{DeserializationLimits, DeserializeIn};
use multitude::{Arena, Box as ArenaBox};

pub(crate) const BATCH_RECORDS: usize = 16;
pub(crate) const REFRESH_RECORDS: usize = 1_000;
const ARENA_CAPACITY: usize = 256 * 1024;
const RETAIN_EVERY: u64 = 8;
const GLOBAL_RETAINED: usize = 32;

type StandardRetained = BTreeMap<u64, (String, String)>;
type ArenaRetained = BTreeMap<u64, (ArenaBox<str>, ArenaBox<str>)>;

#[derive(serde::Deserialize)]
pub(crate) struct StandardEndpoint {
    host: String,
    port: u16,
    secure: bool,
}

#[derive(serde::Deserialize)]
pub(crate) struct StandardMetadata {
    region: String,
    zone: String,
    annotations: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct StandardRecord {
    pub(crate) id: u64,
    generation: u32,
    shard: u16,
    priority: i32,
    enabled: bool,
    archived: bool,
    ratio: f64,
    weight: u64,
    retries: u8,
    timeout_ms: u32,
    created_at: u64,
    updated_at: u64,
    pub(crate) name: String,
    namespace: String,
    description: String,
    owner: Option<String>,
    note: Option<String>,
    tags: Vec<String>,
    samples: Vec<u32>,
    labels: BTreeMap<String, String>,
    capabilities: BTreeSet<String>,
    endpoint: StandardEndpoint,
    payload: String,
    metadata: StandardMetadata,
    checksum: String,
}

#[derive(serde::Deserialize)]
struct FilterEndpoint {
    port: u16,
    secure: bool,
}

#[derive(serde::Deserialize)]
struct FilterHeader {
    id: u64,
    generation: u32,
    shard: u16,
    priority: i32,
    enabled: bool,
    archived: bool,
    ratio: f64,
    weight: u64,
    retries: u8,
    timeout_ms: u32,
    updated_at: u64,
    endpoint: FilterEndpoint,
}

struct Candidate {
    index: usize,
    id: u64,
    generation: u32,
    shard: u16,
    priority: i32,
    enabled: bool,
    archived: bool,
    ratio: f64,
    weight: u64,
    retries: u8,
    timeout_ms: u32,
    updated_at: u64,
    port: u16,
    secure: bool,
}

#[derive(serde::Deserialize)]
pub(crate) struct LazyStandardRecord<'input> {
    pub(crate) id: u64,
    generation: u32,
    shard: u16,
    priority: i32,
    enabled: bool,
    archived: bool,
    ratio: f64,
    weight: u64,
    retries: u8,
    timeout_ms: u32,
    created_at: u64,
    updated_at: u64,
    pub(crate) name: String,
    namespace: String,
    description: String,
    owner: Option<String>,
    note: Option<String>,
    tags: Vec<String>,
    samples: Vec<u32>,
    labels: BTreeMap<String, String>,
    capabilities: BTreeSet<String>,
    endpoint: StandardEndpoint,
    #[serde(borrow)]
    payload: &'input serde_json::value::RawValue,
    metadata: StandardMetadata,
    checksum: String,
}

#[derive(DeserializeIn)]
pub(crate) struct ArenaEndpoint {
    host: ArenaBox<str>,
    port: u16,
    secure: bool,
}

#[derive(DeserializeIn)]
pub(crate) struct ArenaMetadata {
    region: ArenaBox<str>,
    zone: ArenaBox<str>,
    annotations: BTreeMap<ArenaBox<str>, ArenaBox<str>>,
}

#[derive(DeserializeIn)]
pub(crate) struct ArenaRecord {
    pub(crate) id: u64,
    generation: u32,
    shard: u16,
    priority: i32,
    enabled: bool,
    archived: bool,
    ratio: f64,
    weight: u64,
    retries: u8,
    timeout_ms: u32,
    created_at: u64,
    updated_at: u64,
    pub(crate) name: ArenaBox<str>,
    namespace: ArenaBox<str>,
    description: ArenaBox<str>,
    owner: Option<ArenaBox<str>>,
    note: Option<ArenaBox<str>>,
    tags: ArenaBox<[ArenaBox<str>]>,
    samples: ArenaBox<[u32]>,
    labels: BTreeMap<ArenaBox<str>, ArenaBox<str>>,
    capabilities: BTreeSet<ArenaBox<str>>,
    endpoint: ArenaEndpoint,
    payload: ArenaBox<str>,
    metadata: ArenaMetadata,
    checksum: ArenaBox<str>,
}

pub(crate) struct RecordBatchState {
    pub(crate) arena: Arena,
    pub(crate) input: Vec<u8>,
}

pub(crate) struct ReusableVectorState {
    pub(crate) values: multitude::vec::Vec<'static, ArenaRecord>,
    pub(crate) input: Vec<u8>,
}

pub(crate) struct StandardRefreshState {
    input: Vec<u8>,
    retained: Option<StandardRetained>,
}

pub(crate) struct ArenaRefreshState {
    arena: Arena,
    input: Vec<u8>,
    retained: Option<ArenaRetained>,
}

pub(crate) fn workload_json(escaped: bool) -> Vec<u8> {
    workload_json_with_records(BATCH_RECORDS, escaped)
}

pub(crate) fn workload_json_with_records(record_count: usize, escaped: bool) -> Vec<u8> {
    let records = (0..record_count)
        .map(|index| {
            let priority_index = i32::try_from(index).expect("fixed record batch index fits in i32");
            let text = |label: &str| {
                if escaped {
                    format!("{label} \"quoted\" line {index}\npath\\segment")
                } else {
                    format!("{label}-plain-record-{index}")
                }
            };

            serde_json::json!({
                "id": index,
                "generation": if index % 11 == 0 { 3 } else { 4 },
                "shard": index % 8,
                "priority": 100 - priority_index,
                "enabled": index % 2 == 0,
                "archived": index % 13 == 0,
                "ratio": if index % 7 == 0 { 0.5 } else { 0.75 },
                "weight": 10_000 + index,
                "retries": if index % 17 == 0 { 5 } else { 3 },
                "timeout_ms": if index % 19 == 0 { 5_000 } else { 2_500 },
                "created_at": 1_700_000_000 + index,
                "updated_at": 1_700_010_000 + index,
                "name": text("service"),
                "namespace": text("namespace"),
                "description": text("representative string-heavy record description"),
                "owner": if index % 3 == 0 { None } else { Some(text("owner")) },
                "note": (index % 4 == 0).then(|| text("optional note")),
                "tags": [text("frontend"), text("production"), text("latency-sensitive")],
                "samples": [1, 2, 3, 5, 8, 13],
                "labels": {
                    "team": text("runtime"),
                    "environment": text("production"),
                    "component": text("gateway")
                },
                "capabilities": [text("read"), text("write"), text("observe")],
                "endpoint": {
                    "host": text("service.internal.example"),
                    "port": if index % 23 == 0 { 80 } else { 443 },
                    "secure": index % 29 != 0
                },
                "payload": text(&"payload body ".repeat(64)),
                "metadata": {
                    "region": text("westus"),
                    "zone": text("zone-a"),
                    "annotations": {
                        "source": text("synthetic"),
                        "refresh": text("periodic")
                    }
                },
                "checksum": format!("{index:016x}{:016x}", index.wrapping_mul(17))
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_vec(&records).expect("synthetic record values are always JSON serializable")
}

pub(crate) fn malformed_json() -> Vec<u8> {
    let mut input = workload_json(false);
    assert_eq!(input.pop(), Some(b']'), "serialized record batch ends with an array delimiter");
    input
}

pub(crate) fn warm_arena() -> Arena {
    let arena = Arena::builder().with_capacity(ARENA_CAPACITY).build();
    let value = arena.alloc_box(0_u64);
    drop(value);
    arena
}

pub(crate) fn warm_cached_arena(input: &[u8]) -> Arena {
    let mut arena = warm_arena();
    arena_vec_baseline_hot_path(&mut arena, input);
    arena
}

pub(crate) fn warm_streaming_arena(input: &[u8]) -> Arena {
    let mut arena = warm_arena();
    arena
        .deserialize_json_each(input, |_: ArenaRecord| {})
        .expect("benchmark JSON is valid");
    drop(arena.alloc_vec_with_capacity::<ArenaRecord>(REFRESH_RECORDS));
    arena.reset();
    arena
}

pub(crate) fn unescaped_state() -> RecordBatchState {
    let input = workload_json(false);
    RecordBatchState {
        arena: warm_cached_arena(&input),
        input,
    }
}

pub(crate) fn escaped_state() -> RecordBatchState {
    let input = workload_json(true);
    RecordBatchState {
        arena: warm_cached_arena(&input),
        input,
    }
}

pub(crate) fn reusable_vector_state() -> ReusableVectorState {
    let arena = Box::leak(Box::new(warm_arena()));
    let input = workload_json(false);
    let mut values = arena.alloc_vec();
    arena_vec_reuse_hot_path(&mut values, &input);
    ReusableVectorState { values, input }
}

pub(crate) fn reset_recreate_state() -> RecordBatchState {
    let input = workload_json(false);
    RecordBatchState {
        arena: warm_cached_arena(&input),
        input,
    }
}

pub(crate) fn malformed_state() -> RecordBatchState {
    let input = malformed_json();
    let mut arena = warm_arena();
    malformed_arena_hot_path(&mut arena, &input);
    RecordBatchState { arena, input }
}

pub(crate) fn standard_refresh_state() -> StandardRefreshState {
    StandardRefreshState {
        input: workload_json_with_records(REFRESH_RECORDS, true),
        retained: None,
    }
}

pub(crate) fn arena_vec_refresh_state() -> ArenaRefreshState {
    let input = workload_json_with_records(REFRESH_RECORDS, true);
    ArenaRefreshState {
        arena: warm_cached_arena(&input),
        input,
        retained: None,
    }
}

pub(crate) fn arena_each_refresh_state() -> ArenaRefreshState {
    let input = workload_json_with_records(REFRESH_RECORDS, true);
    ArenaRefreshState {
        arena: warm_streaming_arena(&input),
        input,
        retained: None,
    }
}

pub(crate) fn arena_raw_each_refresh_state() -> ArenaRefreshState {
    let input = workload_json_with_records(REFRESH_RECORDS, true);
    let mut arena = warm_arena();
    drop(refresh_arena_raw_each_hot_path(&mut arena, &input));
    arena.reset();
    ArenaRefreshState {
        arena,
        input,
        retained: None,
    }
}

pub(crate) fn arena_raw_index_refresh_state() -> ArenaRefreshState {
    let input = workload_json_with_records(REFRESH_RECORDS, true);
    let mut arena = warm_arena();
    drop(refresh_arena_raw_index_hot_path(&mut arena, &input));
    arena.reset();
    ArenaRefreshState {
        arena,
        input,
        retained: None,
    }
}

#[inline(never)]
pub(crate) fn standard_vec_hot_path(input: &[u8]) {
    let values: Vec<StandardRecord> = serde_json::from_slice(black_box(input)).expect("benchmark JSON is valid");
    let summary = (values.len(), values.first().map_or(0, |record| record.name.len()));
    black_box(summary);
    drop(values);
}

#[inline(never)]
pub(crate) fn arena_box_slice_hot_path(arena: &mut Arena, input: &[u8]) {
    let values: ArenaBox<[ArenaRecord]> = arena.deserialize_json(black_box(input)).expect("benchmark JSON is valid");
    let summary = (values.len(), values.first().map_or(0, |record| record.name.len()));
    black_box(summary);
    drop(values);
    arena.reset();
}

#[inline(never)]
pub(crate) fn deserialize_arena_vec(values: &mut multitude::vec::Vec<'_, ArenaRecord>, input: &[u8]) {
    values
        .deserialize_json_reusing(black_box(input))
        .expect("benchmark JSON matches the arena record");
    black_box((values.len(), values.first().map_or(0, |record| record.name.len())));
}

#[inline(never)]
pub(crate) fn arena_vec_baseline_hot_path(arena: &mut Arena, input: &[u8]) {
    let mut values = arena.alloc_vec();
    deserialize_arena_vec(&mut values, input);
    drop(values);
    arena.reset();
}

#[inline(never)]
pub(crate) fn arena_vec_reuse_hot_path(values: &mut multitude::vec::Vec<'_, ArenaRecord>, input: &[u8]) {
    deserialize_arena_vec(values, input);
}

#[inline(never)]
pub(crate) fn repeated_no_reset_iteration(state: &mut ReusableVectorState) {
    arena_vec_reuse_hot_path(&mut state.values, &state.input);
}

#[inline(never)]
pub(crate) fn reset_recreate_hot_path(arena: &mut Arena, input: &[u8]) {
    arena.reset();
    let mut values = arena.alloc_vec();
    deserialize_arena_vec(&mut values, input);
    drop(values);
}

#[inline(never)]
pub(crate) fn sparse_standard_hot_path(input: &[u8]) {
    let values: Vec<StandardRecord> = serde_json::from_slice(black_box(input)).expect("benchmark JSON is valid");
    let retained: BTreeMap<u64, (String, String)> = values
        .into_iter()
        .filter(|record| record.id % RETAIN_EVERY == 0)
        .map(|record| (record.id, (record.name, record.payload)))
        .collect();
    black_box(&retained);
    drop(retained);
}

#[inline(never)]
pub(crate) fn sparse_lazy_standard_hot_path(input: &[u8]) {
    let values: Vec<LazyStandardRecord<'_>> = serde_json::from_slice(black_box(input)).expect("benchmark JSON is valid");
    let retained: BTreeMap<u64, (String, String)> = values
        .into_iter()
        .filter(|record| record.id % RETAIN_EVERY == 0)
        .map(|record| {
            let payload = serde_json::from_str(record.payload.get()).expect("raw payload is a JSON string");
            (record.id, (record.name, payload))
        })
        .collect();
    black_box(&retained);
    drop(retained);
}

#[inline(never)]
pub(crate) fn sparse_arena_hot_path(arena: &mut Arena, input: &[u8]) {
    let mut values = arena.alloc_vec();
    deserialize_arena_vec(&mut values, input);
    let retained: BTreeMap<u64, (ArenaBox<str>, ArenaBox<str>)> = values
        .into_iter()
        .filter(|record| record.id % RETAIN_EVERY == 0)
        .map(|record| (record.id, (record.name, record.payload)))
        .collect();
    black_box(&retained);
    drop(retained);
    arena.reset();
}

#[inline(never)]
pub(crate) fn refresh_standard_hot_path(input: &[u8]) -> StandardRetained {
    let values: Vec<StandardRecord> = serde_json::from_slice(black_box(input)).expect("benchmark JSON is valid");
    let selected = select_global(
        values
            .iter()
            .enumerate()
            .map(|(index, record)| candidate_from_standard(index, record)),
    );
    retain_selected_standard(values, &selected)
}

#[inline(never)]
pub(crate) fn refresh_arena_vec_hot_path(arena: &mut Arena, input: &[u8]) -> ArenaRetained {
    arena.reset();
    let mut values = arena.alloc_vec();
    deserialize_arena_vec(&mut values, input);
    let selected = select_global(values.iter().enumerate().map(|(index, record)| candidate_from_arena(index, record)));
    values
        .into_iter()
        .enumerate()
        .filter(|(index, _)| selected.binary_search(index).is_ok())
        .map(|(_, record)| (record.id, (record.name, record.payload)))
        .collect()
}

#[inline(never)]
pub(crate) fn refresh_arena_each_hot_path(arena: &mut Arena, input: &[u8]) -> ArenaRetained {
    arena.reset();
    let mut values = arena.alloc_vec_with_capacity(REFRESH_RECORDS);
    arena
        .deserialize_json_each(black_box(input), |record: ArenaRecord| values.push(record))
        .expect("benchmark JSON is valid");
    let selected = select_global(values.iter().enumerate().map(|(index, record)| candidate_from_arena(index, record)));
    values
        .into_iter()
        .enumerate()
        .filter(|(index, _)| selected.binary_search(index).is_ok())
        .map(|(_, record)| (record.id, (record.name, record.payload)))
        .collect()
}

#[inline(never)]
pub(crate) fn refresh_arena_raw_each_hot_path(arena: &mut Arena, input: &[u8]) -> ArenaRetained {
    arena.reset();
    let mut indexed = Vec::with_capacity(REFRESH_RECORDS);
    let mut candidates = Vec::with_capacity(REFRESH_RECORDS);
    arena
        .deserialize_json_each(black_box(input), |raw: &serde_json::value::RawValue| {
            let header: FilterHeader = serde_json::from_str(raw.get()).expect("benchmark record has a filter header");
            candidates.push(candidate_from_header(indexed.len(), &header));
            indexed.push(raw);
        })
        .expect("benchmark JSON is valid");
    let selected = select_global(candidates);
    materialize_selected(arena, &indexed, &selected)
}

#[inline(never)]
pub(crate) fn refresh_arena_raw_index_hot_path(arena: &mut Arena, input: &[u8]) -> ArenaRetained {
    arena.reset();
    let indexed: Vec<&serde_json::value::RawValue> = serde_json::from_slice(black_box(input)).expect("benchmark JSON is a raw-value array");
    let selected = select_global(indexed.iter().enumerate().map(|(index, raw)| {
        let header: FilterHeader = serde_json::from_str(raw.get()).expect("benchmark record has a filter header");
        candidate_from_header(index, &header)
    }));
    materialize_selected(arena, &indexed, &selected)
}

#[inline(never)]
pub(crate) fn standard_refresh_iteration(state: &mut StandardRefreshState) {
    state.retained = Some(refresh_standard_hot_path(&state.input));
    black_box(&state.retained);
}

#[inline(never)]
pub(crate) fn arena_vec_refresh_iteration(state: &mut ArenaRefreshState) {
    state.retained = Some(refresh_arena_vec_hot_path(&mut state.arena, &state.input));
    black_box(&state.retained);
}

#[inline(never)]
pub(crate) fn arena_each_refresh_iteration(state: &mut ArenaRefreshState) {
    state.retained = Some(refresh_arena_each_hot_path(&mut state.arena, &state.input));
    black_box(&state.retained);
}

#[inline(never)]
pub(crate) fn arena_raw_each_refresh_iteration(state: &mut ArenaRefreshState) {
    state.retained = Some(refresh_arena_raw_each_hot_path(&mut state.arena, &state.input));
    black_box(&state.retained);
}

#[inline(never)]
pub(crate) fn arena_raw_index_refresh_iteration(state: &mut ArenaRefreshState) {
    state.retained = Some(refresh_arena_raw_index_hot_path(&mut state.arena, &state.input));
    black_box(&state.retained);
}

fn select_global(candidates: impl IntoIterator<Item = Candidate>) -> Vec<usize> {
    let mut candidates: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.enabled
                && !candidate.archived
                && candidate.generation == 4
                && candidate.secure
                && candidate.port == 443
                && candidate.timeout_ms <= 2_500
                && candidate.retries <= 3
                && candidate.ratio >= 0.75
                && candidate.shard != 3
        })
        .collect();
    candidates.sort_unstable_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| right.weight.cmp(&left.weight))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.truncate(GLOBAL_RETAINED);
    let mut selected: Vec<_> = candidates.into_iter().map(|candidate| candidate.index).collect();
    selected.sort_unstable();
    selected
}

fn candidate_from_standard(index: usize, record: &StandardRecord) -> Candidate {
    Candidate {
        index,
        id: record.id,
        generation: record.generation,
        shard: record.shard,
        priority: record.priority,
        enabled: record.enabled,
        archived: record.archived,
        ratio: record.ratio,
        weight: record.weight,
        retries: record.retries,
        timeout_ms: record.timeout_ms,
        updated_at: record.updated_at,
        port: record.endpoint.port,
        secure: record.endpoint.secure,
    }
}

fn candidate_from_arena(index: usize, record: &ArenaRecord) -> Candidate {
    Candidate {
        index,
        id: record.id,
        generation: record.generation,
        shard: record.shard,
        priority: record.priority,
        enabled: record.enabled,
        archived: record.archived,
        ratio: record.ratio,
        weight: record.weight,
        retries: record.retries,
        timeout_ms: record.timeout_ms,
        updated_at: record.updated_at,
        port: record.endpoint.port,
        secure: record.endpoint.secure,
    }
}

fn candidate_from_header(index: usize, header: &FilterHeader) -> Candidate {
    Candidate {
        index,
        id: header.id,
        generation: header.generation,
        shard: header.shard,
        priority: header.priority,
        enabled: header.enabled,
        archived: header.archived,
        ratio: header.ratio,
        weight: header.weight,
        retries: header.retries,
        timeout_ms: header.timeout_ms,
        updated_at: header.updated_at,
        port: header.endpoint.port,
        secure: header.endpoint.secure,
    }
}

fn retain_selected_standard(values: Vec<StandardRecord>, selected: &[usize]) -> StandardRetained {
    values
        .into_iter()
        .enumerate()
        .filter(|(index, _)| selected.binary_search(index).is_ok())
        .map(|(_, record)| (record.id, (record.name, record.payload)))
        .collect()
}

fn materialize_selected(arena: &Arena, indexed: &[&serde_json::value::RawValue], selected: &[usize]) -> ArenaRetained {
    selected
        .iter()
        .map(|&index| {
            let record: ArenaRecord = arena
                .deserialize_json(indexed[index].get())
                .expect("benchmark record matches the arena schema");
            (record.id, (record.name, record.payload))
        })
        .collect()
}

#[inline(never)]
pub(crate) fn malformed_standard_hot_path(input: &[u8]) {
    let result = serde_json::from_slice::<Vec<StandardRecord>>(black_box(input));
    assert!(result.is_err(), "malformed benchmark JSON must be rejected");
    let _ = black_box(result);
}

#[inline(never)]
pub(crate) fn malformed_arena_hot_path(arena: &mut Arena, input: &[u8]) {
    let result = arena.deserialize_json::<ArenaBox<[ArenaRecord]>, _>(black_box(input));
    assert!(result.is_err(), "malformed benchmark JSON must be rejected");
    let _ = black_box(result);
    arena.reset();
}

#[inline(never)]
pub(crate) fn resource_limited_hot_path(arena: &mut Arena, input: &[u8]) {
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(BATCH_RECORDS - 1);
    let result = arena.deserialize_json_with_limits::<ArenaBox<[ArenaRecord]>, _>(black_box(input), limits);
    assert!(result.is_err(), "the batch exceeds the configured sequence limit");
    let _ = black_box(result);
    arena.reset();
}

#[cfg(feature = "stats")]
pub(crate) fn diagnostic_stats(input: &[u8]) -> (Arena, ArenaStats, ArenaStats) {
    let arena = warm_arena();
    let values: ArenaBox<[ArenaRecord]> = arena.deserialize_json(input).expect("benchmark JSON is valid");
    let live = arena.stats();
    drop(values);
    let released = arena.stats();
    (arena, live, released)
}
