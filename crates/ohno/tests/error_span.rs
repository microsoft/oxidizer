// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::drop_non_drop, reason = "this is test code")]

use std::sync::atomic::{AtomicI32, Ordering};

use ohno::{Error, ErrorExt, ErrorSpan, OhnoCore, SpanInfo, error_span};

#[macro_use]
mod util;

#[derive(Error)]
struct BasicTestError {
    inner: OhnoCore,
}

#[test]
fn case_1_regular_string() {
    #[error_span("operation failed")]
    fn regular_string_test() -> Result<String, BasicTestError> {
        Err(BasicTestError::caused_by("base error"))
    }

    let error = regular_string_test().unwrap_err();
    assert_eq!(error.message(), "base error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"base error"));
    assert_span!(error, "operation failed");
}

#[test]
fn case_2_inline_argument() {
    #[error_span("failed to process file {filename}")]
    fn inline_argument_test(filename: &str) -> Result<String, BasicTestError> {
        Err(BasicTestError::caused_by("file error"))
    }

    let filename = "test.txt";
    let error = inline_argument_test(filename).unwrap_err();
    assert_eq!(error.message(), "file error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"file error"));
    assert_span!(error, "failed to process file test.txt");
}

#[test]
fn case_3_positional_argument() {
    #[error_span("processed {} bytes", data.len())]
    fn positional_argument_test(data: &[u8]) -> Result<String, BasicTestError> {
        Err(BasicTestError::caused_by("processing error"))
    }

    let data = vec![1u8, 2u8, 3u8, 4u8, 5u8];
    let error = positional_argument_test(&data).unwrap_err();
    assert_eq!(error.message(), "processing error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"processing error"));
    assert_span!(error, "processed 5 bytes");
}

#[test]
fn multiple_inline_arguments() {
    #[error_span("multiple {param1} inline {param2} arguments")]
    fn multiple_inline_test(param1: &str, param2: i32) -> Result<(), BasicTestError> {
        Err(BasicTestError::caused_by("multiple param error"))
    }

    let error = multiple_inline_test("value1", 42).unwrap_err();
    assert_eq!(error.message(), "multiple param error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"multiple param error"));
    assert_span!(error, "multiple value1 inline 42 arguments");
}

#[test]
fn multiple_positional_arguments() {
    #[error_span("multiple {} positional {} arguments", first, second)]
    fn multiple_positional_test(first: &str, second: i32) -> Result<(), BasicTestError> {
        Err(BasicTestError::caused_by("multiple pos error"))
    }

    let error = multiple_positional_test("first", 100).unwrap_err();
    assert_eq!(error.message(), "multiple pos error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"multiple pos error"));
    assert_span!(error, "multiple first positional 100 arguments");
}

#[test]
fn mixed_inline_and_positional_arguments() {
    #[error_span("mixed {inline} and {} positional", positional)]
    fn mixed_arguments_test(inline: &str, positional: i32) -> Result<(), BasicTestError> {
        Err(BasicTestError::caused_by("mixed error"))
    }

    let error = mixed_arguments_test("inline_val", 200).unwrap_err();
    assert_eq!(error.message(), "mixed error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"mixed error"));
    assert_span!(error, "mixed inline_val and 200 positional");
}

#[test]
fn generic_function_with_where() {
    #[error_span("where t: {t}")]
    fn where_test<T>(t: T) -> Result<(), BasicTestError>
    where
        T: std::fmt::Display,
    {
        let _ = t.to_string();
        Err(BasicTestError::caused_by("where error"))
    }

    let error = where_test("Hi").unwrap_err();
    assert_eq!(error.message(), "where error");

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"where error"));
    assert_span!(error, "where t: Hi");
}

struct SyncService {
    counter: i32,
    atomic_counter: AtomicI32,
}

impl SyncService {
    const fn new() -> Self {
        Self {
            counter: 0,
            atomic_counter: AtomicI32::new(0),
        }
    }

    #[error_span("sync service method failed with value {value}")]
    fn do_something(&mut self, value: i32) -> Result<i32, BasicTestError> {
        self.counter += value;
        self.atomic_counter.fetch_add(value, Ordering::SeqCst);
        Err(BasicTestError::caused_by("negative value"))
    }

    #[error_span("sync read-only method failed")]
    fn read_only(&self) -> Result<i32, BasicTestError> {
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        Err(BasicTestError::caused_by("counter is zero"))
    }

    #[error_span("sync method with self field access failed, counter: {}", self.counter)]
    fn with_self_field(&self) -> Result<i32, BasicTestError> {
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        Err(BasicTestError::caused_by("failed with field"))
    }

    #[error_span("mutable method failed, atomic: {}", self.atomic_counter.load(Ordering::SeqCst))]
    fn with_mut_self_no_args(&mut self) -> Result<i32, BasicTestError> {
        self.counter += 1;
        self.atomic_counter.fetch_add(1, Ordering::SeqCst);
        Err(BasicTestError::caused_by("mutation failed"))
    }

    #[error_span("method failed")] // you can't use message as it's consumed in the function
    fn with_self_and_string(&self, message: String) -> Result<i32, BasicTestError> {
        let e = BasicTestError::caused_by(format!("message was: {message}"));
        drop(message); // ensure message is consumed
        Err(e)
    }

    #[error_span("method failed with string ref: {message}")]
    fn with_self_and_string_ref(&self, message: &String) -> Result<i32, BasicTestError> {
        Err(BasicTestError::caused_by(format!("message was: {message}")))
    }

    #[error_span("consuming method failed")]
    fn consume_self(self) -> Result<i32, BasicTestError> {
        let counter = self.counter;
        drop(self); // ensure self is consumed
        Err(BasicTestError::caused_by(format!("consumed with counter: {counter}")))
    }

    #[error_span("consuming method with arg failed, value: {value}")]
    fn consume_self_with_arg(self, value: i32) -> Result<i32, BasicTestError> {
        drop(self); // ensure self is consumed
        Err(BasicTestError::caused_by(format!("consumed with value: {value}")))
    }

    #[error_span("consuming mutable method failed")]
    fn consume_self_mut(mut self) -> Result<i32, BasicTestError> {
        self.counter += 1;
        let counter = self.counter;
        drop(self); // ensure self is consumed
        Err(BasicTestError::caused_by(format!("consumed mut with counter: {counter}")))
    }
}

#[test]
fn sync_method_with_mut_self() {
    let mut service = SyncService::new();
    let error = service.do_something(-5).unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"negative value"));
    assert_span!(error, "sync service method failed with value -5");
}

#[test]
fn sync_method_with_self() {
    let service = SyncService::new();
    let error = service.read_only().unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"counter is zero"));
    assert_span!(error, "sync read-only method failed");
}

