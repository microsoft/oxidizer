// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public in-memory handler and generated transcoder regression oracles.

use futures::StreamExt;
use futures::executor::block_on;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use pb::Library;
use rest_over_grpc::handling::{Code, Context};
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_tests::custom::{InMemoryLibrary, Transcoder, pb};

#[test]
fn gamma_survivor_library_filter_stream_and_genre_oracles() {
    block_on(async {
        let library = InMemoryLibrary;
        for (filter, expected) in [
            ("", vec![("shelves/1", "history"), ("shelves/2", "science")]),
            ("science", vec![("shelves/2", "science")]),
            ("history", vec![("shelves/1", "history")]),
            ("absent", vec![]),
        ] {
            let request = pb::ListShelvesRequest { filter: filter.into() };
            let mut context = Context::default();
            let response = library.list_shelves(request.clone(), &mut context).await.unwrap();
            assert_eq!(
                response
                    .shelves
                    .iter()
                    .map(|s| (s.name.as_str(), s.theme.as_str()))
                    .collect::<Vec<_>>(),
                expected
            );
            let stream = library
                .stream_shelves(request.clone(), &mut context)
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;
            assert_eq!(stream.into_iter().map(Result::unwrap).collect::<Vec<_>>(), response.shelves);
            let themes = library
                .stream_shelf_themes(request, &mut context)
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;
            assert_eq!(themes.into_iter().map(Result::unwrap).collect::<Vec<_>>(), response.shelves);
        }
        for (genre, expected) in [(pb::Genre::History, "history"), (pb::Genre::Science, "science")] {
            let response = library
                .list_shelves_by_genre(pb::ListShelvesByGenreRequest { genre: genre as i32 }, &mut Context::default())
                .await
                .unwrap();
            assert_eq!(
                response.shelves,
                [pb::Shelf {
                    name: format!("shelves/{expected}"),
                    theme: expected.into()
                }]
            );
        }
        let error = library
            .list_shelves_by_genre(
                pb::ListShelvesByGenreRequest {
                    genre: pb::Genre::Unspecified as i32,
                },
                &mut Context::default(),
            )
            .await
            .unwrap_err();
        assert_eq!((error.code(), error.message()), (Code::InvalidArgument, "genre is required"));
    });
}

#[test]
fn gamma_survivor_library_metadata_missing_details_and_creation() {
    block_on(async {
        let library = InMemoryLibrary;
        let mut request_headers = HeaderMap::new();
        request_headers.insert("x-trace-id", HeaderValue::from_static("trace-marker"));
        let mut context = Context::new(request_headers);
        let shelf = library
            .get_shelf(pb::GetShelfRequest { shelf: "seven".into() }, &mut context)
            .await
            .unwrap();
        assert_eq!(
            shelf,
            pb::Shelf {
                name: "shelves/seven".into(),
                theme: "history".into()
            }
        );
        assert_eq!(context.response_headers()["x-trace-id"], "trace-marker");
        assert_eq!(context.response_headers()[header::ETAG], "\"shelf-v1\"");
        let mut empty = Context::default();
        library
            .get_shelf(pb::GetShelfRequest { shelf: "eight".into() }, &mut empty)
            .await
            .unwrap();
        assert!(!empty.response_headers().contains_key("x-trace-id"));
        let error = library
            .get_shelf(pb::GetShelfRequest { shelf: "missing".into() }, &mut Context::default())
            .await
            .unwrap_err();
        assert_eq!((error.code(), error.message()), (Code::NotFound, "no such shelf"));
        assert_eq!(
            error.details(),
            [serde_json::json!({
                "@type":"type.googleapis.com/google.rpc.ResourceInfo",
                "resourceType":"library.Shelf","resourceName":"shelves/missing"
            })]
        );
        let mut context = Context::default();
        let created = library
            .create_shelf(
                pb::CreateShelfRequest {
                    shelf: Some(pb::Shelf {
                        name: "ignored".into(),
                        theme: "retained".into(),
                    }),
                },
                &mut context,
            )
            .await
            .unwrap();
        assert_eq!(
            created,
            pb::Shelf {
                name: "shelves/created".into(),
                theme: "retained".into()
            }
        );
        assert_eq!(context.response_headers()[header::LOCATION], "/v1/shelves/created");
        let error = library
            .create_shelf(pb::CreateShelfRequest { shelf: None }, &mut Context::default())
            .await
            .unwrap_err();
        assert_eq!((error.code(), error.message()), (Code::InvalidArgument, "shelf is required"));
    });
}

