// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compresses and decompresses a body that arrives over time on tokio.
//!
//! The example does not retain the complete body. Peak memory follows the chunk size instead of the
//! body size.

use std::env;
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::{CompressionStream, Resources, gzip};
use tick::{Clock, ClockControl, Timer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};

/// The interval between chunks from the simulated upstream.
const ARRIVAL_PERIOD: Duration = Duration::from_millis(1);

/// Simulates an upstream source, such as a socket, that produces a body gradually.
///
/// The caller supplies the clock and can use simulated time. Input views and engines use the same
/// caller-owned [`Resources`].
fn body(clock: Clock, resources: Resources) -> impl Stream<Item = Result<BytesView, std::io::Error>> {
    let (sender, receiver) = mpsc::channel(4);

    tokio::spawn(async move {
        let mut arrivals = Timer::new(&clock, ARRIVAL_PERIOD);

        for event in 0..200 {
            let _tick = arrivals.next().await;

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
    // Arriving chunks and both engines use this provider and its shared engine pool.
    let resources = Resources::new(GlobalPool::new());

    // Anvil runs this example as an automated test. An auto-advancing clock avoids two seconds of
    // wall time and removes a dependency on runtime scheduling.
    let clock = if env::var_os("ANVIL_EXAMPLE").is_some() {
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