#[test]
fn sync_method_with_self_field_access() {
    let service = SyncService::new();
    let error = service.with_self_field().unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"failed with field"));
    assert_span!(error, "sync method with self field access failed, counter: 0");
}

#[test]
fn sync_method_with_mut_self_no_args() {
    let mut service = SyncService::new();
    let error = service.with_mut_self_no_args().unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"mutation failed"));
    // The atomic counter is 1 after fetch_add, not 0
    assert_span!(error, "mutable method failed, atomic: 1");
}

#[test]
fn sync_method_with_self_and_string() {
    let service = SyncService::new();
    let message = String::from("test message");
    let error = service.with_self_and_string(message).unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"message was: test message"));
    assert_span!(error, "method failed");
}

#[test]
fn sync_method_with_self_and_string_ref() {
    let service = SyncService::new();
    let message = String::from("ref message");
    let error = service.with_self_and_string_ref(&message).unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"message was: ref message"));
    assert_span!(error, "method failed with string ref: ref message");
}

#[test]
fn sync_method_consume_self() {
    let service = SyncService::new();
    let error = service.consume_self().unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed with counter: 0"));
    assert_span!(error, "consuming method failed");
}

#[test]
fn sync_method_consume_self_with_arg() {
    let service = SyncService::new();
    let error = service.consume_self_with_arg(42).unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed with value: 42"));
    assert_span!(error, "consuming method with arg failed, value: 42");
}

#[test]
fn sync_method_consume_self_mut() {
    let service = SyncService::new();
    let error = service.consume_self_mut().unwrap_err();

    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"consumed mut with counter: 1"));
    assert_span!(error, "consuming mutable method failed");
}

#[test]
fn impl_as_ref() {
    #[error_span("operation failed. Path: {}", path.as_ref().display())]
    fn impl_as_ref_test(path: impl AsRef<std::path::Path>) -> Result<String, BasicTestError> {
        Err(BasicTestError::caused_by("path error"))
    }

    let error = impl_as_ref_test("test/path/1.txt").unwrap_err();
    let error_display = format!("{error}");
    let lines = error_display.lines().collect::<Vec<_>>();
    assert_eq!(lines.first(), Some(&"path error"));
    assert_span!(error, "operation failed. Path: test/path/1.txt");
}

#[test]
fn empty_context_iter() {
    let core = OhnoCore::default();
    assert!(core.context_iter().next().is_none());
}

#[test]
fn context_iter_reverse_order() {
    let mut core = OhnoCore::default();

    let spans = ["span 1", "span 2", "span 3", "span 4", "span 5"];
    for (i, &msg) in spans.iter().enumerate() {
        #[expect(clippy::cast_possible_truncation, reason = "Test")]
        let span = SpanInfo::detailed(msg, "test.rs", (i + 1) as u32 * 10);
        core.add_error_span(span);
    }

    let messages: Vec<(&str, &str, u32)> = core
        .context_iter()
        .map(|span| {
            let location = span.location.as_ref().unwrap();
            (span.message.as_ref(), location.file, location.line)
        })
        .collect();

    assert_eq!(
        messages,
        vec![
            ("span 5", "test.rs", 50),
            ("span 4", "test.rs", 40),
            ("span 3", "test.rs", 30),
            ("span 2", "test.rs", 20),
            ("span 1", "test.rs", 10),
        ]
    );
}
