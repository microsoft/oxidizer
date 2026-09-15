// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Downstream checks for response method success and storage-error propagation.

use std::time::Duration;

use http_headers::headers::{CacheControl, SetCookieOwned, UserAgentOwned};
use http_headers::sink::{EncodedValues, FieldSink, FieldSinkExt, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{FieldName, FieldValue};

#[derive(Debug, Default)]
struct ToggleSink {
    reject: bool,
    name: Option<&'static FieldName>,
    values: Vec<FieldValue>,
}

impl FieldSource for ToggleSink {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        if self.name == Some(name) {
            FieldLines::from_slice(name, &self.values)
        } else {
            None
        }
    }
}

impl FieldSink for ToggleSink {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if self.reject {
            Err(InsertError)
        } else {
            self.name = Some(name);
            self.values = values.into_iter().collect();
            Ok(())
        }
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if self.reject {
            return Err(InsertError);
        }
        if self.name == Some(name) {
            self.values.extend(values);
        } else {
            self.name = Some(name);
            self.values = values.into_iter().collect();
        }
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        if self.name == Some(name) {
            self.name = None;
            self.values.clear();
        }
    }
}

#[test]
fn response_methods_cover_success_and_failure_in_the_same_downstream_sink() {
    let mut sink = ToggleSink::default();

    sink.set_user_agent(UserAgentOwned::try_from_static("client/1").expect("valid"))
        .expect("enabled storage succeeds");
    assert_eq!(sink.values[0], "client/1");
    sink.reject = true;
    assert_eq!(
        sink.set_user_agent(UserAgentOwned::try_from_static("client/2").expect("valid"))
            .err(),
        Some(InsertError)
    );
    assert_eq!(sink.values[0], "client/1");

    sink.reject = false;
    sink.set_content_length(42).expect("enabled storage succeeds");
    assert_eq!(sink.values[0], "42");
    sink.reject = true;
    assert_eq!(sink.set_content_length(42).err(), Some(InsertError));
    assert_eq!(sink.values[0], "42");

    sink.reject = false;
    sink.set_cache_control(CacheControl::private()).expect("enabled storage succeeds");
    assert_eq!(sink.values[0], "private");
    sink.reject = true;
    assert_eq!(sink.set_cache_control(CacheControl::no_cache()).err(), Some(InsertError));
    assert_eq!(sink.values[0], "private");

    sink.reject = false;
    sink.set_access_control_max_age(600).expect("enabled storage succeeds");
    assert_eq!(sink.values[0], "600");
    sink.reject = true;
    assert_eq!(sink.set_access_control_max_age(300).err(), Some(InsertError));
    assert_eq!(
        sink.set_access_control_max_age_duration(Duration::from_mins(5)).err(),
        Some(InsertError)
    );
    assert_eq!(sink.values[0], "600");

    sink.reject = false;
    let mut first = SetCookieOwned::new();
    first.push_str("a=1").expect("valid cookie");
    sink.append_set_cookie(first).expect("enabled storage succeeds");
    assert_eq!(sink.values[0], "a=1");
    sink.reject = true;
    let mut second = SetCookieOwned::new();
    second.push_str("b=2").expect("valid cookie");
    assert_eq!(sink.append_set_cookie(second).err(), Some(InsertError));
    assert_eq!(sink.values.len(), 1);
    assert_eq!(sink.values[0], "a=1");
}
