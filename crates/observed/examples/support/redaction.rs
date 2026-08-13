// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A passthrough redaction engine shared by the `observed` crate's examples.
//!
//! Examples exist to show which attributes reach which signal, so they need the
//! attribute *values* to be legible in their output. A default
//! [`data_privacy::RedactionEngine`] erases every classified value, which would
//! print `status = ""` for `status: 404` and make the routing impossible to
//! follow.
//!
//! A real deployment configures a redactor per data class instead - see the
//! `data_privacy` crate for the policy API. Never ship this engine.
//!
//! Included by individual examples via `#[path = "support/redaction.rs"]` so the
//! examples stay self-contained and do not depend on the internal
//! `observed_testing` test harness.

/// Builds a redaction engine that passes every classified value through
/// unchanged, keeping example output readable.
pub(crate) fn passthrough_redaction_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
            data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
        ))
        .build()
}
