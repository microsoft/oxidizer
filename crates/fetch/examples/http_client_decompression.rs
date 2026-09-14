// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates automatic response decompression with a fully mocked transport.

use bytesbuf::BytesView;
use compressors::Resources;
use compressors::format::Format;
use fetch::fake::{FakeDeps, FakeHandler};
use fetch::options::{DecompressionMethod, DecompressionOptions};
use fetch::{HttpClient, HttpResponseBuilder};
use http::HeaderValue;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};

fn main() -> Result<(), ohno::AppError> {
    futures::executor::block_on(async {
        let expected = "a mocked gzip response";
        let compressed = compressors::format::compress(
            Format::Gzip,
            BytesView::copied_from_slice(expected.as_bytes(), &fetch::HttpBodyBuilder::new_fake()),
            Resources::global(),
        )?;

        let handler = FakeHandler::from_fn(move |request| {
            assert_eq!(request.headers().get(ACCEPT_ENCODING), Some(&HeaderValue::from_static("gzip")));

            HttpResponseBuilder::new_fake()
                .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                .bytes(compressed.clone())
                .build()
        });
        let client = HttpClient::builder_fake(handler, FakeDeps::default())
            .decompression(DecompressionOptions::with_methods(&[DecompressionMethod::Gzip]))
            .build();

        let response = client.get("https://example.com").fetch().await?;
        assert!(response.headers().get(CONTENT_ENCODING).is_none());
        assert_eq!(response.into_body().into_text().await?, expected);

        Ok(())
    })
}
