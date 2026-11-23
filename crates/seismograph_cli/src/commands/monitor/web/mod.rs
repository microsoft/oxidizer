// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loopback HTTP and gRPC server for the browser monitor.

mod report;
mod service;

#[allow(
    dead_code,
    unreachable_pub,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::restriction,
    reason = "generated protocol code"
)]
mod seismograph {
    pub(super) mod monitor {
        pub(in super::super) mod v1 {
            include!(concat!(env!("OUT_DIR"), "/seismograph.monitor.v1.rs"));
            include!(concat!(env!("OUT_DIR"), "/seismograph.monitor.v1.serde.rs"));
            include!(concat!(env!("OUT_DIR"), "/seismograph.monitor.v1.rest.rs"));
        }
    }
}

use self::seismograph::monitor::v1 as api;

#[allow(
    unreachable_pub,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::restriction,
    reason = "generated REST routes"
)]
mod transcoder {
    use super::seismograph;
    include!(concat!(env!("OUT_DIR"), "/transcoder.rest.rs"));
}

#[cfg(test)]
mod tests;

use std::convert::Infallible;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use arty::runtime::Runtime;
use arty::runtime::policy::WorkersPolicy;
use arty_tokio::TokioCompatExt as _;
use bytes::Bytes;
use clap::Args as ClapArgs;
use http::{Request, Response, StatusCode, Version, header};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rest_over_grpc::serving::{DEFAULT_MAX_BODY_BYTES, RestBody, serve_http_with_max_body_bytes};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tower_service::Service as _;

use self::api::__rest_over_grpc_bridge_Monitor::MonitorRestBridge;
use self::api::monitor_server::MonitorServer;
use self::service::{MAX_SNAPSHOT_BYTES, Monitor};
use self::transcoder::Transcoder;

/// Arguments for the browser monitor.
#[derive(Debug, ClapArgs)]
pub(crate) struct Args {
    /// Local port. Use 0 to let the operating system choose a free port.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Open the default browser and sign in.
    #[arg(long)]
    open: bool,
}

pub(crate) fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let tokio = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .tokio_compat_with(tokio.handle())
        .build()?;
    let result = runtime.scheduler().block_on(async |_| serve(args).await);
    let shutdown = runtime.stop();
    tokio.shutdown_timeout(Duration::from_secs(1));
    result??;
    shutdown?;
    Ok(())
}

async fn serve(args: &Args) -> io::Result<()> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, args.port)).await?;
    let authority = listener.local_addr()?.to_string();
    let token = InstanceToken::generate()?;
    let server = Arc::new(Server::new(&authority, &token.0));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    // Register the Ctrl+C handler before advertising that the server is ready.
    tokio::select! {
        biased;
        result = &mut shutdown => return result,
        () = std::future::ready(()) => {}
    }
    let open_browser = |url: &str| open::that_detached(url);
    advertise(&mut io::stdout().lock(), &server.origin, &token, args.open.then_some(open_browser))?;
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            result = &mut shutdown => return result,
            _ = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept(), if connections.len() < 64 => {
                let (stream, _) = accepted?;
                let server = Arc::clone(&server);
                connections.spawn(async move {
                    let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                    builder.http1().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(Duration::from_secs(10));
                    builder.http2().max_concurrent_streams(16);
                    let handler = hyper::service::service_fn(move |request| {
                        let server = Arc::clone(&server);
                        async move { Ok::<_, Infallible>(server.handle(request).await) }
                    });
                    let _peer_closed = builder.serve_connection(TokioIo::new(stream), handler).await;
                });
            }
        }
    }
}

fn advertise<F>(output: &mut impl Write, origin: &str, token: &InstanceToken, open_browser: Option<F>) -> io::Result<()>
where
    F: FnOnce(&str) -> io::Result<()>,
{
    writeln!(output, "Open {origin}/")?;
    if let Some(open_browser) = open_browser {
        output.flush()?;
        // A fragment passes the key to the page without including it in an HTTP request.
        if open_browser(&format!("{origin}/#{}", token.0)).is_ok() {
            return Ok(());
        }
        writeln!(output, "Could not open the browser. Open the address above.")?;
    }
    writeln!(output, "Access key: {}", token.0)
}

