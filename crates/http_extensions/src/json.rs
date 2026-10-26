// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lazy JSON parsing utilities for HTTP responses.
//!
//! [`Json`] stores response data without parsing it.
//! It parses the data when you request a value.
//! It supports borrowed and owned deserialization.

use std::marker::PhantomData;

use bytes::{Buf, Bytes};
use bytesbuf::BytesView;
use ohno::{ErrorLabel, Labeled};
use recoverable::{Recovery, RecoveryInfo};
use serde_core::Deserialize;
use serde_core::de::DeserializeOwned;

use crate::HttpError;
use crate::error_labels::{LABEL_JSON, LABEL_JSON_DESERIALIZATION, LABEL_JSON_SERIALIZATION};

/// An error from JSON serialization or deserialization.
///
/// Parsing invalid JSON and failed serialization return this error.
#[derive(ohno::Error)]
#[no_constructors]
#[display("{message}")]
pub struct JsonError {
    label: ErrorLabel,
    message: &'static str,
    inner: ohno::OhnoCore,
}

impl JsonError {
    #[must_use]
    pub(crate) fn serialization(error: serde_json::Error) -> Self {
        Self {
            label: LABEL_JSON_SERIALIZATION,
            message: "JSON serialization error",
            inner: ohno::OhnoCore::from(error),
        }
    }

    #[must_use]
    pub(crate) fn deserialization(error: serde_json::Error) -> Self {
        Self {
            label: LABEL_JSON_DESERIALIZATION,
            message: "JSON deserialization error",
            inner: ohno::OhnoCore::from(error),
        }
    }
}

impl Labeled for JsonError {
    fn label(&self) -> &ErrorLabel {
        &self.label
    }
}

impl From<JsonError> for HttpError {
    fn from(value: JsonError) -> Self {
        Self::other(value, RecoveryInfo::never(), LABEL_JSON)
    }
}

impl Recovery for JsonError {
    fn recovery(&self) -> RecoveryInfo {
        RecoveryInfo::never()
    }
}

/// A JSON parser that delays deserialization until a read method is called.
///
/// `Json<T>` holds JSON data in its raw form and only parses it when you call
/// [`read`](Self::read) or [`read_owned`](Self::read_owned).
///
/// The type supports two modes:
///
/// - [`read`](Self::read) can borrow from the buffer.
///   You can call it more than once.
///   A returned lifetime is tied to the parser lifetime.
/// - [`read_owned`](Self::read_owned) consumes the parser and returns owned data.
///
/// See the [`HttpBody::into_json_ref`](crate::HttpBody::into_json_ref) and
/// [`HttpBody::into_json`](crate::HttpBody::into_json) methods for more examples.
#[derive(Debug)]
pub struct Json<T> {
    state: JsonState,
    _type: PhantomData<T>,
}

impl<T> Json<T> {
    pub(crate) fn new(bytes: BytesView) -> Self {
        Self {
            state: JsonState::BytesView(bytes),
            _type: PhantomData,
        }
    }
}

