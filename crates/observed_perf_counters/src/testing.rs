// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write as _;

#[cfg(all(windows, target_pointer_width = "64"))]
use crate::schema::{CounterSchema, CounterSpec, Manifest};

// Synthetic external input for unit scenarios; production never invents identities.
pub(crate) fn manifest(category: &str, names: &[&str]) -> String {
    let category = quick_xml::escape::escape(category);
    let mut counters = String::new();
    for (index, name) in names.iter().enumerate() {
        let name = quick_xml::escape::escape(*name);
        let id = 1000 - index;
        write!(counters,
            "<counter id=\"{id}\" name=\"{name}\" description=\"fixture\" type=\"perf_counter_large_rawcount\" detailLevel=\"standard\" defaultScale=\"0\">\
             <counterAttributes><counterAttribute name=\"reference\"/></counterAttributes></counter>"
        ).unwrap();
    }
    format!(
        "<instrumentationManifest xmlns=\"http://schemas.microsoft.com/win/2004/08/events\"><instrumentation>\
         <counters xmlns=\"http://schemas.microsoft.com/win/2005/12/counters\" schemaVersion=\"1.1\"><provider providerGuid=\"{{6ae97c31-c343-4f66-94b7-027b2646b55e}}\" providerType=\"userMode\" callback=\"custom\" applicationIdentity=\"FixtureCounters.dll\">\
         <counterSet name=\"{category}\" guid=\"{{8fa87d62-531d-45e3-89e3-b2f4b5b2a8b7}}\" instances=\"multiple\">{counters}</counterSet>\
         </provider></counters></instrumentation></instrumentationManifest>"
    )
}

#[cfg(all(windows, target_pointer_width = "64"))]
pub(crate) fn counter_schema(category: &str, names: &[&str]) -> CounterSchema {
    let manifest = Manifest::parse(&manifest(category, names)).unwrap();
    let specs = names
        .iter()
        .enumerate()
        .map(|(source_index, name)| CounterSpec {
            name: (*name).into(),
            source_index,
        })
        .collect::<Vec<_>>();
    CounterSchema::from_manifest(&manifest, category, "worker", &specs).unwrap()
}

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// Lightweight event-callback subscriber shared by diagnostic tests; spans are ignored.
pub(crate) struct OnEvent<F>(pub(crate) F);

impl<F: Fn() + Send + Sync + 'static> Subscriber for OnEvent<F> {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {
        (self.0)();
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}
