// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for HTTP client URI handling and normalization.

#![allow(clippy::unwrap_used, reason = "test code")]

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::{HttpBodyBuilder, HttpClient, HttpResponseBuilder};
use http::{Request, StatusCode};
use layered::Service;
use templated_uri::{BaseUri, PathAndQuery, Uri};

fn prepare_client_with_base_uri(base_uri: BaseUri) -> HttpClient {
    HttpClient::builder_fake(
        FakeHandler::from_async_fn(|request| async move {
            let response = format!("requested URI: {}", request.uri());
            HttpResponseBuilder::new_fake().status(StatusCode::OK).text(response).build()
        }),
        FakeDeps::default(),
    )
    .base_uri(base_uri)
    .build()
}

async fn fetch_text(client: &HttpClient, uri: Uri) -> String {
    client.get(uri).fetch_text().await.unwrap().into_body()
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fetch_method() {
    let client = prepare_client_with_base_uri(BaseUri::from_static("https://default.example.com"));

    // Replace a missing request endpoint with the client endpoint.
    let uri_without_endpoint: Uri = Uri::default().with_path_and_query(PathAndQuery::from_static("/foo"));
    let response_endpoint_none = fetch_text(&client, uri_without_endpoint).await;
    assert_eq!(
        response_endpoint_none, "requested URI: https://default.example.com/foo",
        "the request must use the client endpoint"
    );

    // Use an empty path and query.
    let uri_without_endpoint: Uri = Uri::default().with_path_and_query(PathAndQuery::from_static("/"));
    let response_endpoint_none = fetch_text(&client, uri_without_endpoint).await;
    assert_eq!(
        response_endpoint_none, "requested URI: https://default.example.com/",
        "the request must use the client endpoint"
    );

    // Use the default Uri.
    let response_endpoint_none = fetch_text(&client, Uri::default()).await;
    assert_eq!(
        response_endpoint_none, "requested URI: https://default.example.com/",
        "the request must use the client endpoint"
    );

    // Replace the request endpoint.
    let response_endpoint_set = fetch_text(&client, Uri::try_from("https://example.com/bar").unwrap()).await;
    assert_eq!(
        response_endpoint_set, "requested URI: https://default.example.com/bar",
        "the client endpoint must replace the request endpoint"
    );

    // Use another host so the client must replace the full endpoint.
    let response_endpoint_different_host = fetch_text(&client, Uri::try_from("https://192.0.2.42/bar").unwrap()).await;
    assert_eq!(
        response_endpoint_different_host, "requested URI: https://default.example.com/bar",
        "the client endpoint must replace the request endpoint"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fetch_with_base_uri_with_path() {
    let client = prepare_client_with_base_uri(BaseUri::from_static("https://default.example.com/base/"));

    let uri: Uri = Uri::default().with_path_and_query(PathAndQuery::from_static("/foo"));
    let full_uri = fetch_text(&client, uri).await;

    assert_eq!(
        full_uri, "requested URI: https://default.example.com/base/foo",
        "the base URI path must precede the request path"
    );

    let uri: Uri = Uri::default().with_path_and_query(PathAndQuery::from_static("/"));
    let full_uri = fetch_text(&client, uri).await;
    assert_eq!(
        full_uri, "requested URI: https://default.example.com/base/",
        "the base URI path must precede the request path"
    );

    let uri: Uri = Uri::default();
    let full_uri = fetch_text(&client, uri).await;
    assert_eq!(
        full_uri, "requested URI: https://default.example.com/base/",
        "the base URI path must precede the request path"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn change_client_base_uri() {
    let client = prepare_client_with_base_uri(BaseUri::from_static("https://default.example.com/foo/"));

    let client_2 = client.with_base_uri(BaseUri::from_static("https://default.example.com/bar/"));

    let uri: Uri = Uri::default().with_path_and_query(PathAndQuery::from_static("/api"));
    let full_uri = fetch_text(&client, uri.clone()).await;

    assert_eq!(
        full_uri, "requested URI: https://default.example.com/foo/api",
        "the original client must keep its base URI"
    );

    let full_uri = fetch_text(&client_2, uri).await;

    assert_eq!(
        full_uri, "requested URI: https://default.example.com/bar/api",
        "the derived client must use the new base URI"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn send_request() {
    let client = prepare_client_with_base_uri(BaseUri::from_static("https://default.example.com"));
    let request = Request::builder()
        .uri("/send_request")
        .body(HttpBodyBuilder::new_fake().empty())
        .unwrap();
    let response = client.execute(request).await.unwrap();
    let response = response.into_body().into_text().await.unwrap();
    assert_eq!(
        response, "requested URI: https://default.example.com/send_request",
        "the request must use the client endpoint"
    );
}

testing_aids::init_tracing!();
