// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use super::Value;

/// One ordered map entry. Map keys are values because Serde permits non-string keys.
///
/// ```
/// use multitude::de::{Entry, Value};
///
/// let entry: Entry = Entry {
///     key: Value::Bool(false),
///     value: Value::Bool(true),
/// };
/// assert!(matches!(entry.key, Value::Bool(false)));
/// ```
#[derive(Debug, PartialEq)]
pub struct Entry<A: Allocator + Clone = Global> {
    /// The map key.
    ///
    /// ```
    /// use multitude::de::{Entry, Value};
    ///
    /// let entry: Entry = Entry {
    ///     key: Value::Bool(false),
    ///     value: Value::Bool(true),
    /// };
    /// assert!(matches!(entry.key, Value::Bool(false)));
    /// ```
    pub key: Value<A>,
    /// The map value.
    ///
    /// ```
    /// use multitude::de::{Entry, Value};
    ///
    /// let entry: Entry = Entry {
    ///     key: Value::Bool(false),
    ///     value: Value::Bool(true),
    /// };
    /// assert!(matches!(entry.value, Value::Bool(true)));
    /// ```
    pub value: Value<A>,
}
