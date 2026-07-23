// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::Global;

use super::Entry;
use crate::Box;

/// An ordered Serde map. Entry order and duplicate keys are preserved.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::{Entry, Map, Value};
///
/// # fn main() -> Result<(), multitude::AllocError> {
/// let arena = Arena::new();
/// let map: Map = arena.try_alloc_slice_fill_iter_box([Entry {
///     key: Value::Bool(false),
///     value: Value::Bool(true),
/// }])?;
/// assert!(matches!(map[0].value, Value::Bool(true)));
/// # Ok(())
/// # }
/// ```
pub type Map<A = Global> = Box<[Entry<A>], A>;
