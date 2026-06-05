// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mocked replies from "Crates.io" api for a specific crate / crate query
use http::StatusCode;
use http_extensions::{FakeHandler, HttpError, HttpResponse, HttpResponseBuilder};
use serde_json::{Value, json};

fn json_response(value: &Value) -> Result<HttpResponse, HttpError> {
    HttpResponseBuilder::new_fake().status(StatusCode::OK).json(value).build()
}

pub(super) fn crates_io_fake_handler(crate_name: String) -> FakeHandler {
    FakeHandler::from_sync_handler(move |request| {
        let path = request.uri().path();
        let query = request.uri().query();
        if path == format!("/api/v1/crates/{crate_name}") {
            return json_response(&json!({
                "crate": {
                    "name": "serde",
                    "downloads": 1337,
                    "description": "This is a mocked serde crate crates.io output"
                }
            }
            ));
        }
        if path == "/api/v1/crates" && query == Some(&format!("q={crate_name}")) {
            return json_response(&json!({
                "crates": [
                    {
                        "name": "serde",
                        "downloads": 1337,
                        "description": "This is a mocked serde crate crates.io output"
                    },
                    {
                        "name": "serde_json",
                        "downloads": 42,
                        "description": "This is a mocked serde json crate crates.io output"
                    }
                ]
            }));
        }
        HttpResponseBuilder::new_fake()
            .status(StatusCode::NOT_FOUND)
            .text("Resource not found")
            .build()
    })
}