impl<'a, T: Deserialize<'a>> Json<T> {
    /// Parses the JSON data using lifetime-aware deserialization.
    ///
    /// This method deserializes borrowed and owned data.
    /// Strings and byte arrays can borrow directly from the JSON buffer.
    /// Numbers, Boolean values, and owned containers create new values.
    ///
    /// The returned value is tied to the lifetime of this parser, but that does not
    /// require all fields to borrow data.
    /// A value can contain borrowed and owned fields.
    ///
    /// # Examples
    ///
    /// ```
    /// # use serde::Deserialize;
    /// # use bytesbuf::BytesView;
    /// # use std::borrow::Cow;
    /// # use http_extensions::{HttpBodyBuilder, HttpError, Json};
    ///
    /// #[derive(Deserialize)]
    /// struct Person<'a> {
    ///     #[serde(borrow)] // Tell serde to borrow this field.
    ///     name: Cow<'a, str>,
    ///     age: u32,
    /// }
    ///
    /// fn handle_json<'a>(json: &'a mut Json<Person<'a>>) -> Result<(), HttpError> {
    ///     let person: Person = json.read()?;
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake().text(r#"{"name":"Alice","age":30}"#);
    /// #     let mut json = body.into_json_ref::<Person>().await.unwrap();
    /// #     handle_json(&mut json).unwrap();
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON deserialization fails.
    pub fn read(&'a mut self) -> Result<T, JsonError> {
        // Convert a sequence to bytes if it is not already converted.
        if let JsonState::BytesView(bytes) = &mut self.state {
            let bytes = std::mem::take(bytes).to_bytes();
            self.state = JsonState::Bytes(bytes);
        }

        // The parser can now borrow from the bytes.
        serde_json::from_slice(self.state.as_bytes()).map_err(JsonError::deserialization)
    }
}

impl<T: DeserializeOwned> Json<T> {
    /// Parses the JSON data into owned values and consumes the parser.
    ///
    /// This method creates owned strings and collections.
    /// The returned value does not depend on the parser lifetime.
    /// Use it to store parsed data after the parser goes out of scope.
    ///
    /// This method consumes the parser, so you can only call it once.
    ///
    /// # Examples
    ///
    /// ```
    /// # use serde::Deserialize;
    /// # use bytesbuf::BytesView;
    /// # use http_extensions::{Json, HttpBodyBuilder, HttpError};
    ///
    /// #[derive(Deserialize)]
    /// struct Person {
    ///     name: String, // Owned string
    ///     age: u32,
    /// }
    ///
    /// fn handle_json(json: Json<Person>) -> Result<(), HttpError> {
    ///     let person: Person = json.read_owned()?; // Consumes the parser
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake().text(r#"{"name":"Alice","age":30}"#);
    /// #     handle_json(body.into_json_ref().await.unwrap()).unwrap();
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON deserialization fails.
    pub fn read_owned(self) -> Result<T, JsonError> {
        match self.state {
            JsonState::BytesView(bytes) => serde_json::from_reader(bytes).map_err(JsonError::deserialization),
            JsonState::Bytes(bytes) => serde_json::from_reader(bytes.reader()).map_err(JsonError::deserialization),
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "BytesView is intentionally large, though future optimizations may decrease size"
)]
#[derive(Debug)]
enum JsonState {
    BytesView(BytesView),
    Bytes(Bytes),
}

impl JsonState {
    /// Returns a reference to the underlying bytes.
    ///
    /// # Panics
    ///
    /// Panics if called when the state is still `BytesView`, which indicates
    /// a programming error since `read()` always converts to `Bytes` first.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Bytes(bytes) => bytes,
            Self::BytesView(_) => unreachable!("guarded by the BytesView-to-Bytes conversion in read()"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::borrow::Cow;

    use bytes::Bytes;
    use ohno::{ErrorExt, Labeled};
    use recoverable::{Recovery, RecoveryInfo};
    use serde::Deserialize;
    use serde_json::json;

    use crate::{Json, JsonError};

    #[derive(Debug, Deserialize)]
    struct Person<'a> {
        #[serde(borrow)]
        name: Cow<'a, str>,
        #[serde(borrow)]
        surname: Cow<'a, str>,
        #[serde(borrow)]
        extra: Option<&'a str>,
    }

    #[derive(Deserialize)]
    struct OwnedPerson {
        name: String,
        surname: String,
    }

    #[test]
    pub(crate) fn assert_send() {
        static_assertions::assert_impl_all!(Json<String> : Send);
    }

    #[test]
    pub(crate) fn smoke_test() {
        let json = json!({
            "name": "John",
            "surname": "Doe"
        });

        // Convert JSON to a BytesView (the actual constructor parameter)
        let json_bytes = Bytes::from(json.to_string());
        let json_bytes = bytesbuf::BytesView::from(json_bytes);
        let mut json_parser = Json::<Person>::new(json_bytes);

        let person = json_parser.read().unwrap();
        assert_eq!(person.name, "John");
        assert_eq!(person.surname, "Doe");
    }

    #[test]
    pub(crate) fn test_read_owned() {
        let json = json!({
            "name": "Jane",
            "surname": "Smith"
        });

        let json_bytes = Bytes::from(json.to_string());
        let json_bytes = bytesbuf::BytesView::from(json_bytes);
        let json_parser = Json::<OwnedPerson>::new(json_bytes);

        let person = json_parser.read_owned().unwrap();
        assert_eq!(person.name, "Jane");
        assert_eq!(person.surname, "Smith");
    }

    #[test]
    pub(crate) fn test_read_then_read_owned() {
        let json = json!({
            "name": "Jane",
            "surname": "Smith"
        });

        let json_bytes = bytesbuf::BytesView::from(Bytes::from(json.to_string()));
        let mut json_parser = Json::<OwnedPerson>::new(json_bytes);

        let person = json_parser.read().unwrap();
        assert_eq!(person.name, "Jane");
        assert_eq!(person.surname, "Smith");

        let person = json_parser.read_owned().unwrap();
        assert_eq!(person.name, "Jane");
        assert_eq!(person.surname, "Smith");
    }

    #[test]
    pub(crate) fn test_escaped_json() {
        let json = json!({
            "name": "Jane",
            "surname": "\"Smith\"",
            "extra": "val"
        });

        let json_bytes = bytesbuf::BytesView::from(Bytes::from(json.to_string()));
        let mut json_parser = Json::<Person>::new(json_bytes);

        let person = json_parser.read().unwrap();

        assert!(matches!(person.name, Cow::Borrowed("Jane")));
        assert!(matches!(person.surname, Cow::Owned(_)));
        assert_eq!(person.surname.to_string(), "\"Smith\"");
        assert_eq!(person.extra, Some("val"));
    }

    #[test]
    pub(crate) fn test_escaped_fails() {
        let json = json!({
            "name": "Jane",
            "surname": "\"Smith\"",
            "extra": "\"Extra\""
        });

        let json_bytes = bytesbuf::BytesView::from(Bytes::from(json.to_string()));
        let mut json_parser = Json::<Person>::new(json_bytes);

        let _error = json_parser.read().unwrap_err();
    }

    #[test]
    fn json_error_deserialization() {
        let error = JsonError::deserialization(serde_json::Error::io(std::io::Error::other("json de error")));
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(error.label(), "json_deserialization");
        assert_eq!(error.message(), "JSON deserialization error\ncaused by: json de error");
    }

    #[test]
    fn json_error_serialization() {
        let error = JsonError::serialization(serde_json::Error::io(std::io::Error::other("json se error")));
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(error.label(), "json_serialization");
        assert_eq!(error.message(), "JSON serialization error\ncaused by: json se error");
    }

    #[test]
    fn http_error_from_json_deserialization() {
        use ohno::assert_error_message;

        let json_error = JsonError::deserialization(serde_json::Error::io(std::io::Error::other("json de error")));
        let http_error: crate::HttpError = json_error.into();
        assert_eq!(http_error.recovery(), RecoveryInfo::never());
        assert_eq!(http_error.label(), "json");
        assert_error_message!(http_error, "JSON deserialization error\ncaused by: json de error");
    }

    #[test]
    fn http_error_from_json_serialization() {
        use ohno::assert_error_message;

        let json_error = JsonError::serialization(serde_json::Error::io(std::io::Error::other("json se error")));
        let http_error: crate::HttpError = json_error.into();
        assert_eq!(http_error.recovery(), RecoveryInfo::never());
        assert_eq!(http_error.label(), "json");
        assert_error_message!(http_error, "JSON serialization error\ncaused by: json se error");
    }
}
