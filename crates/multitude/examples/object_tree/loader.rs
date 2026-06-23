// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loader: bridges the backend [`DataAccess`] to the [`Value`] object model by
//! materializing a row forest into an arena.

use multitude::Arena;

use crate::backend::{DataAccess, RowReader};
use crate::object::Value;
use crate::rc::{RcArray, RcBinary, RcStr, RcUtf16Str};

/// Materializes the whole forest into `arena`.
#[must_use]
pub(crate) fn load(arena: &Arena, da: DataAccess<'_>) -> RcArray<Value> {
    RcArray::new(arena, da.rows().map(|r| load_object(arena, &r)))
}

/// Materializes one object (a row and everything beneath it). Each object is a
/// field array; the name is materialized both as a UTF-8 [`RcStr`] and a
/// UTF-16 [`RcUtf16Str`].
fn load_object(arena: &Arena, reader: &RowReader<'_>) -> Value {
    let children = reader.children();
    let child_array = RcArray::new(arena, children.rows().map(|c| load_object(arena, &c)));

    RcArray::new(
        arena,
        [
            reader.id().into(),
            RcStr::new(arena, reader.name()).into(),
            RcUtf16Str::new(arena, reader.name()).into(),
            RcBinary::new(arena, reader.blob()).into(),
            child_array.into(),
        ],
    )
    .into()
}