struct InstanceToken(String);

impl InstanceToken {
    fn generate() -> io::Result<Self> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self(hex(&bytes)))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

struct Server {
    authority: String,
    origin: String,
    authorization: String,
    monitor: Monitor,
    grpc: MonitorServer<Monitor>,
    rest: Transcoder<MonitorRestBridge<Monitor, AuthenticationGuard>>,
    // Only one request at a time can buffer a large upload.
    uploads: Semaphore,
    downloads: Arc<Semaphore>,
}

type AuthenticationGuard = fn(&tonic::metadata::MetadataMap) -> Result<(), tonic::Status>;

#[routerama::resolver]
enum Asset {
    #[route(GET, "/")]
    Index,
    #[route(GET, "/monitor.js")]
    Script,
    #[route(GET, "/monitor.css")]
    Style,
}

#[routerama::resolver]
enum NativeFile<'p> {
    #[route(POST, "/api/v1/snapshots/native")]
    Open,
    #[route(GET, "/api/v1/snapshots/{id}/native")]
    Download { id: &'p str },
    #[route(GET, "/api/v1/snapshots/{id}/file")]
    JsonDownload { id: &'p str },
}

type Body = UnsyncBoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

impl Server {
    fn new(authority: &str, token: &str) -> Self {
        let monitor = Monitor::default();
        Self {
            authority: authority.to_owned(),
            origin: format!("http://{authority}"),
            authorization: format!("Bearer {token}"),
            monitor: monitor.clone(),
            grpc: MonitorServer::new(monitor.clone())
                .max_decoding_message_size(MAX_SNAPSHOT_BYTES + 1024)
                .max_encoding_message_size(MAX_SNAPSHOT_BYTES + 1024),
            rest: Transcoder::new(MonitorRestBridge::externally_authenticated(monitor)),
            uploads: Semaphore::new(1),
            downloads: Arc::new(Semaphore::new(1)),
        }
    }

    async fn handle(&self, request: Request<Incoming>) -> Response<Body> {
        let mut response = self.route(request).await;
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff"));
        headers.insert(header::REFERRER_POLICY, header::HeaderValue::from_static("no-referrer"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(
                "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
            ),
        );
        response
    }

    async fn route(&self, request: Request<Incoming>) -> Response<Body> {
        let headers = request.headers();
        let authority = request.uri().authority().map_or_else(
            || headers.get(header::HOST).and_then(|host| host.to_str().ok()),
            |authority| Some(authority.as_str()),
        );
        if authority != Some(&self.authority) || headers.get(header::ORIGIN).is_some_and(|origin| origin != self.origin.as_str()) {
            return text(StatusCode::FORBIDDEN, "Request origin is not allowed.");
        }
        let is_grpc = request.version() == Version::HTTP_2 && request.uri().path().starts_with("/seismograph.monitor.v1.Monitor/");
        let is_rest = request.uri().path().starts_with("/api/v1/");
        if is_grpc || is_rest {
            if headers
                .get(header::AUTHORIZATION)
                .is_none_or(|value| value != self.authorization.as_str())
            {
                return text(StatusCode::UNAUTHORIZED, "Use the access key from seismograph monitor-web.");
            }
            let path = request.uri().path();
            let native = NativeFile::resolver().resolve(request.method().as_str(), path);
            let upload = matches!(native, Ok(NativeFile::Open))
                || (is_rest && request.method() == http::Method::POST && path == "/api/v1/snapshots")
                || (is_grpc && path == "/seismograph.monitor.v1.Monitor/Open");
            let _slot = if upload {
                let Ok(slot) = self.uploads.try_acquire() else {
                    return text(StatusCode::TOO_MANY_REQUESTS, "A snapshot upload is already in progress.");
                };
                Some(slot)
            } else {
                None
            };
            let download = matches!(native, Ok(NativeFile::Download { .. } | NativeFile::JsonDownload { .. }))
                || (is_grpc && path == "/seismograph.monitor.v1.Monitor/Download");
            let transfer = if download {
                let Ok(permit) = Arc::clone(&self.downloads).try_acquire_owned() else {
                    return text(StatusCode::TOO_MANY_REQUESTS, "A snapshot download is already in progress.");
                };
                Some(permit)
            } else {
                None
            };
            let mut result = if is_grpc {
                let mut grpc = self.grpc.clone();
                match grpc.call(request).await {
                    Ok(response) => response.map(|body| body.map_err(Into::into).boxed_unsync()),
                    Err(never) => match never {},
                }
            } else {
                match native {
                    Ok(NativeFile::Open) => self.open_native(request).await,
                    Ok(NativeFile::Download { id }) => match self.monitor.file(id) {
                        Ok((_, bytes)) => response(StatusCode::OK, "application/octet-stream", bytes),
                        Err(error) => rpc_error(&error),
                    },
                    other => {
                        if let Ok(NativeFile::JsonDownload { id }) = other
                            && !id.bytes().all(|byte| byte.is_ascii_digit())
                        {
                            return text(StatusCode::NOT_FOUND, "Snapshot not found.");
                        }
                        let limit = if upload {
                            MAX_SNAPSHOT_BYTES.div_ceil(3) * 4 + 1024
                        } else {
                            DEFAULT_MAX_BODY_BYTES
                        };
                        serve_http_with_max_body_bytes(request, limit, &self.rest).await.map(wrap)
                    }
                }
            };
            if let Some(permit) = transfer {
                result = result.map(|body| {
                    body.map_frame(move |frame| {
                        let _permit = &permit;
                        frame
                    })
                    .boxed_unsync()
                });
            }
            return result;
        }

        let (kind, bytes) = match Asset::resolver().resolve(request.method().as_str(), request.uri().path()) {
            Ok(Asset::Index) => ("text/html; charset=utf-8", &include_bytes!("assets/index.html")[..]),
            Ok(Asset::Script) => ("text/javascript; charset=utf-8", &include_bytes!("assets/monitor.js")[..]),
            Ok(Asset::Style) => ("text/css; charset=utf-8", &include_bytes!("assets/monitor.css")[..]),
            Err(_) => return text(StatusCode::NOT_FOUND, "Page not found."),
        };
        response(StatusCode::OK, kind, Bytes::from_static(bytes))
    }

    async fn open_native(&self, request: Request<Incoming>) -> Response<Body> {
        if request
            .headers()
            .get(header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/octet-stream")
        {
            return text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Send the native snapshot as application/octet-stream.",
            );
        }
        let name = request
            .headers()
            .get("x-snapshot-name")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let data = match tokio::time::timeout(
            Duration::from_secs(30),
            Limited::new(request.into_body(), MAX_SNAPSHOT_BYTES).collect(),
        )
        .await
        {
            Ok(Ok(data)) => data.to_bytes(),
            Ok(Err(error)) if error.is::<LengthLimitError>() => return text(StatusCode::PAYLOAD_TOO_LARGE, "The snapshot exceeds 64 MiB."),
            Ok(Err(_)) => return text(StatusCode::BAD_REQUEST, "The upload did not complete."),
            Err(_) => return text(StatusCode::REQUEST_TIMEOUT, "The upload timed out."),
        };
        match self.monitor.open_file(name, data).await {
            Ok(snapshot) => match serde_json::to_vec(snapshot.get_ref()) {
                Ok(bytes) => response(StatusCode::OK, "application/json", Bytes::from(bytes)),
                Err(_) => text(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The server could not encode the snapshot summary.",
                ),
            },
            Err(error) => rpc_error(&error),
        }
    }
}

fn rpc_error(error: &tonic::Status) -> Response<Body> {
    let status = match error.code() {
        tonic::Code::NotFound => StatusCode::NOT_FOUND,
        tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
        tonic::Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    response(
        status,
        "text/plain; charset=utf-8",
        Bytes::copy_from_slice(error.message().as_bytes()),
    )
}

fn wrap(body: RestBody) -> Body {
    body.map_err(Into::into).boxed_unsync()
}

fn text(status: StatusCode, message: &'static str) -> Response<Body> {
    response(status, "text/plain; charset=utf-8", Bytes::from_static(message.as_bytes()))
}

fn response(status: StatusCode, content_type: &'static str, bytes: Bytes) -> Response<Body> {
    let mut response = Response::new(wrap(RestBody::buffered(bytes)));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
    response
}
