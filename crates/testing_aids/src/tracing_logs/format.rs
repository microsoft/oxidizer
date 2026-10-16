// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{Format, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Stages events locally so a formatting panic cannot contaminate a later event.
pub(super) struct CompleteEvent;

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
