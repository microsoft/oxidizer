// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated before/after/transform interceptors on one service.
//!
//! This example shows every interceptor kind working together on a single
//! `#[router]` service, all called directly with no boxed futures, services, or
//! per-request allocations:
//!
//! - a router-wide `#[before]` interceptor authenticates every request and
//!   enriches the typed request extensions, short-circuiting when the caller is
//!   anonymous;
//! - a per-handler `#[before]` interceptor guards one route through a
//!   [`SelectedContext`], whose split request-head borrow leaves the handler's
//!   zero-copy `&str` capture intact;
//! - a buffering `#[transform(limit = N, ...)]` interceptor replaces one
//!   handler's request body with a concrete body that its `#[body]` parameter
//!   then extracts;
//! - a streaming `#[transform(stream, ...)]` interceptor wraps another
//!   handler's transport body lazily, so a decompression- or metering-style
//!   middleware never buffers; and
//! - a bare `#[after]` interceptor stamps a header onto *every generated
//!   response*, including short-circuits and routing failures.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use pin_project_lite::pin_project;
use routerama::response::{Body, Response};
use routerama::route::{
    AfterContext, Before, BeforeContext, BodyTransform, BytesBody, ClonedExtension, Request, RequestParts, SelectedContext, StatusCode,
    router,
};

/// A fake administrator credential this example accepts.
const ADMIN_TOKEN: &str = "admin-caller";

/// A fake non-administrator credential.
const GUEST_TOKEN: &str = "guest-one";

/// A request-local identity inserted by the authentication interceptor.
#[derive(Clone, Copy)]
struct Caller {
    id: u32,
    admin: bool,
}

pin_project! {
    /// A streaming request-body wrapper that counts bytes as they pass, without
    /// ever collecting the body. It is the replacement body the streaming
    /// transform hands to `#[body]` extraction.
    struct Metered<B> {
        #[pin]
        inner: B,
        counter: Arc<AtomicUsize>,
    }
}

impl<B> http_body::Body for Metered<B>
where
    B: http_body::Body<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let polled = this.inner.poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(data) = frame.data_ref()
        {
            let _ = this.counter.fetch_add(data.len(), Ordering::Relaxed);
        }
        polled
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

struct Api {
    metered: Arc<AtomicUsize>,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::needless_pass_by_ref_mut,
    reason = "router handlers and interceptors must be async and take the macro-required &mut context; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Api {
    /// Any authenticated caller may greet.
    #[route(GET, "/hello")]
    async fn hello(&self, caller: ClonedExtension<Caller>) -> String {
        format!("hello, caller {}", caller.id)
    }

    /// Only administrators reach this handler; the per-handler `guard`
    /// interceptor short-circuits everyone else.
    #[route(GET, "/admin")]
    async fn admin(&self, caller: ClonedExtension<Caller>) -> String {
        format!("welcome, admin {}", caller.id)
    }

    /// The `normalize` transform trims and lowercases the body before this
    /// handler extracts it.
    #[route(POST, "/notes")]
    async fn create_note(&self, #[body] note: BytesBody<1024>) -> (StatusCode, String) {
        (
            StatusCode::CREATED,
            String::from_utf8(note.as_bytes().to_vec()).expect("normalized body is UTF-8"),
        )
    }

    /// `slug` is borrowed straight out of the request URI, and the `#[body]`
    /// parameter extracts from the streaming wrapper the `meter` transform
    /// returned, not from the raw transport body.
    #[route(POST, "/uploads/{slug}")]
    async fn upload(&self, slug: &str, #[body] payload: BytesBody<1024>) -> (StatusCode, String) {
        (StatusCode::ACCEPTED, format!("{slug}:{}", payload.as_bytes().len()))
    }

