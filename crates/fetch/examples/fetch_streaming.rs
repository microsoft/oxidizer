// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streams an HTTP response to a file.

use std::io::{BufWriter, Write};

use fetch::HttpClient;
use futures::TryStreamExt;
use observed::Sink;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // An application passes its own sink. `Sink::noop()` drops every event.
    let client = HttpClient::new_tokio(Sink::noop());

    // Send a GET request and read the body as a stream.
    let body = client.get("https://example.com").fetch().await?.into_body();
    let mut stream = body.into_stream();

    let mut file = BufWriter::new(tempfile::tempfile()?);

    while let Some(mut chunk) = stream.try_next().await? {
        let size = chunk.len();

        // Write each chunk as it arrives.
        std::io::copy(&mut chunk, &mut file)?;
        println!("Wrote chunk to file, size: {size}");
    }

    file.flush()?;
    println!("File download is complete.");

    Ok(())
}
