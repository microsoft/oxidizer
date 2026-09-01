// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compressing and decompressing a body that arrives over time, on tokio.
//!
//! The body is never held whole: each chunk passes through the codec and leaves, so peak memory
//! follows the chunk size rather than the size of the body.
//!
//! Run with `cargo run --example tokio_stream --all-features`.

use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::{CompressionStream, gzip};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};

/// Stands in for an upstream that produces a body gradually, such as a socket.
fn body(memory: GlobalPool) -> impl Stream<Item = Result<BytesView, std::io::Error>> {
    let (sender, receiver) = mpsc::channel(4);

    tokio::spawn(async move {
        let mut clock = tokio::time::interval(Duration::from_micros(50));

        for event in 0..200 {
            clock.tick().await;

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

    let compressed = CompressionStream::compress(body(memory.clone()), gzip::Compressor::new(memory.clone()));
    let mut plain = CompressionStream::decompress(compressed, gzip::Decompressor::new(memory));

    let mut bytes = 0;
    while let Some(chunk) = plain.next().await {
        bytes += chunk?.len();
    }

    println!("{bytes} bytes recovered");

    Ok(())
}
