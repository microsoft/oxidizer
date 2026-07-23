// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and section-size control for ordinary response-header insertion.

use http::header::{HeaderName, HeaderValue};

fn insert_headers(headers: &mut http::HeaderMap, scenario: Scenario) {
    for &(name, value) in &HEADER_FIELDS[..scenario.count()] {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
}

include!("common/response_head_size_control.rs");
