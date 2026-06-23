// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Object layer: the typed `Value` model and its conversions.

use crate::rc::{RcArray, RcBinary, RcStr, RcUtf16Str};

/// Arena object model. Leaves are reference-counted handles ([`RcStr`],
/// [`RcUtf16Str`], [`RcBinary`], [`RcArray`]); because every arena handle is
/// *thin* (8 bytes), even for DSTs, this `Value` is a fixed 16 bytes and can
/// outlive the arena.
pub(crate) enum Value {
    Int(i64),
    Str(RcStr),
    Utf16(RcUtf16Str),
    Binary(RcBinary),
    Array(RcArray<Self>),
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<RcStr> for Value {
    fn from(value: RcStr) -> Self {
        Self::Str(value)
    }
}

impl From<RcBinary> for Value {
    fn from(value: RcBinary) -> Self {
        Self::Binary(value)
    }
}

impl From<RcArray<Self>> for Value {
    fn from(value: RcArray<Self>) -> Self {
        Self::Array(value)
    }
}

impl From<RcUtf16Str> for Value {
    fn from(value: RcUtf16Str) -> Self {
        Self::Utf16(value)
    }
}

/// Aggregate measurements of a materialized [`Value`] tree.
#[derive(Default)]
pub(crate) struct TreeStats {
    /// Number of [`Value`] nodes in the tree (every array element).
    pub objects: usize,
    /// Logical bytes the tree owns: every array's backing storage
    /// (`len * size_of::<Value>()`) plus the string and binary payloads.
    pub bytes: usize,
}

/// Walks `forest` and totals its node count and logical byte footprint.
#[must_use]
pub(crate) fn measure(forest: &[Value]) -> TreeStats {
    let mut stats = TreeStats::default();
    measure_into(forest, &mut stats);
    stats
}

fn measure_into(values: &[Value], stats: &mut TreeStats) {
    stats.objects += values.len();
    stats.bytes += size_of_val(values);
    for value in values {
        match value {
            Value::Int(_) => {}
            Value::Str(s) => stats.bytes += s.len(),
            Value::Binary(b) => stats.bytes += b.len(),
            Value::Array(children) => measure_into(children, stats),
            Value::Utf16(s) => stats.bytes += s.len() * size_of::<u16>(),
        }
    }
}
