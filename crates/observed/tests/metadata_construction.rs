// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public adapter construction and macro-owned field indexing.

use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind, LogDescription, MetricDescription};
use observed::{Event, Severity, event};

#[test]
fn dynamic_descriptions_enable_only_the_named_signals() {
    let plain = EventDescription::new("bridge");
    assert_eq!(
        (
            plain.name(),
            plain.type_id(),
            plain.is_log(),
            plain.contains_metrics(),
            plain.is_disabled(),
            plain.fields().len()
        ),
        ("bridge", None, false, false, false, 0),
    );
    let log = plain.with_log(LogDescription::new("bridge.log", Severity::Warn, None));
    assert_eq!(log.log().unwrap().name(), "bridge.log");
    assert!(!log.contains_metrics());
    let metric = plain.with_metric(MetricDescription::new("bridge.count", InstrumentKind::Counter, "", ""));
    assert_eq!(metric.metric().unwrap().instrument_name(), "bridge.count");
    assert!(!metric.is_log());
    let fields = plain.with_field_metrics();
    assert!(fields.contains_metrics());
    assert!(fields.metric().is_none());
    assert!(plain.disabled().is_disabled());
    assert_eq!(plain.disabled().name(), "bridge");
}

#[test]
fn dynamic_fields_keep_signal_routes_without_claiming_static_positions() {
    let log = FieldDescriptor::log_only("message");
    assert_eq!(
        (log.field_name(), log.log().unwrap().key(), log.field_index()),
        ("message", "message", None)
    );
    assert!(log.metric().is_none());
    let dimension = FieldDescriptor::dimension("region").with_log_key("cloud.region");
    assert_eq!(dimension.field_name(), "region");
    assert_eq!(dimension.log().unwrap().key(), "cloud.region");
    assert_eq!(dimension.metric().unwrap().key(), "region");
    assert!(dimension.metric().unwrap().instrument_description().is_none());
    assert_eq!(dimension.field_index(), None);
    let measurement = FieldDescriptor::instrument(
        "bytes",
        MetricDescription::new("bridge.bytes", InstrumentKind::Counter, "Transferred bytes", "By"),
    );
    assert_eq!(
        measurement.metric().unwrap().instrument_description().unwrap().instrument_name(),
        "bridge.bytes"
    );
    assert!(measurement.log().is_none());
    assert_eq!(measurement.field_index(), None);
}

#[test]
fn macro_inventory_assigns_indices_once_and_keeps_optional_fields_in_the_inventory() {
    #[event("inventory")]
    #[info]
    #[counter(bytes)]
    struct Inventory {
        #[unredacted]
        #[if_none(drop)]
        route: Option<&'static str>,
        #[unredacted]
        bytes: u64,
    }
    let fields = Inventory::DESCRIPTION.fields();
    assert_eq!(
        fields
            .iter()
            .map(|field| (field.field_name(), field.field_index()))
            .collect::<Vec<_>>(),
        [("route", Some(0)), ("bytes", Some(1))],
    );
    let mut visited = Vec::new();
    let result = Inventory { route: None, bytes: 7 }.visit_fields(&mut |field, _| {
        visited.push((field.field_name(), field.field_index()));
        std::ops::ControlFlow::Continue(())
    });
    assert_eq!(result, std::ops::ControlFlow::Continue(()));
    assert_eq!(visited, [("bytes", Some(1))]);
}

#[test]
fn the_hidden_indexer_reassigns_copied_descriptors_instead_of_retaining_stale_positions() {
    let field = observed::__private::field_descriptor(
        std::hint::black_box("source"),
        None,
        Some(observed::metadata::MetricFieldEntry::dimension("region")),
    );
    assert_eq!(
        (field.field_name(), field.metric().unwrap().key(), field.field_index()),
        ("source", "region", None)
    );
    let original = observed::__private::event_fields([
        FieldDescriptor::log_only("first"),
        FieldDescriptor::log_only("second"),
        FieldDescriptor::log_only("third"),
    ]);
    let reordered = observed::__private::event_fields([original[2], original[0]]);
    assert_eq!(
        reordered.map(|field| (field.field_name(), field.field_index())),
        [("third", Some(0)), ("first", Some(1))],
    );
    assert!(observed::__private::event_fields([]).is_empty());
}
