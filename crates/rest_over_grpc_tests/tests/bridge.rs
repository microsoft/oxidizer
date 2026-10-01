// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Proves the `rest_over_grpc::build` `tonic` bridge works end to end: a service
//! implemented only against tonic's generated `greeter_server::Greeter` trait is,
//! via the generated guarded adapter, also a `rest_over_grpc` service whose
//! `transcode` transcodes REST/JSON requests.

use futures::StreamExt as _;
use futures::executor::block_on;
use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
use rest_over_grpc_tests::tonic_bridge::greeter::__rest_over_grpc_bridge_Greeter::GreeterRestBridge;
use rest_over_grpc_tests::tonic_bridge::{GreeterService, Transcoder};

// Both the protobuf message and the generated adapter must remain addressable.
const _: Option<rest_over_grpc_tests::tonic_bridge::greeter::GreeterRestBridge> = None;

fn authenticated(name: &tonic::metadata::MetadataMap) -> Result<(), tonic::Status> {
    if name.get("authorization").is_some_and(|value| value == "Bearer trusted") {
        Ok(())
    } else {
        Err(tonic::Status::unauthenticated("REST authorization required"))
    }
}

fn authorized_headers() -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, http::HeaderValue::from_static("Bearer trusted"));
    headers
}

/// Unwraps a unary [`TranscodeResponse`] into its buffered [`HttpResponse`].
fn unary(response: TranscodeResponse) -> HttpResponse {
    match response {
        TranscodeResponse::Unary(http) => Some(http),
        TranscodeResponse::Streaming(_) => None,
    }
    .expect("expected a unary response")
}

/// Collects a streaming [`TranscodeResponse`]'s frames, returning its content
/// type and concatenated body bytes.
fn streaming(response: TranscodeResponse) -> (String, Vec<u8>) {
    let stream = match response {
        TranscodeResponse::Streaming(stream) => Some(stream),
        TranscodeResponse::Unary(_) => None,
    }
    .expect("expected a streaming response");
    let content_type = stream.content_type().to_str().expect("generated content type is ASCII").to_owned();
    let body = block_on(async {
        let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
        frames.concat()
    });
    (content_type, body)
}

#[test]
fn tonic_bridge_transcodes_unary() {
    let response = unary(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/greet/World",
            authorized_headers(),
            b"",
        ),
    ));
    assert_eq!(response.status(), http::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(response.body()).expect("valid JSON body");
    assert_eq!(body["message"], "Hello, World!");
}

#[test]
fn tonic_bridge_debug_output_is_redacted() {
    let bridge = GreeterRestBridge::with_guard(GreeterService, authenticated);
    assert_eq!(format!("{bridge:?}"), "GreeterRestBridge { .. }");
}

#[test]
fn tonic_bridge_maps_handler_status_to_not_found() {
    let response = unary(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/greet/missing",
            authorized_headers(),
            b"",
        ),
    ));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(response.body()).expect("valid JSON body");
    assert_eq!(body["message"], "no greeting for that name");
}

#[test]
fn tonic_bridge_maps_status_to_not_found() {
    let response = unary(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/nope",
            authorized_headers(),
            b"",
        ),
    ));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

#[test]
fn tonic_bridge_transcodes_server_streaming_as_json_array() {
    let (content_type, body) = streaming(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/greet/World:stream",
            authorized_headers(),
            b"",
        ),
    ));
    assert_eq!(content_type, "application/json");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON array body");
    assert_eq!(body[0]["message"], "Hello, World!");
    assert_eq!(body[1]["message"], "Bye, World!");
}

#[test]
fn tonic_bridge_negotiates_ndjson_for_streaming() {
    let mut headers = authorized_headers();
    let _ = headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("application/x-ndjson"));
    let (content_type, body) = streaming(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/greet/World:stream",
            headers,
            b"",
        ),
    ));
    assert_eq!(content_type, "application/x-ndjson");
    let text = String::from_utf8(body).expect("utf8 body");
    assert_eq!(text, "{\"message\":\"Hello, World!\"}\n{\"message\":\"Bye, World!\"}\n");
}

#[test]
fn tonic_bridge_considers_every_accept_header_line_by_quality() {
    let mut headers = authorized_headers();
    headers.append(http::header::ACCEPT, http::HeaderValue::from_static("application/json;q=0.2"));
    headers.append(http::header::ACCEPT, http::HeaderValue::from_static("text/event-stream;q=0.9"));
    let (content_type, body) = streaming(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode(
            "GET",
            "/v1/greet/World:stream",
            headers,
            b"",
        ),
    ));
    assert_eq!(content_type, "text/event-stream");
    assert!(String::from_utf8(body).unwrap().contains("data: {"));
}

