// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deserialize a typed object graph whose root, strings, and slices are owned
//! by the arena.

#![allow(clippy::missing_panics_doc, reason = "example code")]

use multitude::de::DeserializeIn;
use multitude::{Arc, Arena, Box};

#[derive(DeserializeIn)]
struct Metadata {
    active: bool,
    note: Option<Box<str>>,
}

#[derive(DeserializeIn)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u64,
    #[serde(alias = "display_name")]
    name: Box<str>,
    tags: Box<[Box<str>]>,
    metadata: Metadata,
    #[multitude(via_serde)]
    trace_id: String,
}

fn main() -> serde_json::Result<()> {
    let request: Arc<Request> = {
        let arena = Arena::new();
        arena.deserialize_json(
            r#"{
                "id": 42,
                "display_name": "Ada",
                "tags": ["admin", "on-call"],
                "metadata": {"active": true, "note": "primary"},
                "trace_id": "abc-123"
            }"#,
        )?
    };

    // The Arc and every nested arena smart pointer keep their chunks alive,
    // so the complete graph remains valid after the Arena handle is dropped.
    assert_eq!(request.id, 42);
    assert_eq!(request.name.as_str(), "Ada");
    assert_eq!(request.tags[1].as_str(), "on-call");
    assert!(request.metadata.active);
    assert_eq!(request.metadata.note.as_deref(), Some("primary"));
    assert_eq!(request.trace_id, "abc-123");
    Ok(())
}
