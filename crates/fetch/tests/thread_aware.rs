// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for thread-aware (per-core) client relocation.

use std::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use fetch::HttpClient;
use fetch::tokio::TokioDeps;
use futures::future::join_all;
use thread_aware::ThreadAware;
use thread_aware::affinity::pinned_affinities;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_isolated_on_tokio() {
    let counts = Arc::new(AtomicUsize::new(0));
    let clone = Arc::clone(&counts);

    let client = HttpClient::builder_tokio(TokioDeps::default())
        .custom_pipeline(move |dispatch, _| {
            clone.fetch_add(1, Ordering::Relaxed);
            dispatch
        })
        .build();
    assert_eq!(counts.load(Ordering::Relaxed), 1);

    for affinity in pinned_affinities(&[2, 2]) {
        let mut client_clone = client.clone();
        client_clone.relocate(None, affinity);
    }
    assert_eq!(counts.load(Ordering::Relaxed), 1);
}

#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokio_client_relocated_ensure_works() {
    let server = serve(Bytes::from("Hello World!")).await;
    let url = server.uri() + "/hello-world";
    let client = HttpClient::builder_tokio(TokioDeps::default()).insecure_allow_http().build();

    // Use the client on tokio
    let text = client.get(url.clone()).fetch_text().await.unwrap().into_body();
    assert_eq!(text, "Hello World!");

    // relocate the client and use it on worker threads
    let handles = pinned_affinities(&[2, 2])
        .into_iter()
        .map(|affinity| {
            let mut client = client.clone();
            let url = url.clone();
            tokio::spawn(async move {
                client.relocate(None, affinity);
                let text = client.get(url).fetch_text().await.unwrap().into_body();

                assert_eq!(text, "Hello World!");
            })
        })
        .collect::<Vec<_>>();
    _ = join_all(handles).await;

    // Use the client on tokio again
    let text = client.get(url.clone()).fetch_text().await.unwrap().into_body();
    assert_eq!(text, "Hello World!");
}

async fn serve(body: impl Into<Bytes>) -> MockServer {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello-world"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.into().to_vec()))
        .mount(&mock_server)
        .await;

    mock_server
}