#[test]
fn tonic_bridge_forwards_streaming_response_metadata() {
    use std::pin::Pin;

    use futures::stream::{self, Stream};
    use rest_over_grpc_tests::tonic_bridge::greeter::{HelloReply, HelloRequest, greeter_server};

    #[derive(Clone, Copy)]
    struct MetadataGreeter;

    #[tonic::async_trait]
    impl greeter_server::Greeter for MetadataGreeter {
        async fn say_hello(&self, _request: tonic::Request<HelloRequest>) -> Result<tonic::Response<HelloReply>, tonic::Status> {
            Err(tonic::Status::unimplemented("unary is not exercised by this test"))
        }

        type StreamGreetingsStream = Pin<Box<dyn Stream<Item = Result<HelloReply, tonic::Status>> + Send + 'static>>;

        async fn stream_greetings(
            &self,
            _request: tonic::Request<HelloRequest>,
        ) -> Result<tonic::Response<Self::StreamGreetingsStream>, tonic::Status> {
            let items = stream::iter(vec![Ok(HelloReply { message: "hi".to_owned() })]);
            let mut response = tonic::Response::new(Box::pin(items) as Self::StreamGreetingsStream);
            let _ = response
                .metadata_mut()
                .insert("x-greeting-source", tonic::metadata::MetadataValue::from_static("stream"));
            Ok(response)
        }
    }

    let response = block_on(
        Transcoder::new(GreeterRestBridge::with_guard(MetadataGreeter, authenticated)).transcode(
            "GET",
            "/v1/greet/World:stream",
            authorized_headers(),
            b"",
        ),
    );
    let stream = match response {
        TranscodeResponse::Streaming(stream) => stream,
        TranscodeResponse::Unary(_) => panic!("expected a streaming response"),
    };
    assert_eq!(
        stream
            .headers()
            .get("x-greeting-source")
            .expect("initial response metadata is forwarded"),
        "stream"
    );
}

