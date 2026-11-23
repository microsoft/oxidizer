// Copyright (c) Microsoft Corporation.

//! Example demonstrating the rolling file log destination.
//!
//! Uses a small `max_file_size` to trigger rotation quickly, then
//! prints the resulting directory listing.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_otel --features rolling_file --example observed_rolling_file
//! ```

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use log::LevelFilter;
use microsoft_enterprise_data_taxonomy::MicrosoftEnterpriseDataTaxonomy;
use observed::{emit, event};
use observed_destination_file::{FileLimit, rolling_file_logs};
use observed_otel::RedactionPolicy;

const PUBLIC: MicrosoftEnterpriseDataTaxonomy =
    MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData;

#[event("worker.tick")]
#[info("Worker tick {iteration}")]
struct WorkerTick {
    #[data_class(PUBLIC)]
    iteration: i64,
    #[data_class(PUBLIC)]
    items_processed: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir()?;
    let log_file = temp_dir.path().join("rolling_example.log");

    let redaction = RedactionPolicy::new().class(
        MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData,
        SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
    );

    let sink = observed_otel::builder()
        .redaction(redaction)
        .log(
            rolling_file_logs(&log_file)
                .level(LevelFilter::Trace)
                .max_file_size(5000) // small size to demonstrate rolling
                .file_limit(FileLimit::MaxFiles(3))
                .compression(false),
        )
        .build()?;

    println!("Writing rolling logs to: {}", temp_dir.path().display());
    println!("\nDirectory before:");
    print_dir(temp_dir.path())?;

    for i in 1..100 {
        emit!(
            sink,
            WorkerTick {
                iteration: i,
                items_processed: i * 10,
            }
        );
    }

    sink.flush()?;

    println!("\nDirectory after:");
    print_dir(temp_dir.path())?;

    Ok(())
}

fn print_dir(dir: &std::path::Path) -> std::io::Result<()> {
    for entry in dir.read_dir()? {
        let entry = entry?;
        println!(
            "  {:<50} ({:>6} bytes)",
            entry.file_name().display(),
            entry.metadata()?.len()
        );
    }
    Ok(())
}
