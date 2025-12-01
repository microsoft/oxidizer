// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "test-util")]

use std::io;

use ohno::{EnrichableExt, OhnoCore, assert_error_message};

// Test helper error type for various tests
#[derive(Debug)]
pub struct TestError {
    message: String,
    inner: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl TestError {
    pub fn new(message: impl AsRef<str>) -> Self {
        Self {
            message: message.as_ref().to_string(),
            inner: None,
        }
    }

    #[must_use]
    pub fn with_inner<E: std::error::Error + Send + Sync + 'static>(self, inner: E) -> Self {
        Self {
            inner: Some(Box::new(inner)),
            ..self
        }
    }

    #[must_use]
    pub fn with_inner_message(self, message: impl AsRef<str>) -> Self {
        self.with_inner(Self::new(message))
    }

    #[must_use]
    pub fn into_io_error(self) -> std::io::Error {
        std::io::Error::other(self)
    }
}

impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

#[test]
fn test_detailed_enrich() {
    let error = OhnoCore::from("base error")
        .enrich("first message")
        .enrich("second message")
        .enrich("third message");

    let display = error.to_string();
    assert!(display.contains("base error"));
    assert!(display.contains("first message"));
    assert!(display.contains("second message"));
    assert!(display.contains("third message"));

    // Test enrichment iteration
    let enrichments: Vec<_> = error.enrichments().collect();
    assert_eq!(enrichments.len(), 3);

    // Most recent first
    assert_eq!(enrichments[0].message, "third message");
    assert_eq!(enrichments[1].message, "second message");
    assert_eq!(enrichments[2].message, "first message");
}

#[test]
fn test_with_enrich() {
    let error = OhnoCore::from("base").enrich_with(|| format!("computed: {}", 42));

    let error_string = error.to_string();
    assert!(error_string.contains("computed: 42"));
    assert_eq!(error.enrichments().count(), 1);
}

#[test]
fn test_source_enum_variants() {
    let error = OhnoCore::from("message error");
    assert!(error.source().is_none());

    // Test Source::Error variant
    let io_error = io::Error::new(io::ErrorKind::NotFound, "file.txt");
    let wrapped = OhnoCore::from(io_error);
    assert!(wrapped.source().is_some());
}

#[test]
fn test_backtrace_capture() {
    let error_with_bt = OhnoCore::from("test");
    let error_also_with_bt = OhnoCore::from(io::Error::other("test"));
    let error_without_bt = OhnoCore::without_backtrace(io::Error::other("test"));

    // Note: Backtrace capture depends on RUST_BACKTRACE environment variable
    // We can't test the actual presence but we can test the methods exist
    let _ = error_with_bt.has_backtrace();
    let _ = error_with_bt.backtrace();
    let _ = error_also_with_bt.has_backtrace();
    let _ = error_also_with_bt.backtrace();
    assert!(!error_without_bt.has_backtrace());
    assert_eq!(error_without_bt.backtrace().status(), std::backtrace::BacktraceStatus::Disabled);
}

#[test]
fn test_trace_messages_iterator() {
    let error = OhnoCore::from("base").enrich("first").enrich("second");

    let messages: Vec<_> = error.enrichment_messages().collect();
    assert_eq!(messages, vec!["second", "first"]);
}

#[test]
fn error_source_is_accessible() {
    let inner_with_source = TestError::new("outer").with_inner_message("inner");

    assert_eq!(inner_with_source.to_string(), "outer");

    let core = OhnoCore::from(inner_with_source);
    assert_error_message!(core, "outer");

    let source = core.source().unwrap();
    assert_error_message!(source, "outer");

    let source = source.source().unwrap();
    assert_error_message!(source, "inner");
}

#[test]
fn clone_ohno_core() {
    let original = OhnoCore::from("original error").enrich("first message").enrich("second message");
    let mut cloned = original.clone();
    assert_eq!(original.to_string(), cloned.to_string());

    cloned = cloned.enrich("additional message");
    assert_ne!(original.to_string(), cloned.to_string());
}

#[test]
fn clone_with_inner_error() {
    let inner = TestError::new("inner error");
    let original = OhnoCore::from(inner).enrich("enrichment message");
    let cloned = original.clone();

    let _ = original.source().unwrap().downcast_ref::<TestError>().unwrap();
    let _ = cloned.source().unwrap().downcast_ref::<TestError>().unwrap();

    let original_msg = original.to_string();
    let cloned_msg = cloned.to_string();
    assert_eq!(original_msg, cloned_msg);
}