#[test]
fn gamma_survivor_library_update_identity_and_required_payload() {
    block_on(async {
        for force in [false, true] {
            let request = pb::UpdateShelfRequest {
                id: "changed".into(),
                force,
                shelf: Some(pb::Shelf {
                    name: "ignored".into(),
                    theme: "science".into(),
                }),
            };
            let result = InMemoryLibrary.update_shelf(request, &mut Context::default()).await.unwrap();
            assert_eq!(
                result.shelf,
                Some(pb::Shelf {
                    name: "shelves/changed".into(),
                    theme: "science".into()
                })
            );
            assert_eq!(result.result_code, i32::from(force));
        }
        let error = InMemoryLibrary
            .update_shelf(
                pb::UpdateShelfRequest {
                    id: "unused".into(),
                    force: true,
                    shelf: None,
                },
                &mut Context::default(),
            )
            .await
            .unwrap_err();
        assert_eq!((error.code(), error.message()), (Code::InvalidArgument, "shelf is required"));
        assert_eq!(
            InMemoryLibrary
                .get_tree(pb::GetTreeRequest { path: "a/b/".into() }, &mut Context::default())
                .await
                .unwrap(),
            pb::GetTreeResponse { path: "a/b/".into() }
        );
    });
}

#[test]
fn gamma_survivor_generated_transcoder_route_query_errors_and_headers() {
    block_on(async {
        let transcoder = Transcoder::new(InMemoryLibrary);
        for (method, path, status, expected) in [
            (
                "GET",
                "/v1/shelves?filter=history",
                StatusCode::OK,
                serde_json::json!({"shelves":[{"name":"shelves/1","theme":"history"}]}),
            ),
            ("GET", "/v1/shelves?filter=absent", StatusCode::OK, serde_json::json!({})),
            (
                "GET",
                "/v1/shelves/genre/HISTORY",
                StatusCode::OK,
                serde_json::json!({"shelves":[{"name":"shelves/history","theme":"history"}]}),
            ),
            ("GET", "/v1/tree/a%2Fb/c", StatusCode::OK, serde_json::json!({"path":"a%2Fb/c"})),
        ] {
            let response = transcoder.transcode(method, path, HeaderMap::new(), b"").await;
            let TranscodeResponse::Unary(response) = response else {
                panic!("expected unary")
            };
            assert_eq!(response.status(), status);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(response.body()).unwrap(),
                expected,
                "{path}"
            );
        }

        let mut headers = HeaderMap::new();
        headers.insert("x-trace-id", HeaderValue::from_static("wire-marker"));
        let response = transcoder.transcode("GET", "/v1/shelves/7", headers, b"").await;
        let TranscodeResponse::Unary(response) = response else {
            panic!("expected unary")
        };
        assert_eq!(response.headers()["x-trace-id"], "wire-marker");
        assert_eq!(response.headers()[header::ETAG], "\"shelf-v1\"");
        for (method, path) in [
            ("GET", "/v1/shelves?filter=%GG"),
            ("GET", "/v1/shelves/genre/UNKNOWN"),
            ("GET", "/v1/shelves/%FF"),
        ] {
            let response = transcoder.transcode(method, path, HeaderMap::new(), b"").await;
            let TranscodeResponse::Unary(response) = response else {
                panic!("expected unary")
            };
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
            assert_eq!(serde_json::from_slice::<serde_json::Value>(response.body()).unwrap()["code"], 3);
        }
        assert!(
            transcoder
                .try_transcode("DELETE", "/unregistered", HeaderMap::new(), b"")
                .await
                .is_none()
        );
    });
}

#[test]
fn gamma_survivor_generated_query_scalar_shapes_and_alias_body_precedence() {
    block_on(async {
        let transcoder = Transcoder::new(InMemoryLibrary);
        let response = transcoder.transcode(
            "GET","/v1/shelves:search?include_archived=true&tags=a%2Bb&tags=c+d&genre=SCIENCE&threshold=1.25&options.enabled=true&numeric_text=001",
            HeaderMap::new(),b"",
        ).await;
        let TranscodeResponse::Unary(response) = response else {
            panic!("expected unary")
        };
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(response.body()).unwrap(),
            serde_json::json!({
                "includeArchived":true,"tags":["a+b","c d"],"genre":"SCIENCE","threshold":1.25,
                "options":{"enabled":true},"numericText":"001"
            })
        );
        let response = transcoder
            .transcode(
                "POST",
                "/v1/shelves/path-id:replace",
                HeaderMap::new(),
                br#"{"id":"body-id","shelf":{"name":"body-name","theme":"retained"},"force":true}"#,
            )
            .await;
        let TranscodeResponse::Unary(response) = response else {
            panic!("expected unary")
        };
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), b"1");
        let request = pb::SearchShelvesRequest {
            include_archived: true,
            tags: vec!["exact".into()],
            genre: 2,
            threshold: 3.25,
            options: Some(pb::SearchOptions { enabled: true }),
            numeric_text: "001".into(),
        };
        assert_eq!(
            InMemoryLibrary
                .search_shelves(request.clone(), &mut Context::default())
                .await
                .unwrap(),
            request
        );
    });
}
