// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compressing and decompressing a body that arrives over time, on tokio.
//!
//! The body is never held whole: each chunk passes through the engine and leaves, so peak memory
//! follows the chunk size rather than the size of the body.

use std::env;
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::{CompressionStream, Resources, gzip};
use tick::{Clock, ClockControl, PeriodicTimer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};

/// How often a chunk arrives from the stand-in upstream.
const ARRIVAL_PERIOD: Duration = Duration::from_millis(1);

/// Stands in for an upstream that produces a body gradually, such as a socket.
///
/// Takes its clock rather than choosing one, so a caller can drive the arrivals in simulated time
/// instead of waiting for them. Input views come from the same [`Resources`] the engines do, so one
/// caller-owned provider covers the whole pipeline.
fn body(clock: Clock, resources: Resources) -> impl Stream<Item = Result<BytesView, std::io::Error>> {
    let (sender, receiver) = mpsc::channel(4);

    tokio::spawn(async move {
        let mut arrivals = PeriodicTimer::new(&clock, ARRIVAL_PERIOD);

        for event in 0..200 {
            arrivals.next().await;

            let line = format!("{{\"event\":{event},\"message\":\"a log line\"}}\n");
            let chunk = BytesView::copied_from_slice(line.as_bytes(), resources.memory());

            if sender.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
    });

    ReceiverStream::new(receiver)
}

#[tokio::main]
async fn main() -> Result<(), compressors::Error> {
    // Held once and handed to everything: the arriving chunks, the compressor and the decompressor
    // all draw on this provider and share its pool of recycled engines.
    let resources = Resources::new(GlobalPool::new());

    // Under `scripts/run-examples.rs` this runs as an automated check, where waiting out 200 real
    // arrivals would be two seconds of wall clock and a dependency on runtime scheduling. A clock
    // that advances itself on every query settles the same 200 arrivals immediately.
    let clock = if env::var_os("IS_TESTING").is_some() {
        ClockControl::new().auto_advance(ARRIVAL_PERIOD).to_clock()
    } else {
        Clock::new_tokio()
    };

    let compressed = CompressionStream::compress(body(clock, resources.clone()), gzip::Compressor::new(&resources));
    let mut plain = CompressionStream::decompress(compressed, gzip::Decompressor::new(&resources));

    let mut bytes = 0;
    while let Some(chunk) = plain.next().await {
        bytes += chunk?.len();
    }

    println!("{bytes} bytes recovered");

    Ok(())
}
