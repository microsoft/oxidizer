// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and section-size control for existing contiguous formatting.

#![allow(dead_code, reason = "the shared size control declares constants used by the other representations")]

use routerama::response::Body;

#[derive(serde::Serialize)]
struct EscapedPayload<'a> {
    message: &'a str,
}

fn render(scenario: Scenario) -> Body {
    match scenario {
        Scenario::FullyStatic => Body::from(FULLY_STATIC),
        Scenario::NumericJson => Body::from(format!(r#"{{"id":{NUMERIC_ID},"active":true}}"#)),
        Scenario::EscapedJson => Body::from(
            serde_json::to_string(&EscapedPayload { message: ESCAPED_MESSAGE }).expect("serializing the size-control payload succeeds"),
        ),
        Scenario::MediumTextShell => Body::from(format!("{MEDIUM_PREFIX}{MEDIUM_NAME}{MEDIUM_SUFFIX}")),
    }
}

include!("common/response_template_size_control.rs");