#[test]
fn tonic_bridge_rejects_unauthenticated_unary_and_stream_before_invocation() {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream::Stream;
    use rest_over_grpc_tests::tonic_bridge::greeter::{HelloReply, HelloRequest, greeter_server};

    struct Counted(Arc<AtomicUsize>);
    #[tonic::async_trait]
    impl greeter_server::Greeter for Counted {
        async fn say_hello(&self, request: tonic::Request<HelloRequest>) -> Result<tonic::Response<HelloReply>, tonic::Status> {
            assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer trusted");
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(tonic::Response::new(HelloReply::default()))
        }
        type StreamGreetingsStream = Pin<Box<dyn Stream<Item = Result<HelloReply, tonic::Status>> + Send + 'static>>;
        async fn stream_greetings(
            &self,
            request: tonic::Request<HelloRequest>,
        ) -> Result<tonic::Response<Self::StreamGreetingsStream>, tonic::Status> {
            assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer trusted");
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(tonic::Response::new(Box::pin(futures::stream::empty())))
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = GreeterRestBridge::with_guard(Counted(Arc::clone(&calls)), authenticated);
    let transcoder = Transcoder::new(bridge);
    for path in ["/v1/greet/World", "/v1/greet/World:stream"] {
        let response = unary(block_on(transcoder.transcode("GET", path, http::HeaderMap::new(), b"")));
        assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let response = unary(block_on(transcoder.transcode("GET", "/v1/greet/World", authorized_headers(), b"")));
    assert_eq!(response.status(), http::StatusCode::OK);
    let _ = streaming(block_on(transcoder.transcode(
        "GET",
        "/v1/greet/World:stream",
        authorized_headers(),
        b"",
    )));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[cfg_attr(miri, ignore)] // Miri isolation does not permit the test's TCP listener.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end authorization parity scenario across both transports"
)]
async fn protected_bridge_matches_tonic_transport_identity_and_permission_checks() {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream::{self, Stream};
    use rest_over_grpc_tests::tonic_bridge::greeter::{HelloReply, HelloRequest, greeter_server};
    use tokio_stream::wrappers::TcpListenerStream;

    const READER: &str = "Bearer reader-1";
    const GUEST: &str = "Bearer guest-1";

    fn greeting_policy(metadata: &tonic::metadata::MetadataMap) -> Result<(), tonic::Status> {
        match metadata.get("authorization").and_then(|value| value.to_str().ok()) {
            Some(READER) => Ok(()),
            Some(GUEST) => Err(tonic::Status::permission_denied("greeting read permission required")),
            _ => Err(tonic::Status::unauthenticated("recognized identity required")),
        }
    }

    #[derive(Clone)]
    struct ProtectedGreeter(Arc<AtomicUsize>);

    #[tonic::async_trait]
    impl greeter_server::Greeter for ProtectedGreeter {
        async fn say_hello(&self, request: tonic::Request<HelloRequest>) -> Result<tonic::Response<HelloReply>, tonic::Status> {
            assert_eq!(request.metadata().get("authorization").unwrap(), READER);
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(tonic::Response::new(HelloReply {
                message: format!("Hello, {}!", request.into_inner().name),
            }))
        }

        type StreamGreetingsStream = Pin<Box<dyn Stream<Item = Result<HelloReply, tonic::Status>> + Send + 'static>>;

        async fn stream_greetings(
            &self,
            request: tonic::Request<HelloRequest>,
        ) -> Result<tonic::Response<Self::StreamGreetingsStream>, tonic::Status> {
            assert_eq!(request.metadata().get("authorization").unwrap(), READER);
            self.0.fetch_add(1, Ordering::Relaxed);
            let reply = HelloReply {
                message: format!("Hello, {}!", request.into_inner().name),
            };
            Ok(tonic::Response::new(Box::pin(stream::iter([Ok(reply)]))))
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let service = ProtectedGreeter(Arc::clone(&calls));
    let transcoder = Transcoder::new(GreeterRestBridge::with_guard(service.clone(), greeting_policy));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(greeter_server::GreeterServer::with_interceptor(
                service,
                |request: tonic::Request<()>| {
                    greeting_policy(request.metadata())?;
                    Ok(request)
                },
            ))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut grpc = tonic::client::Grpc::new(channel);

    for (credential, rest_status, grpc_code) in [
        (None, http::StatusCode::UNAUTHORIZED, tonic::Code::Unauthenticated),
        (Some(GUEST), http::StatusCode::FORBIDDEN, tonic::Code::PermissionDenied),
    ] {
        let mut headers = http::HeaderMap::new();
        if let Some(credential) = credential {
            headers.insert(http::header::AUTHORIZATION, http::HeaderValue::from_static(credential));
        }
        for (path, rpc) in [
            ("/v1/greet/World", "/greeter.Greeter/SayHello"),
            ("/v1/greet/World:stream", "/greeter.Greeter/StreamGreetings"),
        ] {
            let response = unary(transcoder.transcode("GET", path, headers.clone(), b"").await);
            assert_eq!(response.status(), rest_status);
            let mut request = tonic::Request::new(HelloRequest { name: "World".to_owned() });
            if let Some(credential) = credential {
                request.metadata_mut().insert("authorization", credential.parse().unwrap());
            }
            let rpc = http::uri::PathAndQuery::from_static(rpc);
            grpc.ready().await.unwrap();
            let error = if path.ends_with(":stream") {
                grpc.server_streaming::<HelloRequest, HelloReply, _>(request, rpc, tonic_prost::ProstCodec::default())
                    .await
                    .err()
                    .unwrap()
            } else {
                grpc.unary::<HelloRequest, HelloReply, _>(request, rpc, tonic_prost::ProstCodec::default())
                    .await
                    .err()
                    .unwrap()
            };
            assert_eq!(error.code(), grpc_code);
        }
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, http::HeaderValue::from_static(READER));
    let response = unary(transcoder.transcode("GET", "/v1/greet/World", headers.clone(), b"").await);
    assert_eq!(response.status(), http::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert_eq!(body["message"], "Hello, World!");
    let response = transcoder.transcode("GET", "/v1/greet/World:stream", headers, b"").await;
    let TranscodeResponse::Streaming(stream) = response else {
        panic!("expected authorized REST stream");
    };
    let frames: Vec<_> = stream.into_frames().collect().await;
    assert!(frames.iter().all(Result::is_ok));
    let body = frames.into_iter().map(Result::unwrap).collect::<Vec<_>>().concat();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body[0]["message"], "Hello, World!");
    assert_eq!(calls.load(Ordering::Relaxed), 2);

    let mut unary_request = tonic::Request::new(HelloRequest { name: "World".to_owned() });
    unary_request.metadata_mut().insert("authorization", READER.parse().unwrap());
    grpc.ready().await.unwrap();
    let response: tonic::Response<HelloReply> = grpc
        .unary(
            unary_request,
            http::uri::PathAndQuery::from_static("/greeter.Greeter/SayHello"),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .unwrap();
    assert_eq!(response.into_inner().message, "Hello, World!");
    let mut stream_request = tonic::Request::new(HelloRequest { name: "World".to_owned() });
    stream_request.metadata_mut().insert("authorization", READER.parse().unwrap());
    grpc.ready().await.unwrap();
    let response: tonic::Response<tonic::Streaming<HelloReply>> = grpc
        .server_streaming(
            stream_request,
            http::uri::PathAndQuery::from_static("/greeter.Greeter/StreamGreetings"),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .unwrap();
    let mut stream = response.into_inner();
    assert_eq!(stream.message().await.unwrap().unwrap().message, "Hello, World!");
    assert!(stream.message().await.unwrap().is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    drop(stream);
    drop(grpc);
    shutdown_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn tonic_bridge_explicitly_acknowledges_external_authentication() {
    let response = unary(block_on(
        Transcoder::new(GreeterRestBridge::externally_authenticated(GreeterService)).transcode(
            "GET",
            "/v1/greet/World",
            http::HeaderMap::new(),
            b"",
        ),
    ));
    assert_eq!(response.status(), http::StatusCode::OK);
}

#[test]
fn generated_bridge_rejects_oversized_query_before_decoding() {
    let target = format!("/v1/greet/World?{}", vec!["name=World"; 129].join("&"));
    let response = unary(block_on(
        Transcoder::new(GreeterRestBridge::with_guard(GreeterService, authenticated)).transcode("GET", &target, authorized_headers(), b""),
    ));
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    assert!(body["message"].as_str().unwrap().contains("query exceeds"), "{body}");
}

#[test]
fn tonic_bridge_preserves_error_details_and_metadata_at_each_stage() {
    use std::pin::Pin;

    use futures::stream::{self, Stream};
    use http::{HeaderMap, HeaderValue};
    use rest_over_grpc_tests::tonic_bridge::greeter::{HelloReply, HelloRequest, greeter_server};

    fn failure() -> tonic::Status {
        let mut status = tonic::Status::with_details(tonic::Code::InvalidArgument, "bad greeting", bytes::Bytes::from_static(b"\x00\xff"));
        let mut headers = HeaderMap::new();
        headers.insert("x-reason", HeaderValue::from_static("invalid"));
        headers.insert("x-opaque", HeaderValue::from_bytes(b"\x80").unwrap());
        headers.insert("trace-bin", HeaderValue::from_static("Af4"));
        headers.insert("broken-bin", HeaderValue::from_static("{}.."));
        *status.metadata_mut() = tonic::metadata::MetadataMap::from_headers(headers);
        status
    }

    struct Failing;
    #[tonic::async_trait]
    impl greeter_server::Greeter for Failing {
        async fn say_hello(&self, _: tonic::Request<HelloRequest>) -> Result<tonic::Response<HelloReply>, tonic::Status> {
            Err(failure())
        }
        type StreamGreetingsStream = Pin<Box<dyn Stream<Item = Result<HelloReply, tonic::Status>> + Send + 'static>>;
        async fn stream_greetings(
            &self,
            request: tonic::Request<HelloRequest>,
        ) -> Result<tonic::Response<Self::StreamGreetingsStream>, tonic::Status> {
            if request.into_inner().name == "init" {
                return Err(failure());
            }
            Ok(tonic::Response::new(Box::pin(stream::iter(vec![Err(failure())]))))
        }
    }

    let transcoder = Transcoder::new(GreeterRestBridge::with_guard(Failing, authenticated));
    for path in ["/v1/greet/World", "/v1/greet/init:stream"] {
        let response = unary(block_on(transcoder.transcode("GET", path, authorized_headers(), b"")));
        assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
        assert_eq!(response.headers().get("x-reason").unwrap(), "invalid");
        let body: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(
            body["details"][0],
            serde_json::json!({
                "kind": "grpc-status-details-bin",
                "encoding": "base64",
                "value": "AP8="
            })
        );
        assert_eq!(body["details"].as_array().unwrap().len(), 1);
    }
    let response = block_on(transcoder.transcode("GET", "/v1/greet/World:stream", authorized_headers(), b""));
    let TranscodeResponse::Streaming(stream) = response else {
        panic!("expected stream");
    };
    assert!(
        stream.headers().get("x-reason").is_none(),
        "item metadata cannot alter committed headers"
    );
    let frames: Vec<_> = block_on(stream.into_frames().collect());
    let error = frames[0].as_ref().unwrap_err();
    assert_eq!(error.code(), rest_over_grpc::handling::Code::InvalidArgument);
    assert_eq!(
        error.details()[0],
        serde_json::json!({
            "kind": "grpc-status-details-bin",
            "encoding": "base64",
            "value": "AP8="
        })
    );
    assert!(error.details().contains(&serde_json::json!({
        "kind": "grpc-metadata",
        "name": "x-reason",
        "value": "invalid"
    })));
    assert!(error.details().contains(&serde_json::json!({
        "kind": "grpc-metadata",
        "name": "trace-bin",
        "encoding": "base64",
        "value": "Af4="
    })));
    assert!(error.details().contains(&serde_json::json!({
        "kind": "grpc-metadata",
        "name": "x-opaque",
        "encoding": "base64",
        "representation": "encoded-header",
        "value": "gA=="
    })));
    assert!(error.details().contains(&serde_json::json!({
        "kind": "grpc-metadata",
        "name": "broken-bin",
        "encoding": "base64",
        "representation": "encoded-header",
        "value": "e30uLg=="
    })));
}
