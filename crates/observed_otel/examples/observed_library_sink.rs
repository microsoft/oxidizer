// Copyright (c) Microsoft Corporation.

//! Library-style destination sink with isolated enrichment.
//!
//! Demonstrates `Builder::isolated_enrichment()` on a destination-built
//! sink. A library composes itself into the app's sink via
//! [`Sink::composite`] so one `emit!` lands on both, then opts out of
//! *global* enrichments: entries pushed without a target via
//! `.enrich(&composite, …)` reach the app's records but are filtered out of
//! the library's records. Entries pushed via `.enrich_for(&composite, LIB, …)`
//! still reach the library.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_otel --example observed_library_sink
//! ```

use data_privacy::classified;
use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use log::LevelFilter;
use microsoft_enterprise_data_taxonomy::MicrosoftEnterpriseDataTaxonomy;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, SinkId, emit, event};
use observed_destination_stdout::stdout_logs;
use observed_otel::RedactionPolicy;

const PUBLIC: MicrosoftEnterpriseDataTaxonomy =
    MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData;

static APP: SinkId = SinkId::new("app");
static LIB: SinkId = SinkId::new("lib");

// ---------------------------------------------------------------------------
// Classified newtypes used as enrichment values.
// ---------------------------------------------------------------------------

#[classified(PUBLIC)]
#[derive(Clone)]
struct Tenant(&'static str);

#[classified(PUBLIC)]
#[derive(Clone)]
struct LibVersion(&'static str);

// ---------------------------------------------------------------------------
// Event + Enrichments
// ---------------------------------------------------------------------------

#[event("request.served")]
#[info("Request served on {route}")]
struct RequestServed {
    #[data_class(PUBLIC)]
    route: &'static str,
}

#[derive(Enrichment)]
struct TenantCtx {
    #[dimension(log = "tenant")]
    tenant: Tenant,
}

#[derive(Enrichment)]
struct LibCtx {
    #[dimension(log = "lib.version")]
    lib_version: LibVersion,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Passthrough redaction for the PUBLIC class so attribute values render
    // verbatim instead of being erased by the default fallback.
    let redaction = RedactionPolicy::new().class(
        PUBLIC,
        SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
    );

    // App sink — receives global enrichments (no isolation).
    let app = observed_otel::builder()
        .id(APP)
        .redaction(redaction.clone())
        .log(stdout_logs().level(LevelFilter::Info))
        .build()?;

    // Library sink — opts out of *untargeted* enrichments. Targeted
    // entries (`.enrich_for(sink, LIB, …)`) still reach it; untargeted
    // ones (`.enrich(sink, …)`) do not.
    let lib = observed_otel::builder()
        .id(LIB)
        .isolated_enrichment()
        .redaction(redaction)
        .log(stdout_logs().level(LevelFilter::Info))
        .build()?;

    // Composite so one `emit!` fans out to both. `.enrich(&composite, …)`
    // broadcasts to every child's slot; the isolation flag decides which
    // children actually surface the entry on their records.
    let both = observed::Sink::composite([app.clone(), lib.clone()]);

    println!("--- Global enrichment: app sees `tenant`, lib does not ---");
    (|| {
        emit!(both, RequestServed { route: "/healthz" });
    })
    .enrich(
        &both,
        TenantCtx {
            tenant: Tenant("contoso"),
        },
    )();

    println!();
    println!("--- Targeted enrichment for LIB: only lib sees `lib.version` ---");
    (|| {
        emit!(both, RequestServed { route: "/api/data" });
    })
    .enrich_for(
        &both,
        LIB,
        LibCtx {
            lib_version: LibVersion("2.0"),
        },
    )();

    app.flush()?;
    lib.flush()?;
    Ok(())
}
