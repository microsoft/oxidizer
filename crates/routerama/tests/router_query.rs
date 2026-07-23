// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Query extraction is available only with both `route` and `query`.

use http::header::{ACCEPT, CONTENT_TYPE, HOST};
use http_body_util::BodyExt as _;
use routerama::query::FromQuery;
use routerama::route::{Method, Query, QueryRejection, Request, StatusCode, Uri, router};

#[derive(Debug, FromQuery)]
struct Search {
    term: String,
    page: usize,
}

#[derive(Debug, FromQuery)]
struct BorrowedSearch<'query> {
    term: &'query str,
}

struct SearchApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl SearchApi {
    #[route(GET, "/search")]
    async fn search(&self, query: Query<Search>, uri: Uri) -> String {
        format!("{}:{}:{}", query.term, query.page, uri.path())
    }

    #[route(GET, "/borrowed")]
    async fn borrowed(&self, query: Query<BorrowedSearch<'_>>) -> String {
        query.term.to_owned()
    }

    #[route(GET, "/predicate", host = "search.example", produces = "text/plain")]
    async fn predicate(&self, query: Query<BorrowedSearch<'_>>) -> String {
        query.term.to_owned()
    }
}

#[tokio::test]
async fn query_values_are_extracted_while_only_the_uri_path_is_matched() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/search?term=routerama&page=2")
        .body(())
        .expect("the test request uses valid static metadata");

    let response = SearchApi.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();
    assert_eq!(body, b"routerama:2:/search"[..]);
}

#[tokio::test]
async fn query_rejections_become_bad_request_responses() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/search?term=routerama&page=invalid")
        .body(())
        .expect("the test request uses valid static metadata");

    let response = SearchApi.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn query_values_can_borrow_from_request_parts() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/borrowed?term=zero-copy")
        .body(())
        .expect("the test request uses valid static metadata");

    let response = SearchApi.route(request, &()).await;
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();

    assert_eq!(body, b"zero-copy"[..]);
}

#[tokio::test]
async fn route_predicates_run_before_a_borrowing_query_extractor() {
    let request = Request::get("/predicate?term=filtered")
        .header(HOST, "SEARCH.EXAMPLE")
        .header(ACCEPT, "text/*")
        .body(())
        .expect("the test request uses valid static metadata");

    let response = SearchApi.route(request, &()).await;
    assert_eq!(response.headers()[CONTENT_TYPE], "text/plain");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();
    assert_eq!(body, b"filtered"[..]);
}

struct QueryCatcher;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl QueryCatcher {
    #[route(GET, "/caught-query")]
    async fn query(&self, query: Query<Search>) -> String {
        query.term.clone()
    }

    #[catch(QueryRejection)]
    async fn catch_query(&self, _rejection: QueryRejection) -> (StatusCode, &'static str) {
        (StatusCode::UNPROCESSABLE_ENTITY, "query-caught")
    }
}

#[tokio::test]
async fn query_rejections_can_use_a_typed_catcher() {
    let request = Request::get("/caught-query?term=routerama&page=invalid")
        .body(())
        .expect("the test request uses valid static metadata");
    let response = QueryCatcher.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("the generated response body succeeds")
            .to_bytes(),
        b"query-caught"[..]
    );
}
