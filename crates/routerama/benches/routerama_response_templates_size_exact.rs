// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and section-size control for exactly sized contiguous templates.

#![allow(dead_code, reason = "the shared size control declares constants used by the other representations")]

use routerama::response::{Body, html_body_template, json_body_template};

fn render(scenario: Scenario) -> Body {
    match scenario {
        Scenario::FullyStatic => html_body_template!("status=ready\n", "service=routerama\n"),
        Scenario::NumericJson => {
            json_body_template!(
                id = number(NUMERIC_ID);
                r#"{"id":"#, id, r#","active":true}"#
            )
        }
        Scenario::EscapedJson => {
            json_body_template!(
                message = string(ESCAPED_MESSAGE);
                r#"{"message":"#, message, "}"
            )
        }
        Scenario::MediumTextShell => {
            html_body_template!(
                name = text(MEDIUM_NAME);
                "<html><head><title>Routerama</title></head><body>",
                "<nav>home | routes | diagnostics</nav><main><h1>Hello, ",
                name,
                "</h1><p>This medium shell contains fixed navigation, headings, and ",
                "descriptive text around one small dynamic insertion.</p></main>",
                "<footer>served by Routerama</footer></body></html>"
            )
        }
    }
}

include!("common/response_template_size_control.rs");
