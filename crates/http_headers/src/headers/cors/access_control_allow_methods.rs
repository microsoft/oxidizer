// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::shared::{CorsList, CorsListView, CorsMethodView, method_ref_validated, validate_list, validate_single_list};
use crate::sink::{FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

macro_rules! define_method_list {
    ($descriptor:ident, $owned:ident, $borrowed:ident, $name:expr) => {
        /// Defines the `Access-Control-Allow-Methods` header.
        ///
        /// # Specification
        ///
        /// Defined by the Fetch standard's
        /// [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-allow-methods).
        #[derive(Debug)]
        pub struct $descriptor {
            _private: (),
        }

        /// Owned value for the `Access-Control-Allow-Methods` header.
        ///
        /// # Specification
        ///
        /// Defined by the Fetch standard's [CORS protocol and credentials section].
        ///
        /// # Examples
        ///
        /// ```rust
        /// use http_headers::FieldValue;
        ///
        /// let value = http_headers::headers::AccessControlAllowMethodsOwned::try_from(
        ///     FieldValue::from_static("GET, POST"),
        /// )?;
        /// assert_eq!(value.iter().count(), 2);
        /// # Ok::<(), http_headers::DecodeError>(())
        /// ```
        ///
        /// `Access-Control-Allow-Methods: GET, POST, PUT` lists methods,
        /// `Access-Control-Allow-Methods: *` uses a wildcard, and an empty field
        /// value is also accepted.
        ///
        /// [CORS protocol and credentials section]: https://fetch.spec.whatwg.org/#http-access-control-allow-methods
        #[derive(Clone, Eq, Hash, PartialEq)]
        pub struct $owned(CorsList);

        /// Borrowed value for the `Access-Control-Allow-Methods` header.
        /// # Examples
        ///
        /// ```rust
        /// # #[cfg(feature = "http")]
        /// # fn main() -> Result<(), http_headers::DecodeError> {
        /// use http::HeaderMap;
        /// use http_headers::Field;
        /// use http_headers::headers::AccessControlAllowMethods;
        ///
        /// let mut headers = HeaderMap::new();
        /// headers.insert(
        ///     "access-control-allow-methods",
        ///     http::HeaderValue::from_static("GET, POST"),
        /// );
        /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
        /// assert_eq!(value.len(), 2);
        /// let fmt_output = format!("{value:?}");
        /// assert!(fmt_output.contains("method_count"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// # }
        /// # #[cfg(not(feature = "http"))]
        /// # fn main() {}
        /// ```
        pub struct $borrowed<'a>(CorsListView<'a>);

        impl fmt::Debug for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($owned))
                    .field("value_count", &self.0.value_count())
                    .field("method_count", &self.len())
                    .finish()
            }
        }

        impl fmt::Display for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                crate::headers::shared::fmt_ascii_values(self.field_values(), f)
            }
        }

        impl fmt::Debug for $borrowed<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($borrowed))
                    .field("value_count", &self.0.values.len())
                    .field("method_count", &self.len())
                    .finish()
            }
        }

        impl $owned {
            /// Constructs one canonical field line from validated methods.
            ///
            /// Empty input is valid for this response header. Duplicate methods
            /// are retained in input order.
            ///
            /// # Errors
            ///
            /// Returns an error if an item is not an HTTP method token.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::from_methods(["GET", "POST"])?;
            /// assert_eq!(
            ///     value
            ///         .iter()
            ///         .map(|method| method.as_str())
            ///         .collect::<Vec<_>>(),
            ///     ["GET", "POST"]
            /// );
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            // LLVM emits an uncallable polymorphized instance for this adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            pub fn from_methods<I, S>(methods: I) -> Result<Self, DecodeError>
            where
                I: IntoIterator<Item = S>,
                S: AsRef<str>,
            {
                CorsList::from_items($name, methods, true).map(Self)
            }

            /// Constructs the wildcard field value.
            ///
            /// Whether the wildcard has wildcard semantics depends on the
            /// request and is deliberately not inferred here.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let wildcard = AccessControlAllowMethodsOwned::wildcard();
            /// assert!(wildcard.contains_wildcard());
            /// assert!(wildcard.is_wildcard());
            ///
            /// let explicit = AccessControlAllowMethodsOwned::from_methods(["GET"])?;
            /// assert!(!explicit.contains_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn wildcard() -> Self {
                Self(CorsList::wildcard())
            }

            /// Constructs a present field with an empty method list.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::empty();
            /// assert!(value.is_empty());
            /// assert_eq!(value.len(), 0);
            /// ```
            pub fn empty() -> Self {
                Self(CorsList::empty())
            }

            /// Validates and adopts complete field lines.
            ///
            /// # Errors
            ///
            /// Returns an error for no field lines or malformed list members.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::from_field_values(vec![
            ///     FieldValue::from_static("GET, POST"),
            ///     FieldValue::from_static("PATCH"),
            /// ])?;
            /// assert_eq!(value.len(), 3);
            /// assert_eq!(value.field_values().count(), 2);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn from_field_values(values: Vec<FieldValue>) -> Result<Self, DecodeError> {
                CorsList::from_field_values($name, values, true).map(Self)
            }

            /// Iterates methods in wire order without allocating.
            ///
            /// Duplicate methods are returned separately.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::from_methods(["GET", "POST"])?;
            /// let methods = value
            ///     .iter()
            ///     .map(|method| method.as_str())
            ///     .collect::<Vec<_>>();
            /// assert_eq!(methods, ["GET", "POST"]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn iter(&self) -> impl Iterator<Item = CorsMethodView<'_>> {
                self.0
                    .field_values()
                    .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
                    .map(validate::trim_ows)
                    .filter(|item| !item.is_empty())
                    .map(method_ref_validated)
            }

            /// Returns the number of list members, including duplicates.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::from_methods(["GET", "POST", "PATCH"])?;
            /// assert_eq!(value.len(), 3);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn len(&self) -> usize {
                self.iter().count()
            }

            /// Returns whether the list has no members.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let empty = AccessControlAllowMethodsOwned::empty();
            /// assert!(empty.is_empty());
            ///
            /// let value = AccessControlAllowMethodsOwned::from_methods(["GET"])?;
            /// assert!(!value.is_empty());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn is_empty(&self) -> bool {
                self.iter().next().is_none()
            }

            /// Returns whether any member is `*`.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let wildcard = AccessControlAllowMethodsOwned::wildcard();
            /// assert!(wildcard.contains_wildcard());
            ///
            /// let explicit = AccessControlAllowMethodsOwned::from_methods(["GET", "POST"])?;
            /// assert!(!explicit.contains_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn contains_wildcard(&self) -> bool {
                self.iter().any(|method| method.as_bytes() == b"*")
            }

            /// Returns whether `*` is the only list member.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let wildcard = AccessControlAllowMethodsOwned::wildcard();
            /// assert!(wildcard.is_wildcard());
            ///
            /// let mixed = AccessControlAllowMethodsOwned::from_methods(["*", "GET"])?;
            /// assert!(mixed.contains_wildcard());
            /// assert!(!mixed.is_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn is_wildcard(&self) -> bool {
                let mut methods = self.iter();
                methods.next().is_some_and(|method| method.as_bytes() == b"*") && methods.next().is_none()
            }

            /// Iterates original field lines in wire order.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::try_from(FieldValue::from_static("GET, POST"))?;
            /// let mut fields = value.field_values();
            /// assert_eq!(
            ///     fields.next().map(|field| field.as_bytes()),
            ///     Some(b"GET, POST".as_slice())
            /// );
            /// assert!(fields.next().is_none());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> {
                self.0.field_values().map(FieldValue::as_field_value_ref)
            }

            /// Returns the original field lines.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowMethodsOwned;
            ///
            /// let value = AccessControlAllowMethodsOwned::try_from(FieldValue::from_static("GET, POST"))?;
            /// let fields = value.into_field_values();
            /// assert_eq!(fields.len(), 1);
            /// assert_eq!(fields[0].as_bytes(), b"GET, POST");
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn into_field_values(self) -> Vec<FieldValue> {
                self.0.into_field_values()
            }
        }

        impl<'a> $borrowed<'a> {
            /// Iterates methods in wire order without allocating.
            ///
            /// Duplicate methods are returned separately.
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("GET, POST"),
            /// );
            /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// let methods = value
            ///     .iter()
            ///     .map(|method| method.as_str())
            ///     .collect::<Vec<_>>();
            /// assert_eq!(methods, ["GET", "POST"]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn iter(&self) -> impl Iterator<Item = CorsMethodView<'a>> + '_ {
                self.0
                    .values
                    .repeated()
                    .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
                    .map(validate::trim_ows)
                    .filter(|item| !item.is_empty())
                    .map(method_ref_validated)
            }

            /// Returns the number of list members, including duplicates.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("GET, POST"),
            /// );
            /// headers.append(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("PATCH"),
            /// );
            /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// assert_eq!(value.len(), 3);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn len(&self) -> usize {
                self.iter().count()
            }

            /// Returns whether the list has no members.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static(""),
            /// );
            /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// assert!(value.is_empty());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn is_empty(&self) -> bool {
                self.iter().next().is_none()
            }

            /// Returns whether any member is `*`.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("*, GET"),
            /// );
            /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// assert!(value.contains_wildcard());
            ///
            /// let mut explicit_headers = HeaderMap::new();
            /// explicit_headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("GET"),
            /// );
            /// let explicit = AccessControlAllowMethods::view(&explicit_headers)?.expect("present");
            /// assert!(!explicit.contains_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn contains_wildcard(&self) -> bool {
                self.iter().any(|method| method.as_bytes() == b"*")
            }

            /// Returns whether `*` is the only list member.
            #[must_use]
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("*"),
            /// );
            /// let wildcard = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// assert!(wildcard.is_wildcard());
            ///
            /// let mut mixed_headers = HeaderMap::new();
            /// mixed_headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("*, GET"),
            /// );
            /// let mixed = AccessControlAllowMethods::view(&mixed_headers)?.expect("present");
            /// assert!(mixed.contains_wildcard());
            /// assert!(!mixed.is_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn is_wildcard(&self) -> bool {
                let mut methods = self.iter();
                methods.next().is_some_and(|method| method.as_bytes() == b"*") && methods.next().is_none()
            }

            /// Iterates original field lines in wire order.
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowMethods;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("GET, POST"),
            /// );
            /// headers.append(
            ///     "access-control-allow-methods",
            ///     http::HeaderValue::from_static("PATCH"),
            /// );
            /// let value = AccessControlAllowMethods::view(&headers)?.expect("present");
            /// let fields = value
            ///     .field_values()
            ///     .map(|field| field.as_bytes())
            ///     .collect::<Vec<_>>();
            /// assert_eq!(fields, [b"GET, POST".as_slice(), b"PATCH".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
                self.0.values.repeated()
            }
        }

        impl Field for $descriptor {
            type View<'a> = $borrowed<'a>;
            type Owned = $owned;

            fn name() -> &'static FieldName {
                $name
            }

            fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                let Some(lines) = source.lines(Self::name()) else {
                    return Ok(None);
                };
                lines.validate_list_item_limit(b',', true)?;
                let mut repeated = lines.repeated();
                let first = repeated.next().expect("FieldLines always contains at least one field line");
                if repeated.next().is_none() {
                    validate_single_list($name, first, true)?;
                    return Ok(Some($borrowed(CorsListView { values: lines })));
                }
                validate_list($name, lines.repeated(), true)?;
                Ok(Some($borrowed(CorsListView { values: lines })))
            }

            fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                let Some(lines) = source.lines(Self::name()) else {
                    return Ok(None);
                };
                CorsList::decode($name, &lines, true).map($owned).map(Some)
            }

            fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
            where
                S: FieldSink + ?Sized,
            {
                sink.set_values(Self::name(), value.0.into_encoded())
            }
        }

        impl TryFrom<FieldValue> for $owned {
            type Error = DecodeError;

            fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
                CorsList::from_field_value($name, value, true).map(Self)
            }
        }

        impl TryFrom<Vec<FieldValue>> for $owned {
            type Error = DecodeError;

            fn try_from(values: Vec<FieldValue>) -> Result<Self, Self::Error> {
                Self::from_field_values(values)
            }
        }
    };
}

