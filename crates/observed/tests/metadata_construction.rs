// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated metadata inspection and macro-owned field indexing.

use observed::metadata::{InstrumentKind, MetricValueKind};
use observed::{Event, Severity, event};

#[test]
fn generated_descriptions_expose_signal_metadata_without_consumer_construction() {
    #[event("plain")]
    struct Plain;
    #[event("logged")]
    #[warning]
    struct Logged;
    #[event("count")]
    #[counter]
    struct Count;
    #[event("disabled", disabled)]
    #[info]
    struct Disabled;
    let plain = Plain::DESCRIPTION;
    assert_eq!(
        (
            plain.name(),
            plain.is_log(),
            plain.contains_metrics(),
            plain.is_disabled(),
            plain.fields().len()
        ),
        ("plain", false, false, false, 0),
    );
    assert_eq!(plain.type_id(), Some(std::any::TypeId::of::<Plain>()));
    assert_eq!(Logged::DESCRIPTION.log().unwrap().severity(), Severity::Warn);
    assert!(!Logged::DESCRIPTION.contains_metrics());
    assert_eq!(Count::DESCRIPTION.metric().unwrap().instrument_name(), "count");
    assert!(!Count::DESCRIPTION.is_log());
    assert!(Disabled::DESCRIPTION.is_disabled());
}

#[test]
fn generated_fields_expose_routes_and_measurement_metadata() {
    #[event("routed")]
    #[info]
    #[counter(bytes, name = "transferred.bytes", unit = "By", desc = "Transferred bytes")]
    struct Routed {
        #[unredacted]
        message: &'static str,
        #[unredacted]
        #[dimension(log = "cloud.region", metric = "region")]
        region: &'static str,
        #[unredacted]
        bytes: u64,
    }
    let fields = Routed::DESCRIPTION.fields();
    let log = fields[0];
    assert_eq!(
        (log.field_name(), log.log().unwrap().key(), log.field_index()),
        ("message", "message", Some(0))
    );
    assert!(log.metric().is_none());
    let dimension = fields[1];
    assert_eq!(dimension.field_name(), "region");
    assert_eq!(dimension.log().unwrap().key(), "cloud.region");
    assert_eq!(dimension.metric().unwrap().key(), "region");
    assert!(dimension.metric().unwrap().instrument_description().is_none());
    assert_eq!(dimension.field_index(), Some(1));
    let measurement = fields[2].metric().unwrap().instrument_description().unwrap();
    assert_eq!(
        (
            measurement.instrument_name(),
            measurement.kind(),
            measurement.description(),
            measurement.unit(),
            measurement.value_kind()
        ),
        (
            "transferred.bytes",
            InstrumentKind::Counter,
            "Transferred bytes",
            "By",
            Some(MetricValueKind::U64)
        )
    );
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
    let metric = observed::__private::metric_description(
        std::hint::black_box("count"),
        InstrumentKind::Counter,
        "",
        "",
        Some(MetricValueKind::U64),
    );
    let instrument = observed::__private::metric_field("value", Some(metric));
    assert_eq!(
        (instrument.key(), instrument.instrument_description().unwrap().instrument_name()),
        ("value", "count"),
    );
    let field = observed::__private::field_descriptor(
        std::hint::black_box("source"),
        None,
        Some(observed::__private::metric_field("region", None)),
    );
    assert_eq!(
        (field.field_name(), field.metric().unwrap().key(), field.field_index()),
        ("source", "region", None)
    );
    let original = observed::__private::event_fields([
        observed::__private::field_descriptor("first", Some(observed::__private::log_field("first")), None),
        observed::__private::field_descriptor("second", None, None),
        observed::__private::field_descriptor("third", None, None),
    ]);
    let reordered = observed::__private::event_fields([original[2], original[0]]);
    assert_eq!(
        reordered.map(|field| (field.field_name(), field.field_index())),
        [("third", Some(0)), ("first", Some(1))],
    );
    assert!(observed::__private::event_fields([]).is_empty());
}

#[test]
fn the_macro_source_factory_preserves_the_emission_location() {
    let line = std::hint::black_box(42);
    let location = observed::__private::source_location("emitter", "src/request.rs", line);
    assert_eq!(
        (location.crate_name(), location.file(), location.line()),
        ("emitter", "src/request.rs", 42),
    );
}
