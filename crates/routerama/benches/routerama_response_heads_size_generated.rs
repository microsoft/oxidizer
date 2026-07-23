// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and section-size control for generated static response-header plans.

#![expect(
    dead_code,
    reason = "the shared ordinary-header table is intentionally unused by this generated literal plan"
)]

use http::header::{HeaderName, HeaderValue};

macro_rules! insert_generated {
    ($headers:expr, [$(($name:literal, $value:literal)),* $(,)?]) => {{
        $(
            _ = $headers.insert(
                const { HeaderName::from_static($name) },
                const { HeaderValue::from_static($value) },
            );
        )*
    }};
}

fn insert_headers(headers: &mut http::HeaderMap, scenario: Scenario) {
    match scenario {
        Scenario::Headers0 => {}
        Scenario::Headers1 => insert_generated!(headers, [("x-template-00", "value-00")]),
        Scenario::Headers4 => insert_generated!(
            headers,
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
            ]
        ),
        Scenario::Headers16 => insert_generated!(
            headers,
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
                ("x-template-04", "value-04"),
                ("x-template-05", "value-05"),
                ("x-template-06", "value-06"),
                ("x-template-07", "value-07"),
                ("x-template-08", "value-08"),
                ("x-template-09", "value-09"),
                ("x-template-10", "value-10"),
                ("x-template-11", "value-11"),
                ("x-template-12", "value-12"),
                ("x-template-13", "value-13"),
                ("x-template-14", "value-14"),
                ("x-template-15", "value-15"),
            ]
        ),
    }
}

include!("common/response_head_size_control.rs");
