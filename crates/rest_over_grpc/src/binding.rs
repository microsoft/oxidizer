// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Binding`] captured path-variable value type.

/// A single captured path-variable binding produced by a generated router.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::Binding;
///
/// let binding = Binding::new(&["shelf", "id"], "7");
/// assert_eq!(binding.field_path(), &["shelf", "id"]);
/// assert_eq!(binding.value(), "7");
/// assert_eq!(Binding::EMPTY.value(), "");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding<'p> {
    field_path: &'static [&'static str],
    value: &'p str,
}

impl<'p> Binding<'p> {
    /// An empty binding (no field path, empty value), used as a filler for the
    /// inline storage in [`RouteMatch`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Binding;
    ///
    /// assert!(Binding::EMPTY.field_path().is_empty());
    /// assert_eq!(Binding::EMPTY.value(), "");
    /// ```
    pub const EMPTY: Binding<'static> = Binding::new(&[], "");

    /// Creates a binding for `field_path` capturing `value`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Binding;
    ///
    /// let binding = Binding::new(&["book"], "rust");
    /// assert_eq!(binding.field_path(), &["book"]);
    /// assert_eq!(binding.value(), "rust");
    /// ```
    #[must_use]
    pub const fn new(field_path: &'static [&'static str], value: &'p str) -> Self {
        Self { field_path, value }
    }

    /// The dotted message-field path this binding targets, e.g.
    /// `["shelf", "id"]`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Binding;
    ///
    /// let binding = Binding::new(&["shelf", "id"], "7");
    /// assert_eq!(binding.field_path(), &["shelf", "id"]);
    /// ```
    #[must_use]
    pub const fn field_path(&self) -> &'static [&'static str] {
        self.field_path
    }

    /// The captured value, borrowed from the original request path.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Binding;
    ///
    /// let binding = Binding::new(&["shelf"], "7");
    /// assert_eq!(binding.value(), "7");
    /// ```
    #[must_use]
    pub const fn value(&self) -> &'p str {
        self.value
    }
}
