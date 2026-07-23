// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unknown_lints, reason = "the pinned and latest Clippy versions expose different async-trait lints")]
#![expect(
    clippy::unused_async_trait_impl,
    reason = "synchronous examples implement generated async service traits"
)]

//! Writing a custom bridge for a non-`tonic` gRPC stack — here a stand-in for
//! [`volo`](https://github.com/cloudwego/volo)-grpc.
//!
//! `rest_over_grpc::build` emits a built-in bridge for `tonic` by default: a
//! blanket `impl Library for T where T: <tonic server trait>`, so a service
//! written once against `tonic` also serves REST. For any other stack you write
//! the bridge yourself — implement the generated
//! [`pb::Library`](rest_over_grpc_examples::custom::pb::Library) trait for your service,
//! delegating each RPC to your framework's handler and converting its
//! request/response/status types.
//!
//! This example bridges a `volo`-style service. `volo` isn't a dependency here,
//! so [`volo_gen`] is a small stand-in for the server trait and wrapper types
//! `volo-grpc` would generate. Two caveats about what a real bridge would do
//! differently:
//!
//! * **Blanket vs. concrete.** A real bridge belongs in the crate that defines
//!   `pb::Library` (alongside the generated code), where the orphan rule lets it
//!   be a blanket `impl<T> Library for T where T: volo_gen::LibraryServer` —
//!   exactly like the built-in `tonic` bridge. Here the bridge lives in a
//!   downstream example, so it must be a *concrete* `impl Library for
//!   MyShelfService` (a foreign trait on a local type) to satisfy coherence.
//! * **Message types.** `volo` generates its own (`pilota`) message types; a
//!   real bridge would convert those to the `prost` types this crate uses. For
//!   focus, this stand-in reuses the `prost` messages directly.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example volo_bridge
//! ```

use rest_over_grpc::handling::{Code, Context, ResponseStream, Status};
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_examples::custom::pb::{
    self, CreateShelfRequest, Genre, GetShelfRequest, ListShelvesByGenreRequest, ListShelvesRequest, ListShelvesResponse, Shelf,
};

/// A stand-in for the server trait and wrapper types `volo-grpc` generates.
mod volo_gen {
    use futures_util::Stream;

    use super::{CreateShelfRequest, GetShelfRequest, ListShelvesRequest, ListShelvesResponse, Shelf};

    /// Mirrors `volo_grpc::Request`.
    pub(crate) struct Request<T> {
        message: T,
    }

    impl<T> Request<T> {
        pub(crate) fn new(message: T) -> Self {
            Self { message }
        }

        pub(crate) fn into_inner(self) -> T {
            self.message
        }
    }

    /// Mirrors `volo_grpc::Response`.
    pub(crate) struct Response<T> {
        message: T,
    }

    impl<T> Response<T> {
        pub(crate) fn new(message: T) -> Self {
            Self { message }
        }

        pub(crate) fn into_inner(self) -> T {
            self.message
        }
    }

    /// A subset of the gRPC status codes, mirroring `volo_grpc::Code`.
    #[derive(Clone, Copy)]
    pub(crate) enum Code {
        NotFound,
        InvalidArgument,
    }

    /// Mirrors `volo_grpc::Status`.
    pub(crate) struct Status {
        pub(crate) code: Code,
        pub(crate) message: String,
    }

    impl Status {
        pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
            Self {
                code,
                message: message.into(),
            }
        }
    }

    /// Mirrors the server trait `volo-grpc` generates from the `.proto` service.
    ///
    /// Streaming methods use an associated stream type (as `tonic` and `volo`
    /// both do), and every method returns a `Send` future.
    pub(crate) trait LibraryServer: Send + Sync + 'static {
        fn get_shelf(&self, request: Request<GetShelfRequest>) -> impl Future<Output = Result<Response<Shelf>, Status>> + Send;

        fn create_shelf(&self, request: Request<CreateShelfRequest>) -> impl Future<Output = Result<Response<Shelf>, Status>> + Send;

        fn list_shelves(
            &self,
            request: Request<ListShelvesRequest>,
        ) -> impl Future<Output = Result<Response<ListShelvesResponse>, Status>> + Send;

        type StreamShelvesStream: Stream<Item = Result<Shelf, Status>> + Send;

        fn stream_shelves(
            &self,
            request: Request<ListShelvesRequest>,
        ) -> impl Future<Output = Result<Response<Self::StreamShelvesStream>, Status>> + Send;
    }
}

/// A service written once against `volo`'s server trait.
#[derive(Clone, Copy)]
struct MyShelfService;

impl volo_gen::LibraryServer for MyShelfService {
    async fn get_shelf(&self, request: volo_gen::Request<GetShelfRequest>) -> Result<volo_gen::Response<Shelf>, volo_gen::Status> {
        let request = request.into_inner();
        if request.shelf == "missing" {
            return Err(volo_gen::Status::new(volo_gen::Code::NotFound, "no such shelf"));
        }
        Ok(volo_gen::Response::new(Shelf {
            name: format!("shelves/{}", request.shelf),
            theme: "history".to_owned(),
        }))
    }

    async fn create_shelf(&self, request: volo_gen::Request<CreateShelfRequest>) -> Result<volo_gen::Response<Shelf>, volo_gen::Status> {
        let mut created = request
            .into_inner()
            .shelf
            .ok_or_else(|| volo_gen::Status::new(volo_gen::Code::InvalidArgument, "shelf is required"))?;
        "shelves/created".clone_into(&mut created.name);
        Ok(volo_gen::Response::new(created))
    }

