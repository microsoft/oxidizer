// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures process-global mapping counters without libtest or other test threads.
#![expect(clippy::unwrap_used, reason = "The standalone regression stops on any capture or decoding failure")]

use support::stats;

mod support;

rallocator::rallocator!();

const TEST_NAME: &str = "snapshot_capture_does_not_add_allocator_mappings";

enum Invocation {
    Run,
    List,
    Skip,
}

fn main() {
    // Parse and release all argument storage before warming the measured process.
    match invocation() {
        Ok(Invocation::Run) => {
            snapshot_capture_does_not_add_allocator_mappings();
            println!("{TEST_NAME} ... ok");
        }
        Ok(Invocation::List) => println!("{TEST_NAME}: test"),
        Ok(Invocation::Skip) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

fn invocation() -> Result<Invocation, String> {
    let mut args = std::env::args().skip(1);
    let mut list = false;
    let mut exact = false;
    let mut ignored = false;
    let mut filters = Vec::new();
    let mut skips = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--exact" => exact = true,
            "--ignored" => ignored = true,
            "--nocapture" | "--show-output" | "--include-ignored" | "--test" | "--quiet" | "-q" => {}
            "--format" | "--color" | "--test-threads" | "--skip" => {
                let value = args.next().ok_or_else(|| format!("missing value for {arg}"))?;
                let valid = match arg.as_str() {
                    "--format" => matches!(value.as_str(), "terse" | "pretty"),
                    "--color" => matches!(value.as_str(), "auto" | "always" | "never"),
                    "--test-threads" => value.parse::<usize>().is_ok_and(|count| count > 0),
                    "--skip" => {
                        skips.push(value);
                        true
                    }
                    _ => unreachable!(),
                };
                if !valid {
                    return Err(format!("unsupported value for {arg}"));
                }
            }
            _ if arg.starts_with('-') => return Err(format!("unsupported test argument: {arg}")),
            _ => filters.push(arg),
        }
    }
    let matches = |filter: &String| {
        if exact { TEST_NAME == filter } else { TEST_NAME.contains(filter) }
    };
    if ignored || skips.iter().any(matches) || (!filters.is_empty() && !filters.iter().any(matches)) {
        Ok(Invocation::Skip)
    } else if list {
        Ok(Invocation::List)
    } else {
        Ok(Invocation::Run)
    }
}

fn snapshot_capture_does_not_add_allocator_mappings() {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: false,
            capture_backtraces: false,
            ..Default::default()
        },
        ..Default::default()
    });
    drop(snapshot().unwrap());
    let before = stats().unwrap();

    let captured = snapshot().unwrap();
    let after = stats().unwrap();

    assert!(after.mapped_bytes <= before.mapped_bytes, "before: {before:?}; after: {after:?}");
    assert_eq!(after.os_mappings, before.os_mappings);
    drop(captured);
}

fn snapshot() -> Option<seismograph::snapshot::Snapshot> {
    seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).ok()
}
