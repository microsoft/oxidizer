// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use bytes::Bytes;
use futures::StreamExt as _;
use http::{Request, Response, StatusCode};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::ServerConfig;
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use crate::recording::{RecordedRequest, ResponseFrame, ResponsePlan, ServerSnapshot};

type ResponseBody = UnsyncBoxBody<Bytes, Infallible>;

/// Translates a scripted plan into the body shape `hyper` serves.
fn into_response(plan: ResponsePlan) -> Response<ResponseBody> {
    let ResponsePlan {
        status,
        headers,
        frames,
        stall_after_frames,
    } = plan;
    let body = if !stall_after_frames && let [ResponseFrame::Data(data)] = frames.as_slice() {
        Full::new(data.clone()).boxed_unsync()
    } else {
        let frames = frames
            .into_iter()
            .map(|frame| {
                Ok(match frame {
                    ResponseFrame::Data(data) => Frame::data(data),
                    ResponseFrame::Trailers(trailers) => Frame::trailers(trailers),
                })
            })
            .collect::<Vec<Result<Frame<Bytes>, Infallible>>>();
        if stall_after_frames {
            StreamBody::new(futures::stream::iter(frames).chain(futures::stream::pending())).boxed_unsync()
        } else {
            StreamBody::new(futures::stream::iter(frames)).boxed_unsync()
        }
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[derive(Clone, Copy, Debug)]
enum Transport {
    Http,
    Https,
}

#[derive(Debug)]
struct State {
    responses: Vec<ResponsePlan>,
    next_response: AtomicUsize,
    requests: Mutex<Vec<(usize, RecordedRequest)>>,
    connections: AtomicUsize,
}

/// A scripted HTTP/1.1 and HTTP/2 origin server on the loopback interface.
///
/// Serves the response plans it was started with, one per request, and records
/// what arrived. `http` and `http_ipv6` serve plaintext; `https` negotiates TLS
/// with a self-signed certificate and advertises both `h2` and `http/1.1`.
#[derive(Debug)]
pub struct TestServer {
    address: SocketAddr,
    transport: Transport,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Serves plaintext HTTP over the IPv4 loopback.
    pub fn http(responses: impl IntoIterator<Item = ResponsePlan>) -> Self {
        Self::start(responses, Transport::Http, None, IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    /// Serves plaintext HTTP over the IPv6 loopback, so a caller reaches it through a bracketed
    /// authority.
    pub fn http_ipv6(responses: impl IntoIterator<Item = ResponsePlan>) -> Self {
        Self::start(responses, Transport::Http, None, IpAddr::V6(Ipv6Addr::LOCALHOST))
    }

    /// Serves TLS over the IPv4 loopback with a self-signed certificate issued
    /// for `certificate_names`.
    ///
    /// Naming something other than `localhost` is how a test produces a
    /// hostname mismatch, since callers reach the fixture through `localhost`.
    pub fn https(responses: impl IntoIterator<Item = ResponsePlan>, certificate_names: &[&str]) -> Self {
        let certificate_names = certificate_names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        let CertifiedKey { cert, signing_key } = generate_simple_self_signed(certificate_names).unwrap();
        let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private_key)
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        Self::start(
            responses,
            Transport::Https,
            Some(TlsAcceptor::from(Arc::new(config))),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
    }

    fn start(
        responses: impl IntoIterator<Item = ResponsePlan>,
        transport: Transport,
        tls_acceptor: Option<TlsAcceptor>,
        bind: IpAddr,
    ) -> Self {
        let listener = StdTcpListener::bind((bind, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(State {
            responses: responses.into_iter().collect(),
            next_response: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        });
        let (shutdown, shutdown_rx) = oneshot::channel();
        let server_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("fetch-winhttp-test-server".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
                runtime.block_on(run_server(listener, server_state, tls_acceptor, shutdown_rx));
            })
            .unwrap();

        Self {
            address,
            transport,
            state,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    /// The absolute URL that reaches `path` on this fixture.
    pub fn url(&self, path: &str) -> String {
        let scheme = match self.transport {
            Transport::Http => "http",
            Transport::Https => "https",
        };
        let host = match self.transport {
            // An IPv6 literal only forms a valid authority inside brackets.
            Transport::Http => match self.address.ip() {
                IpAddr::V4(address) => address.to_string(),
                IpAddr::V6(address) => format!("[{address}]"),
            },
            Transport::Https => "localhost".to_owned(),
        };
        format!("{scheme}://{host}:{}{path}", self.address.port())
    }

    /// Shuts the fixture down and returns what it observed.
    pub fn finish(mut self) -> ServerSnapshot {
        self.stop();
        let mut requests = self.state.requests.lock().unwrap().clone();
        requests.sort_by_key(|(index, _)| *index);

        ServerSnapshot {
            requests: requests.into_iter().map(|(_, request)| request).collect(),
            connections: self.state.connections.load(Ordering::SeqCst),
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run_server(listener: StdTcpListener, state: Arc<State>, tls_acceptor: Option<TlsAcceptor>, mut shutdown: oneshot::Receiver<()>) {
    let listener = TcpListener::from_std(listener).unwrap();
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else {
                    continue;
                };
                state.connections.fetch_add(1, Ordering::SeqCst);
                let state = Arc::clone(&state);
                let tls_acceptor = tls_acceptor.clone();
                connections.spawn(async move {
                    if let Some(acceptor) = tls_acceptor {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            serve_connection(stream, state).await;
                        }
                    } else {
                        serve_connection(stream, state).await;
                    }
                });
            }
        }
    }

    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

async fn serve_connection<I>(stream: I, state: Arc<State>)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| handle_request(request, Arc::clone(&state)));
    let builder = auto::Builder::new(TokioExecutor::new());
    let _ = builder.serve_connection(TokioIo::new(stream), service).await;
}

async fn handle_request(request: Request<Incoming>, state: Arc<State>) -> Result<Response<ResponseBody>, Infallible> {
    let index = state.next_response.fetch_add(1, Ordering::SeqCst);
    let (parts, body) = request.into_parts();
    let collected = body.collect().await;
    let recorded = match collected {
        Ok(collected) => {
            let trailers = collected.trailers().cloned();
            RecordedRequest {
                method: parts.method,
                uri: parts.uri,
                version: parts.version,
                headers: parts.headers,
                body: collected.to_bytes(),
                trailers,
            }
        }
        Err(_) => {
            return Ok(into_response(ResponsePlan::status(StatusCode::BAD_REQUEST)));
        }
    };
    state.requests.lock().unwrap().push((index, recorded));

    let response = state
        .responses
        .get(index)
        .cloned()
        .unwrap_or_else(|| ResponsePlan::status(StatusCode::INTERNAL_SERVER_ERROR));
    Ok(into_response(response))
}
