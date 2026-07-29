// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared inputs and output types for the Serde benchmark pair.

use std::hint::black_box;

use multitude::de::{DeserializeIn, Value};
use multitude::{Arena, Box};
use serde::de::DeserializeSeed;

#[path = "shared/bumpalo_record.rs"]
mod bumpalo_record;

use bumpalo_record::{BumpRecord, BumpRecordSeed};

const ARENA_CAPACITY: usize = 4 * 1024;
const BATCH_RECORDS: usize = 32;

pub(crate) const INPUT: &[u8] = br#"{
    "id": 42,
    "name": "arena deserialization",
    "tags": ["serde", "json", "arena", "owned"],
    "metadata": {"active": true, "description": "representative nested input"}
}"#;

#[derive(DeserializeIn)]
pub(crate) struct ArenaMetadata {
    active: bool,
    description: Box<str>,
}

#[derive(DeserializeIn)]
pub(crate) struct ArenaRecord {
    id: u64,
    name: Box<str>,
    tags: Box<[Box<str>]>,
    metadata: ArenaMetadata,
}

#[derive(serde::Deserialize)]
pub(crate) struct StandardMetadata {
    active: bool,
    description: std::string::String,
}

#[derive(serde::Deserialize)]
pub(crate) struct StandardRecord {
    id: u64,
    name: std::string::String,
    tags: std::vec::Vec<std::string::String>,
    metadata: StandardMetadata,
}

pub(crate) struct ArenaOutput<T> {
    arena: Arena,
    value: Option<T>,
}

/// Preallocate and fault in arena storage, then prime smart-pointer allocation.
pub(crate) fn warm_arena() -> Arena {
    let arena = Arena::builder().with_capacity(ARENA_CAPACITY).build();
    let _ = arena.alloc_box(0_u64);
    arena
}

pub(crate) fn warm_reset_arena() -> Arena {
    let mut arena = warm_arena();
    arena.reset();
    arena
}

pub(crate) fn arena_output<T>() -> ArenaOutput<T> {
    ArenaOutput {
        arena: warm_arena(),
        value: None,
    }
}

// Keep both harnesses on the same out-of-line routines so optimization cannot
// make Criterion and Callgrind measure different bodies.
#[inline(never)]
pub(crate) fn typed_arena_hot_path(output: &mut ArenaOutput<ArenaRecord>) {
    output.value = Some(output.arena.deserialize_json(black_box(INPUT)).unwrap());
    let _ = black_box(output.value.as_ref());
}

#[inline(never)]
pub(crate) fn typed_standard_hot_path(output: &mut Option<StandardRecord>) {
    *output = Some(serde_json::from_slice(black_box(INPUT)).unwrap());
    let _ = black_box(output.as_ref());
}

#[inline(never)]
pub(crate) fn dynamic_arena_hot_path(output: &mut ArenaOutput<Value>) {
    output.value = Some(output.arena.deserialize_json(black_box(INPUT)).unwrap());
    let _ = black_box(output.value.as_ref());
}

#[inline(never)]
pub(crate) fn dynamic_standard_hot_path(output: &mut Option<serde_json::Value>) {
    *output = Some(serde_json::from_slice(black_box(INPUT)).unwrap());
    let _ = black_box(output.as_ref());
}

pub(crate) fn warm_bump() -> bumpalo::Bump {
    let mut bump = bumpalo::Bump::with_capacity(ARENA_CAPACITY);
    let _ = bump.alloc(0_u64);
    bump.reset();
    bump
}

#[inline(never)]
pub(crate) fn typed_multitude_lifecycle(arena: &mut Arena) {
    let value: ArenaRecord = arena.deserialize_json(black_box(INPUT)).unwrap();
    let summary = (
        value.id,
        value.name.len(),
        value.tags.iter().map(|tag| tag.len()).sum::<usize>(),
        value.metadata.active,
        value.metadata.description.len(),
    );
    let _ = black_box(summary);
    drop(value);
    arena.reset();
}

#[inline(never)]
pub(crate) fn typed_standard_lifecycle(state: &mut ()) {
    let value: StandardRecord = serde_json::from_slice(black_box(INPUT)).unwrap();
    let summary = (
        value.id,
        value.name.len(),
        value.tags.iter().map(String::len).sum::<usize>(),
        value.metadata.active,
        value.metadata.description.len(),
    );
    let _ = black_box(summary);
    drop(value);
    black_box(state);
}

#[inline(never)]
pub(crate) fn typed_bumpalo_lifecycle(bump: &mut bumpalo::Bump) {
    let mut deserializer = serde_json::Deserializer::from_slice(black_box(INPUT));
    let value = BumpRecordSeed(&*bump).deserialize(&mut deserializer).unwrap();
    deserializer.end().unwrap();
    let _ = black_box(value.summary());
    drop(value);
    bump.reset();
}

#[inline(never)]
pub(crate) fn batch_standard_lifecycle(state: &mut ()) {
    let values: std::vec::Vec<StandardRecord> = (0..BATCH_RECORDS)
        .map(|_| serde_json::from_slice(black_box(INPUT)).unwrap())
        .collect();
    let _ = black_box(&values);
    drop(values);
    black_box(state);
}

#[inline(never)]
pub(crate) fn batch_multitude_lifecycle(arena: &mut Arena) {
    let values: std::vec::Vec<ArenaRecord> = (0..BATCH_RECORDS)
        .map(|_| arena.deserialize_json(black_box(INPUT)).unwrap())
        .collect();
    let _ = black_box(&values);
    drop(values);
    arena.reset();
}

#[inline(never)]
pub(crate) fn batch_bumpalo_lifecycle(bump: &mut bumpalo::Bump) {
    let mut values: std::vec::Vec<BumpRecord<'_>> = std::vec::Vec::with_capacity(BATCH_RECORDS);
    for _ in 0..BATCH_RECORDS {
        let mut deserializer = serde_json::Deserializer::from_slice(black_box(INPUT));
        values.push(BumpRecordSeed(&*bump).deserialize(&mut deserializer).unwrap());
        deserializer.end().unwrap();
    }
    let _ = black_box(&values);
    drop(values);
    bump.reset();
}
