// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compressing and decompressing a body that arrives over time, on tokio.
//!
//! The body is never held whole: each chunk passes through the codec and leaves, so peak memory
//! follows the chunk size rather than the size of the body.

use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::{CompressionStream, Resources, gzip};
use tick::{Clock, PeriodicTimer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};

/// Stands in for an upstream that produces a body gradually, such as a socket.
fn body(memory: GlobalPool) -> impl Stream<Item = Result<BytesView, std::io::Error>> {
    let (sender, receiver) = mpsc::channel(4);

    tokio::spawn(async move {
        // Time comes from a `tick::Clock` rather than the runtime directly, so a test can drive
        // this timer instantly instead of waiting for it.
        let clock = Clock::new_tokio();
        let mut arrivals = PeriodicTimer::new(&clock, Duration::from_micros(50));

        for event in 0..200 {
            arrivals.next().await;

            let line = format!("{{\"event\":{event},\"message\":\"a log line\"}}\n");
            let chunk = BytesView::copied_from_slice(line.as_bytes(), &memory);

            if sender.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
    });

    ReceiverStream::new(receiver)
}

#[tokio::main]
async fn main() -> Result<(), compressors::Error> {
    let memory = GlobalPool::new();

    let compressed = CompressionStream::compress(body(memory.clone()), gzip::Compressor::new(&Resources::default()));
    let mut plain = CompressionStream::decompress(compressed, gzip::Decompressor::new(&Resources::default()));

    let mut bytes = 0;
    while let Some(chunk) = plain.next().await {
        bytes += chunk?.len();
    }

    println!("{bytes} bytes recovered");

    Ok(())
}
