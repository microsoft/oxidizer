// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer-level coverage for generic events and enrichments.
//!
//! None of the structs below declare any bound of their own. The macros infer
//! the predicates each field's redaction path needs, so these compile only if
//! that inference is right - which snapshot tests cannot show, because they just
//! parse and pretty-print the generated tokens without ever type-checking them.

use data_privacy::DataClass;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Severity, emit, event};
use observed_testing::types::{PublicI64, PublicString};
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

const DC: DataClass = DataClass::new("test_taxonomy", "public");

/// `unredacted` path: needs `T: Clone` and `Value: From<T>`.
#[event("generic.unredacted")]
#[info]
struct GenericUnredacted<T> {
    #[unredacted]
    value: T,
}

/// Default path: needs `T: RedactedDisplay`.
#[event("generic.redacted")]
#[info]
struct GenericRedacted<T> {
    value: T,
}

/// `data_class` path: needs `T: Clone` and `Sensitive<T>: RedactedDisplay`.
#[event("generic.classified")]
#[info]
struct GenericClassified<T> {
    #[data_class(DC)]
    value: T,
}

/// An `Option<T>` field carries its bounds on the inner type.
#[event("generic.optional")]
#[info]
struct GenericOptional<T> {
    value: Option<T>,
}

/// Two parameters, each on a different redaction path.
#[event("generic.mixed")]
#[info]
struct GenericMixed<T, U> {
    #[unredacted]
    raw: T,
    classified: U,
}

/// Enrichment `unredacted` path: needs `T: Into<Value>`.
#[derive(Debug, Enrichment)]
struct GenericCtxUnredacted<T> {
    #[unredacted]
    ctx: T,
}

/// Enrichment default path: needs `T: RedactedDisplay + Send + Sync + 'static`.
#[derive(Debug, Enrichment)]
struct GenericCtxRedacted<T> {
    ctx: T,
}

/// Enrichment `data_class` path: needs
/// `Sensitive<T>: RedactedDisplay + Send + Sync + 'static`, which no other
/// generic enrichment case exercises.
#[derive(Debug, Enrichment)]
struct GenericCtxClassified<T> {
    #[data_class(DC)]
    ctx: T,
}

#[test]
fn generic_event_with_unredacted_field_emits() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, GenericUnredacted { value: 42_i64 });

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.unredacted", Severity::Info).dimension("value", 42_i64),
    );
}

#[test]
fn generic_event_with_redacted_field_emits() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        GenericRedacted {
            value: PublicString("hello".to_owned()),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.redacted", Severity::Info).dimension("value", "hello"),
    );
}

#[test]
fn generic_event_with_data_class_field_emits() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        GenericClassified {
            value: "secret".to_owned(),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.classified", Severity::Info).dimension("value", "secret"),
    );
}

#[test]
fn generic_event_with_option_field_emits_both_arms() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        GenericOptional {
            value: Some(PublicString("present".to_owned())),
        }
    );
    emit!(sink, GenericOptional::<PublicString> { value: None });

    let events = processor.events();
    assert_eq!(
        events[0],
        ExpectedEvent::new("generic.optional", Severity::Info).dimension("value", "present"),
    );
    // The default `#[if_none("n/a")]` placeholder fills the missing value.
    assert_eq!(
        events[1],
        ExpectedEvent::new("generic.optional", Severity::Info).dimension("value", "n/a"),
    );
}

#[test]
fn generic_event_with_two_parameters_emits() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        GenericMixed {
            raw: 7_i64,
            classified: PublicString("mixed".to_owned()),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.mixed", Severity::Info)
            .dimension("raw", 7_i64)
            .dimension("classified", "mixed"),
    );
}

#[test]
fn generic_enrichments_reach_the_record() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        (|| {
            emit!(sink, GenericUnredacted { value: 1_i64 });
        })
        .enrich(&sink, GenericCtxRedacted { ctx: PublicI64(99) })();
    })
    .enrich(&sink, GenericCtxUnredacted { ctx: 5_i64 })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.unredacted", Severity::Info)
            .dimension("ctx", 5_i64)
            .dimension("ctx", "99")
            .dimension("value", 1_i64),
    );
}

/// The `data_class` enrichment path wraps the value in `Sensitive<T>` before
/// redaction, generating a predicate on the wrapper rather than on `T`. Emitting
/// through it checks both the bound and the entry conversion.
#[test]
fn generic_data_class_enrichment_reaches_the_record() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        emit!(sink, GenericUnredacted { value: 1_i64 });
    })
    .enrich(&sink, GenericCtxClassified { ctx: "tenant-7" })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.unredacted", Severity::Info)
            .dimension("ctx", "tenant-7")
            .dimension("value", 1_i64),
    );
}

/// A metric-only event: `#[counter]` with no severity attribute, so no field
/// has a log key. `payload` therefore reaches neither signal.
#[event("generic.metric_only")]
#[counter]
struct GenericMetricOnly<T> {
    #[dimension(metric = "reason")]
    reason: PublicString,
    #[expect(dead_code, reason = "reaching no signal is the point - nothing reads it")]
    payload: T,
}

/// A log event whose only generic field is excluded from logs and opts into no
/// metric, so it too reaches neither signal.
#[event("generic.excluded")]
#[info]
struct GenericExcluded<T> {
    kept: PublicI64,
    #[dimension(log = exclude)]
    #[expect(dead_code, reason = "reaching no signal is the point - nothing reads it")]
    dropped: T,
}

/// A field routed to neither signal generates no visit code, so the impl must
/// not demand the bounds of a redaction path it never walks. Both structs below
/// carry a type parameter that implements none of them.
///
/// This is a compile-time assertion: the bodies only need the `Event` impl to
/// apply. `NotRedactable` deliberately implements no `observed` trait.
#[test]
fn fields_routed_to_no_signal_do_not_constrain_their_type() {
    #[derive(Clone)]
    struct NotRedactable;

    const fn assert_event<E: observed::Event>() {}

    assert_event::<GenericMetricOnly<NotRedactable>>();
    assert_event::<GenericExcluded<NotRedactable>>();
}

/// The unrouted field is genuinely absent from the record, which is what makes
/// dropping its bounds correct rather than merely convenient.
#[test]
fn a_field_routed_to_no_signal_is_absent_from_the_record() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        GenericExcluded {
            kept: PublicI64(1),
            dropped: PublicString("hidden".to_owned()),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("generic.excluded", Severity::Info).dimension("kept", "1"),
    );
}