define_method_list!(
    AccessControlAllowMethods,
    AccessControlAllowMethodsOwned,
    AccessControlAllowMethodsView,
    &FieldName::AccessControlAllowMethods
);

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AccessControlAllowMethods, AccessControlAllowMethodsOwned};
    use crate::headers::cors::test_support::TestMap;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn constructors_and_accessors_cover_empty_wildcard_and_repeated_lists() {
        let methods = AccessControlAllowMethodsOwned::from_methods(["GET", "CUSTOM", "GET"]).expect("valid method list");
        assert_eq!(
            methods.iter().map(super::CorsMethodView::as_str).collect::<Vec<_>>(),
            ["GET", "CUSTOM", "GET"]
        );
        assert_eq!(methods.len(), 3);
        assert!(!methods.is_empty());
        assert!(!methods.contains_wildcard());
        assert!(!methods.is_wildcard());
        assert_eq!(methods.field_values().count(), 1);
        assert!(format!("{methods:?}").contains("method_count"));

        let wildcard = AccessControlAllowMethodsOwned::wildcard();
        assert!(wildcard.contains_wildcard());
        assert!(wildcard.is_wildcard());
        let mixed = AccessControlAllowMethodsOwned::from_methods(["*", "GET"]).expect("mixed wildcard list");
        assert!(mixed.contains_wildcard());
        assert!(!mixed.is_wildcard());
        assert!(AccessControlAllowMethodsOwned::empty().is_empty());

        let repeated =
            AccessControlAllowMethodsOwned::from_field_values(vec![FieldValue::from_static("GET, POST"), FieldValue::from_static("PATCH")])
                .expect("repeated list");
        assert_eq!(repeated.len(), 3);
        assert_eq!(repeated.field_values().count(), 2);
        assert_eq!(repeated.into_field_values().len(), 2);

        let one = AccessControlAllowMethodsOwned::try_from(FieldValue::from_static("GET")).expect("one field line");
        assert_eq!(one.len(), 1);
        let many = AccessControlAllowMethodsOwned::try_from(vec![FieldValue::from_static("GET"), FieldValue::from_static("POST")])
            .expect("many field lines");
        assert_eq!(many.len(), 2);
    }

    #[test]
    fn borrowed_owned_insert_and_error_paths_preserve_field_lines() {
        let source = TestMap::new(
            &FieldName::AccessControlAllowMethods,
            vec![FieldValue::from_static("GET, CUSTOM"), FieldValue::from_static("*, POST")],
        );
        let view = AccessControlAllowMethods::view(&source)
            .expect("valid borrowed list")
            .expect("present");
        assert_eq!(view.len(), 4);
        assert!(!view.is_empty());
        assert!(view.contains_wildcard());
        assert!(!view.is_wildcard());
        assert_eq!(view.field_values().count(), 2);
        assert!(format!("{view:?}").contains("method_count"));

        let wildcard_source = TestMap::new(&FieldName::AccessControlAllowMethods, vec![FieldValue::from_static("*")]);
        let wildcard_view = AccessControlAllowMethods::view(&wildcard_source)
            .expect("wildcard view")
            .expect("present");
        assert!(wildcard_view.is_wildcard());

        let owned = AccessControlAllowMethods::owned(&source)
            .expect("valid owned list")
            .expect("present");
        assert_eq!(owned.len(), 4);
        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlAllowMethods::insert(&mut sink, owned).expect("insert method list");
        assert_eq!(sink.name, &FieldName::AccessControlAllowMethods);
        assert_eq!(sink.values, source.values);

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlAllowMethods::view(&absent).expect("absent").is_none());
        assert!(AccessControlAllowMethods::owned(&absent).expect("absent").is_none());

        let no_lines = AccessControlAllowMethodsOwned::from_field_values(Vec::new()).expect_err("no field lines");
        assert_eq!(no_lines.kind(), DecodeErrorKind::MissingValue);
        let invalid = AccessControlAllowMethodsOwned::from_methods(["GET", "bad method"]).expect_err("invalid method");
        assert_eq!(invalid.kind(), DecodeErrorKind::InvalidToken);

        let bad_later = TestMap::new(
            &FieldName::AccessControlAllowMethods,
            vec![FieldValue::from_static("GET"), FieldValue::from_static("bad method")],
        );
        let error = AccessControlAllowMethods::view(&bad_later).expect_err("bad second field line");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(error.value_index(), Some(1));
        let error = AccessControlAllowMethods::owned(&bad_later).expect_err("bad second field line");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(error.value_index(), Some(1));
    }
}
