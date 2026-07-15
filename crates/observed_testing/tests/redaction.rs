// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for privacy redaction: per-processor redaction engines, classified types,
//! enrichment redaction, and different redaction modes.
//!
//! Covers DESIGN.md requirements:
//! - Redaction-by-construction: non-primitive values pass through a `RedactionEngine`
//! - Per-processor redaction: each processor owns its own `RedactionEngine`
//! - Zero-cost redaction for public data (primitives skip redaction entirely)
//! - Sensitive enrichment values are redacted at emission time

use std::borrow::Cow;
use std::sync::Arc;

use data_privacy::RedactionEngine;
use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Event, Severity, Sink, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::{PiiString, PublicString, SecretString, TestTaxonomy};
use observed_testing::{ExpectedEvent, MockProcessor};

#[derive(Debug, Event)]
#[event(name = "user.action")]
#[log(severity = info)]
struct UserAction {
    user: PiiString,
    #[unredacted]
    action_code: i64,
}

#[derive(Debug, Event)]
#[event(name = "auth.token_used")]
#[log(severity = info)]
struct TokenUsed {
    token: SecretString,
    user: PiiString,
    #[unredacted]
    request_id: i64,
}

// ---------------------------------------------------------------------------
// Helper: build an engine with specific redaction rules
// ---------------------------------------------------------------------------

fn engine_erase_all() -> RedactionEngine {
    RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
        .build()
}

fn engine_replace_stars() -> RedactionEngine {
    RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build()
}

fn engine_passthrough() -> RedactionEngine {
    RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
        .build()
}

fn engine_per_class() -> RedactionEngine {
    RedactionEngine::builder()
        // PII: replace with stars
        .add_class_redactor(TestTaxonomy::Pii, SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        // Secret: erase completely
        .add_class_redactor(TestTaxonomy::Secret, SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
        // Public: passthrough
        .add_class_redactor(TestTaxonomy::PublicData, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
        // Fallback for anything else
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
        .build()
}

// ---------------------------------------------------------------------------
// Tests - Event field redaction
// ---------------------------------------------------------------------------

#[test]
fn redaction_modes_on_classified_fields() {
    let passthrough_proc = MockProcessor::with_redaction_engine(engine_passthrough());
    let erase_proc = MockProcessor::with_redaction_engine(engine_erase_all());
    let replace_proc = MockProcessor::with_redaction_engine(engine_replace_stars());

    let sink = Sink::new(
        "test",
        vec![
            Arc::new(passthrough_proc.clone()),
            Arc::new(erase_proc.clone()),
            Arc::new(replace_proc.clone()),
        ],
        tick::SimpleClock::new_frozen(),
    );

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 42,
        }
    );

    // Passthrough: classified string appears as-is
    assert_eq!(
        passthrough_proc.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .dimension("action_code", 42i64)
            .dimension("user", "Alice")
            .log(),
    );

    // Erase: classified string becomes empty
    assert_eq!(
        erase_proc.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .dimension("action_code", 42i64)
            .dimension("user", "")
            .log(),
    );

    // Replace('*'): each character replaced
    assert_eq!(
        replace_proc.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .dimension("action_code", 42i64)
            .dimension("user", "*****")
            .log(),
    );
}

#[test]
fn per_class_redaction_applies_different_rules() {
    let processor = MockProcessor::with_redaction_engine(engine_per_class());
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(
        sink,
        TokenUsed {
            token: SecretString("sk-abc123".into()),
            user: PiiString("Bob".into()),
            request_id: 99,
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("auth.token_used", Severity::Info)
            .dimension("request_id", 99i64)
            .dimension("token", "")
            .dimension("user", "***")
            .log(),
    );
}

#[test]
fn public_classified_type_with_passthrough_per_class() {
    #[derive(Debug, Event)]
    #[event(name = "service.started")]
    #[log(severity = info)]
    struct ServiceStarted {
        service: PublicString,
        #[unredacted]
        port: i64,
    }

    let processor = MockProcessor::with_redaction_engine(engine_per_class());
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(
        sink,
        ServiceStarted {
            service: PublicString("my-service".into()),
            port: 8080,
        }
    );

    // Public class with passthrough rule -> value preserved
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("service.started", Severity::Info)
            .dimension("port", 8080i64)
            .dimension("service", "my-service")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Tests - Enrichment redaction
// ---------------------------------------------------------------------------

#[test]
fn enrichment_string_values_are_redacted() {
    #[derive(Debug, Enrichment)]
    struct Ctx {
        tenant: PiiString,
    }

    let processor = MockProcessor::with_redaction_engine(engine_replace_stars());
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(
        &sink,
        Ctx {
            tenant: PiiString("contoso".into()),
        },
    )();

    // Enrichment string value goes through redaction, classified event field also goes through engine
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("tenant", "*******")
            .dimension("value", "*")
            .log(),
    );
}

#[test]
fn enrichment_sensitive_values_are_redacted() {
    #[derive(Debug, Enrichment)]
    struct Ctx {
        retry_count: PiiString,
    }

    let processor = MockProcessor::with_redaction_engine(engine_erase_all());
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(
        &sink,
        Ctx {
            retry_count: PiiString("42".into()),
        },
    )();

    // Sensitive enrichment values go through RedactedDisplay -> redacted by the engine
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("retry_count", "")
            .dimension("value", "")
            .log(),
    );
}

#[test]
fn enrichment_redaction_uses_per_processor_engine() {
    #[derive(Debug, Clone, Enrichment)]
    struct Ctx {
        user_email: PiiString,
    }

    let pass_proc = MockProcessor::with_redaction_engine(engine_passthrough());
    let erase_proc = MockProcessor::with_redaction_engine(engine_erase_all());

    let sink = Sink::new(
        "test",
        vec![Arc::new(pass_proc.clone()), Arc::new(erase_proc.clone())],
        tick::SimpleClock::new_frozen(),
    );

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(
        &sink,
        Ctx {
            user_email: PiiString("alice@example.com".into()),
        },
    )();

    // Passthrough processor preserves the enrichment value
    assert_eq!(
        pass_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("user_email", "alice@example.com")
            .dimension("value", "1")
            .log(),
    );

    // Erase processor removes the enrichment value
    assert_eq!(
        erase_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("user_email", "")
            .dimension("value", "")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Tests - Insert mode and tagged modes
// ---------------------------------------------------------------------------

#[test]
fn insert_mode_replaces_with_custom_string() {
    let engine = RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Insert(Cow::Borrowed("[REDACTED]"))))
        .build();

    let processor = MockProcessor::with_redaction_engine(engine);
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .dimension("action_code", 1i64)
            .dimension("user", "[REDACTED]")
            .log(),
    );
}

#[test]
fn passthrough_for_specific_class_erase_rest() {
    let engine = RedactionEngine::builder()
        // PII: passthrough (explicitly not redacted)
        .add_class_redactor(TestTaxonomy::Pii, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
        // Everything else erased
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
        .build();

    let processor = MockProcessor::with_redaction_engine(engine);
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(
        sink,
        TokenUsed {
            token: SecretString("secret-token".into()),
            user: PiiString("Alice".into()),
            request_id: 1,
        }
    );

    // PII with passthrough class redactor -> value preserved, Secret -> erase (fallback)
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("auth.token_used", Severity::Info)
            .dimension("request_id", 1i64)
            .dimension("token", "")
            .dimension("user", "Alice")
            .log(),
    );
}