    /// Runs for every request before routing: authenticate and enrich, or
    /// short-circuit with `401`.
    #[before]
    async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let Some(token) = ctx.headers().get("authorization").and_then(|value| value.to_str().ok()) else {
            return Before::Respond(StatusCode::UNAUTHORIZED);
        };
        let admin = token == ADMIN_TOKEN;
        ctx.insert_extension(Caller {
            id: u32::try_from(token.len()).unwrap_or(u32::MAX),
            admin,
        });
        Before::Next
    }

    /// Runs only for the named handlers, after route selection and after
    /// `authenticate`. The split context reads the selected method and URI and
    /// mutates headers and extensions, so `upload` still receives its borrowed
    /// `slug` capture.
    #[before(admin, upload)]
    async fn guard(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        if ctx.uri().path().starts_with("/uploads/") {
            return match ctx.get_extension::<Caller>() {
                Some(_) => Before::Next,
                None => Before::Respond(StatusCode::UNAUTHORIZED),
            };
        }
        match ctx.get_extension::<Caller>() {
            Some(caller) if caller.admin => Before::Next,
            _ => Before::Respond(StatusCode::FORBIDDEN),
        }
    }

    /// Buffers and normalizes the `create_note` body, handing a concrete
    /// replacement body to `#[body]` extraction.
    #[transform(limit = 1024, create_note)]
    async fn normalize(&self, _parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        let text = core::str::from_utf8(&body).unwrap_or_default().trim().to_ascii_lowercase();
        BodyTransform::Replace(Body::from(text))
    }

    /// Wraps the `upload` transport body lazily. The macro substitutes the
    /// router's transport body for `B`, so `#[body]` above is checked against
    /// `Metered<TransportBody>`; nothing is buffered, boxed, or allocated.
    #[transform(stream, upload)]
    async fn meter<B>(&self, _parts: &RequestParts, body: B) -> BodyTransform<Metered<B>, StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        BodyTransform::Replace(Metered {
            inner: body,
            counter: Arc::clone(&self.metered),
        })
    }

    /// Runs on every response this router generates: handler responses,
    /// `#[before]`/`#[transform]` short-circuits, extractor rejections, and
    /// routing failures alike.
    #[after]
    async fn stamp(&self, ctx: &mut AfterContext<'_>) {
        ctx.headers_mut()
            .insert("x-served-by", "routerama".parse().expect("static header value is valid"));
    }
}

async fn read<B>(response: Response<B>) -> (StatusCode, Bytes)
where
    B: http_body::Body<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body succeeds").to_bytes();
    (status, bytes)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let api = Api {
        metered: Arc::new(AtomicUsize::new(0)),
    };

    // Anonymous request: the router-wide interceptor short-circuits with 401,
    // and the bare `#[after]` still observes that generated response.
    let anonymous = api
        .route(Request::get("/hello").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.headers()["x-served-by"], "routerama");

    // A routing failure is a generated response too.
    let missing = api
        .route(
            Request::get("/absent")
                .header("authorization", ADMIN_TOKEN)
                .body(Body::empty())
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.headers()["x-served-by"], "routerama");

    // Authenticated non-admin greeting; the after interceptor stamps a header.
    let hello = api
        .route(
            Request::get("/hello")
                .header("authorization", GUEST_TOKEN)
                .body(Body::empty())
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(hello.headers()["x-served-by"], "routerama");
    assert_eq!(read(hello).await, (StatusCode::OK, Bytes::from_static(b"hello, caller 9")));

    // Non-admin hitting the guarded route: the per-handler interceptor rejects.
    let denied = api
        .route(
            Request::get("/admin")
                .header("authorization", GUEST_TOKEN)
                .body(Body::empty())
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // Admin passes both interceptors.
    let admin = api
        .route(
            Request::get("/admin")
                .header("authorization", ADMIN_TOKEN)
                .body(Body::empty())
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(read(admin).await, (StatusCode::OK, Bytes::from_static(b"welcome, admin 12")));

    // The buffering transform normalizes the body before handler extraction.
    let note = api
        .route(
            Request::post("/notes")
                .header("authorization", ADMIN_TOKEN)
                .body(Body::from("  Buy MILK  "))
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(read(note).await, (StatusCode::CREATED, Bytes::from_static(b"buy milk")));

    // The streaming transform wraps the body: the guard kept the borrowed
    // `slug` capture alive, and the wrapper metered the bytes as the handler's
    // `#[body]` parameter pulled them through.
    let upload = api
        .route(
            Request::post("/uploads/report")
                .header("authorization", ADMIN_TOKEN)
                .body(Body::from("streamed payload"))
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(read(upload).await, (StatusCode::ACCEPTED, Bytes::from_static(b"report:16")));
    assert_eq!(api.metered.load(Ordering::Relaxed), 16);

    println!("all interceptor scenarios passed");
}
