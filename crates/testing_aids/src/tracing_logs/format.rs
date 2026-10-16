// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields, MakeWriter};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

pub(super) fn formatting_layer<S, W>(writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    FormattingLayer {
        events: tracing_subscriber::fmt::layer()
            .event_format(CompleteEvent)
            .with_ansi(false)
            .with_writer(writer),
    }
}

/// Uses the standard event formatter, but never formats user span fields under a lock.
struct FormattingLayer<S, W> {
    events: tracing_subscriber::fmt::Layer<S, DefaultFields, CompleteEvent, W>,
}

impl<S, W> Layer<S> for FormattingLayer<S, W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let fields = format_fields(attrs);
        let span = ctx.span(id).expect("the registry creates the span before notifying layers");
        span.extensions_mut().insert(FormattedFields::<DefaultFields>::new(fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        // Nested events must see the last complete state, not wait for our write lock.
        // A panic here also leaves that state and the extensions lock untouched.
        let fields = format_fields(values);
        let span = ctx.span(id).expect("recording requires a live span in the registry");
        let mut extensions = span.extensions_mut();
        let current = extensions
            .get_mut::<FormattedFields<DefaultFields>>()
            .expect("on_new_span installs formatted fields before the span can be recorded");
        if !current.fields.is_empty() && !fields.is_empty() {
            current.fields.push(' ');
        }
        current.fields.push_str(&fields);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        self.events.on_event(event, ctx);
    }
}

fn format_fields(fields: impl RecordFields) -> String {
    let mut formatted = String::new();
    DefaultFields::new()
        .format_fields(Writer::new(&mut formatted), fields)
        .expect("Debug and Display implementations must not fail when formatting into a String");
    formatted
}

/// Stages events locally so a formatting panic cannot contaminate a later event.
struct CompleteEvent;

impl<S, N> FormatEvent<S, N> for CompleteEvent
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        // The fmt layer clears its reusable thread-local buffer only on normal return.
        // Keep partial output off that buffer if a user's formatter unwinds.
        let mut formatted = String::new();
        // Timestamps make captures non-deterministic and require a clock unavailable
        // under Miri isolation. All sinks share the same plain-text output.
        Format::default()
            .without_time()
            .with_ansi(false)
            .format_event(ctx, Writer::new(&mut formatted), event)?;
        writer.write_str(&formatted)
    }
}
