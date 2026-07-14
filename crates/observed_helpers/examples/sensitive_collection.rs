// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "example code")]

//! Demonstrates using `SensitiveCollection` inside an emit `Event`, processed
//! by a custom `EventProcessor` that captures fields. Shows both redacted and
//! passthrough output.

use std::collections::{HashMap, VecDeque};
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{DataClass, RedactionEngine};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Event, Sink, emit};
use observed_helpers::SensitiveSlice;
use observed_testing::MicrosoftEnterpriseDataTaxonomy;

fn main() {
    let passthrough = RedactionEngine::builder()
        .suppress_redaction(MicrosoftEnterpriseDataTaxonomy::Euii.data_class())
        .suppress_redaction(MicrosoftEnterpriseDataTaxonomy::Eupi.data_class())
        .build();

    let redacting = RedactionEngine::builder()
        .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .build();
    let passthrough_proc = Arc::new(PrintingProcessor::new("passthrough", passthrough));
    let redacting_proc = Arc::new(PrintingProcessor::new("redacted", redacting));

    let sink = Sink::new(
        "sensitive_collection",
        vec![
            Arc::clone(&passthrough_proc) as Arc<dyn EventProcessor>,
            Arc::clone(&redacting_proc) as Arc<dyn EventProcessor>,
        ],
        tick::SimpleClock::new_system(),
    );

    let emails = [
        Email("alice@example.com".into()),
        Email("bob@example.com".into()),
        Email("carol@example.com".into()),
        Email("dave@example.com".into()),
        Email("eve@example.com".into()),
    ];

    let mut scores: HashMap<UserId, i32> = HashMap::new();
    scores.insert(UserId("user-001".into()), 42);
    scores.insert(UserId("user-002".into()), 99);

    let queue: VecDeque<UserId> = [UserId("pending-a".into()), UserId("pending-b".into()), UserId("pending-c".into())]
        .into_iter()
        .collect();

    // Emit the event — both processors see it.
    emit!(
        sink,
        NotificationBatch {
            recipients: SensitiveSlice::new(emails.iter()),
            lookup_users: SensitiveSlice::new(scores.keys()),
            queue: SensitiveSlice::new(queue.iter()),
            total_count: i64::try_from(emails.len()).unwrap(),
        }
    );

    // Print captured fields from each processor.
    println!();
    passthrough_proc.dump();
    println!();
    redacting_proc.dump();
}

const EXAMPLE_DC: DataClass = DataClass::new("example", "public");

/// An email address classified as EUII.
#[data_privacy::classified(MicrosoftEnterpriseDataTaxonomy::Euii)]
#[derive(Clone)]
struct Email(String);

/// A user identifier classified as EUPI.
#[data_privacy::classified(MicrosoftEnterpriseDataTaxonomy::Eupi)]
#[derive(Clone, Hash, Eq, PartialEq)]
struct UserId(String);

#[derive(Event)]
#[event(name = "user.notification_batch")]
#[log(severity = info, message = "Sending notifications to users")]
struct NotificationBatch<'a> {
    /// Email recipients from a `Vec`.
    recipients: SensitiveSlice<'a, 3>,
    /// User IDs from a `HashMap`'s keys, semicolon-delimited.
    #[dimension(log = "lookup_users")]
    lookup_users: SensitiveSlice<'a, 5, ';'>,
    /// User IDs from a `VecDeque`.
    queue: SensitiveSlice<'a, 5>,
    /// How many we're sending.
    #[data_class(EXAMPLE_DC)]
    total_count: i64,
}

struct PrintingProcessor {
    name: &'static str,
    engine: RedactionEngine,
    captured: Mutex<Vec<(String, String)>>,
}

impl PrintingProcessor {
    fn new(name: &'static str, engine: RedactionEngine) -> Self {
        Self {
            name,
            engine,
            captured: Mutex::new(Vec::new()),
        }
    }

    fn dump(&self) {
        let captured = self.captured.lock().unwrap();
        println!("--- {} ---", self.name);
        for (key, value) in captured.iter() {
            println!("  {key}: {value}");
        }
    }
}

impl EventProcessor for PrintingProcessor {
    fn is_interested(&self, _description: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let mut captured = self.captured.lock().unwrap();

        println!("[{}] event={} body={}", self.name, event.name(), event.body().unwrap());

        let engine = &self.engine;
        let _ = event.visit_fields(&mut |desc, getter| {
            let value = getter(engine);
            captured.push((desc.field_name().to_owned(), value.to_string()));
            ControlFlow::Continue(())
        });
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}