    async fn list_shelves(
        &self,
        request: volo_gen::Request<ListShelvesRequest>,
    ) -> Result<volo_gen::Response<ListShelvesResponse>, volo_gen::Status> {
        let filter = request.into_inner().filter;
        let shelves = sample_shelves()
            .into_iter()
            .filter(|s| filter.is_empty() || s.theme == filter)
            .collect();
        Ok(volo_gen::Response::new(ListShelvesResponse { shelves }))
    }

    type StreamShelvesStream = futures_util::stream::Iter<std::vec::IntoIter<Result<Shelf, volo_gen::Status>>>;

    async fn stream_shelves(
        &self,
        request: volo_gen::Request<ListShelvesRequest>,
    ) -> Result<volo_gen::Response<Self::StreamShelvesStream>, volo_gen::Status> {
        let filter = request.into_inner().filter;
        let shelves: Vec<Result<Shelf, volo_gen::Status>> = sample_shelves()
            .into_iter()
            .filter(|s| filter.is_empty() || s.theme == filter)
            .map(Ok)
            .collect();
        Ok(volo_gen::Response::new(futures_util::stream::iter(shelves)))
    }
}

fn sample_shelves() -> Vec<Shelf> {
    vec![
        Shelf {
            name: "shelves/1".to_owned(),
            theme: "history".to_owned(),
        },
        Shelf {
            name: "shelves/2".to_owned(),
            theme: "science".to_owned(),
        },
    ]
}

/// Converts a `volo` status to a [`rest_over_grpc::handling::Status`], so this bridge
/// never leaks the foreign status type into the generated trait.
fn convert_status(status: volo_gen::Status) -> Status {
    let code = match status.code {
        volo_gen::Code::NotFound => Code::NotFound,
        volo_gen::Code::InvalidArgument => Code::InvalidArgument,
    };
    Status::new(code, status.message)
}

/// The hand-written bridge: each generated RPC forwards to the `volo` handler,
/// wrapping the request, unwrapping the response, and mapping the status. This
/// mirrors what the built-in `tonic` bridge emits, but as a concrete `impl`.
///
/// Each method also receives a `&mut Context`; a production bridge would seed
/// the framework request's metadata from `cx.request_headers()` and copy the
/// response metadata back via `cx.merge_response_headers(...)`, exactly as the
/// `tonic` bridge does. This stand-in's `volo_gen` types carry no metadata, so
/// the context is unused here.
impl pb::Library for MyShelfService {
    async fn get_shelf(&self, request: GetShelfRequest, _cx: &mut Context) -> Result<Shelf, Status> {
        <Self as volo_gen::LibraryServer>::get_shelf(self, volo_gen::Request::new(request))
            .await
            .map(volo_gen::Response::into_inner)
            .map_err(convert_status)
    }

    async fn create_shelf(&self, request: CreateShelfRequest, _cx: &mut Context) -> Result<Shelf, Status> {
        <Self as volo_gen::LibraryServer>::create_shelf(self, volo_gen::Request::new(request))
            .await
            .map(volo_gen::Response::into_inner)
            .map_err(convert_status)
    }

    async fn list_shelves(&self, request: ListShelvesRequest, _cx: &mut Context) -> Result<ListShelvesResponse, Status> {
        <Self as volo_gen::LibraryServer>::list_shelves(self, volo_gen::Request::new(request))
            .await
            .map(volo_gen::Response::into_inner)
            .map_err(convert_status)
    }

    async fn list_shelves_by_genre(&self, request: ListShelvesByGenreRequest, _cx: &mut Context) -> Result<ListShelvesResponse, Status> {
        // No `volo` counterpart: this handler filters the sample shelves by the
        // decoded enum value directly (the `{genre}` path variable arrives as its
        // `i32`, from either the value name or its number).
        let theme = match request.genre() {
            Genre::History => "history",
            Genre::Science => "science",
            Genre::Unspecified => return Err(Status::invalid_argument("genre is required")),
        };
        let shelves = sample_shelves().into_iter().filter(|s| s.theme == theme).collect();
        Ok(ListShelvesResponse { shelves })
    }

    async fn stream_shelves(&self, request: ListShelvesRequest, _cx: &mut Context) -> Result<ResponseStream<Shelf>, Status> {
        // Both a `volo` streaming method and the generated trait method have the
        // same two-phase shape: `async fn -> Result<Stream, Status>`. Await
        // initiation, map the initiation error, then box and error-map the
        // response stream (`volo`'s stream is `Send + 'static`), so it streams to
        // the wire after transcode returns.
        let stream = <Self as volo_gen::LibraryServer>::stream_shelves(self, volo_gen::Request::new(request))
            .await
            .map(volo_gen::Response::into_inner)
            .map_err(convert_status)?;
        Ok(Box::pin(rest_over_grpc::codegen_helpers::map_stream_status(stream, convert_status)))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let service = rest_over_grpc_examples::custom::Transcoder::new(MyShelfService);
    let requests = [
        ("GET", "/v1/shelves/history"), // unary
        ("GET", "/v1/shelves"),         // unary list
        ("GET", "/v1/shelves:stream"),  // server-streaming
    ];

    for (method, target) in requests {
        let response = service.transcode(method, target, http::HeaderMap::new(), b"").await;
        match response {
            TranscodeResponse::Unary(http) => {
                println!(
                    "{method} {target} -> {} {}",
                    http.status().as_u16(),
                    String::from_utf8_lossy(http.body())
                );
            }
            TranscodeResponse::Streaming(stream) => {
                use futures_util::StreamExt as _;
                let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
                println!("{method} {target} -> streaming {}", String::from_utf8_lossy(&frames.concat()));
            }
        }
    }
}
