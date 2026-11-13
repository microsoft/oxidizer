// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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

use super::recording::{RecordedRequest, ResponseFrame, ResponsePlan, ResponseScript, ServerSnapshot};
use super::startup::{Startup, StartupListener, bind_listener, launch_worker};

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

#[cfg(test)]
mod gamma_survivor_tests {
    use std::cell::{Cell, RefCell};
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};
    use std::thread;

    use http_body::Body as _;

    use super::super::startup::gamma_round2::{GammaListener, GammaRuntime, tls_config};
    use super::super::startup::run_worker;
    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native tests cover TLS fixture initialization and ALPN; the crypto provider requires foreign functions"
    )]
    fn november6_actual_server_start_preserves_the_tls_acceptor_and_bind_address() {
        let mut config = tls_config();
        config.alpn_protocols = vec![b"declared-protocol".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let server = TestServer::start(
            ResponseScript::Repeat(ResponsePlan::status(StatusCode::CREATED)),
            Transport::Https,
            Some(acceptor),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        );
        assert_eq!(server.address, "[::1]:8123".parse().unwrap());
        assert!(matches!(server.transport, Transport::Https));
        assert_eq!(server.state.script.plan(0).status, StatusCode::CREATED);
        scripted_start::OBSERVED.with_borrow_mut(|observed| {
            assert_eq!(std::mem::take(observed), [Some(vec![b"declared-protocol".to_vec()])]);
        });
        assert!(server.shutdown.is_none());
        assert!(server.thread.is_none());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native tests cover TLS fixture startup and shutdown; the crypto provider requires foreign functions"
    )]
    fn gamma_round2_tcp_startup_retains_configured_worker_state_and_shutdown() {
        let address: SocketAddr = "[::1]:3123".parse().unwrap();
        let worker_state = RefCell::new(None);
        let shutdown_receiver = RefCell::new(None);
        let served = Cell::new(0);
        let mut startup = TestServer::start_with(
            ResponseScript::Sequence(vec![ResponsePlan::status(StatusCode::CREATED)]),
            Some(TlsAcceptor::from(Arc::new(tls_config()))),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            |bind| {
                assert_eq!(bind, "[::1]:0".parse().unwrap());
                GammaListener {
                    address,
                    nonblocking: Cell::new(None),
                }
            },
            |listener, state, acceptor, receiver| {
                assert_eq!(listener.nonblocking.get(), Some(true));
                assert_eq!(listener.address, address);
                assert!(acceptor.is_some());
                *worker_state.borrow_mut() = Some(Arc::clone(&state));
                *shutdown_receiver.borrow_mut() = Some(receiver);
                async {
                    served.set(served.get() + 1);
                }
            },
            |name, work| {
                assert_eq!(name, "fetch-winhttp-test-server");
                run_worker(&GammaRuntime, work);
                47
            },
        );
        assert_eq!(served.get(), 1);
        assert_eq!(startup.address, address);
        assert_eq!(startup.thread.take(), Some(47));
        assert!(Arc::ptr_eq(&startup.state, worker_state.borrow().as_ref().unwrap()));
        assert_eq!(startup.state.next_response.load(Ordering::SeqCst), 0);
        assert_eq!(startup.state.connections.load(Ordering::SeqCst), 0);
        assert!(startup.state.requests.lock().unwrap().is_empty());
        assert_eq!(startup.state.script.plan(0).status, StatusCode::CREATED);
        startup.shutdown.take().unwrap().send(()).unwrap();
        assert_eq!(shutdown_receiver.borrow_mut().as_mut().unwrap().try_recv(), Ok(()));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native tests cover TLS fixture initialization and ALPN; the crypto provider requires foreign functions"
    )]
    fn gamma_round2_tls_startup_forwards_names_ordered_alpn_and_acceptor() {
        let issued = Cell::new(0);
        let result = TestServer::start_tls_with(
            ResponseScript::Repeat(ResponsePlan::status(StatusCode::ACCEPTED)),
            &["first.test", "second.test"],
            |names| {
                assert_eq!(names, ["first.test", "second.test"]);
                issued.set(issued.get() + 1);
                tls_config()
            },
            |script, transport, acceptor, bind| {
                assert_eq!(script.plan(0).status, StatusCode::ACCEPTED);
                assert!(matches!(transport, Transport::Https));
                assert_eq!(bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
                assert_eq!(acceptor.unwrap().config().alpn_protocols, [b"h2".to_vec(), b"http/1.1".to_vec()]);
                53
            },
        );
        assert_eq!(result, 53);
        assert_eq!(issued.get(), 1);
    }

    fn stopped_server(transport: Transport, shutdown: Option<oneshot::Sender<()>>) -> TestServer {
        TestServer {
            address: SocketAddr::from(([127, 0, 0, 1], 8123)),
            transport,
            state: Arc::new(State {
                script: ResponseScript::Sequence(Vec::new()),
                next_response: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                connections: AtomicUsize::new(3),
            }),
            shutdown,
            thread: None,
        }
    }

    #[test]
    fn gamma_round2_finish_waits_for_shutdown_before_taking_snapshot() {
        let (sender, receiver) = oneshot::channel();
        let mut server = stopped_server(Transport::Http, Some(sender));
        let state = Arc::clone(&server.state);
        server.thread = Some(thread::spawn(move || {
            futures::executor::block_on(receiver).unwrap();
            state.connections.store(9, Ordering::SeqCst);
        }));
        assert_eq!(server.finish().connections, 9);
    }

    #[test]
    fn gamma_round2_stop_propagates_a_worker_panic() {
        let mut server = stopped_server(Transport::Http, None);
        server.thread = Some(thread::spawn(|| panic!("gamma_round2_worker")));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.stop())).is_err());
        assert!(server.thread.is_none());
    }

    async fn gamma_round2_request(state: Arc<State>) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let (io, mut peer) = tokio::io::duplex(4096);
        let connection = tokio::spawn(serve_connection(io, state));
        peer.write_all(b"PATCH /in-memory?x=1 HTTP/1.1\r\nHost: example.test\r\nX-Marker: kept\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload").await.unwrap();
        let mut response = Vec::new();
        peer.read_to_end(&mut response).await.unwrap();
        connection.await.unwrap();
        String::from_utf8(response).unwrap()
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native tests cover HTTP/1 fixture request sequencing and recording; Hyper reads the OS wall clock"
    )]
    fn gamma_round2_handler_sequences_and_records_in_memory_requests() {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
            let state = Arc::new(State {
                script: ResponseScript::Sequence(vec![
                    ResponsePlan::status(StatusCode::CREATED),
                    ResponsePlan::status(StatusCode::ACCEPTED),
                ]),
                next_response: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                connections: AtomicUsize::new(0),
            });
            for expected in [StatusCode::CREATED, StatusCode::ACCEPTED] {
                let response = gamma_round2_request(Arc::clone(&state)).await;
                assert!(response.starts_with(&format!("HTTP/1.1 {expected}\r\n")), "{response}");
            }
            let records = state.requests.lock().unwrap();
            assert_eq!(records.len(), 2);
            for (index, (record_index, record)) in records.iter().enumerate() {
                assert_eq!(*record_index, index);
                assert_eq!(record.method, http::Method::PATCH);
                assert_eq!(record.uri, "/in-memory?x=1");
                assert_eq!(record.headers["x-marker"], "kept");
                assert_eq!(record.body, "payload");
            }
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native tests cover repeating response scripts without recording; Hyper reads the OS wall clock"
    )]
    fn gamma_round2_repeating_handler_does_not_accumulate_records() {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
            let state = Arc::new(State {
                script: ResponseScript::Repeat(ResponsePlan::status(StatusCode::FORBIDDEN)),
                next_response: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                connections: AtomicUsize::new(0),
            });
            let response = gamma_round2_request(Arc::clone(&state)).await;
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{response}");
            assert!(state.requests.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn gamma_survivor_stopped_server_urls_and_snapshot_order_are_exact() {
        let server = stopped_server(Transport::Http, None);
        assert_eq!(server.url("/path?q=1"), "http://127.0.0.1:8123/path?q=1");
        for index in [2, 0, 1] {
            server.state.requests.lock().unwrap().push((
                index,
                RecordedRequest {
                    method: http::Method::GET,
                    uri: format!("/request-{index}").parse().unwrap(),
                    version: http::Version::HTTP_11,
                    headers: http::HeaderMap::new(),
                    body: Bytes::new(),
                    trailers: None,
                },
            ));
        }
        let snapshot = server.finish();
        assert_eq!(snapshot.connections, 3);
        assert_eq!(
            snapshot.requests.iter().map(|request| request.uri.to_string()).collect::<Vec<_>>(),
            ["/request-0", "/request-1", "/request-2"]
        );
        assert_eq!(stopped_server(Transport::Https, None).url("/tls"), "https://localhost:8123/tls");
    }

    #[test]
    fn gamma_survivor_server_stop_and_drop_signal_in_process_shutdown() {
        let (sender, mut receiver) = oneshot::channel();
        let mut server = stopped_server(Transport::Http, Some(sender));
        server.stop();
        assert!(server.shutdown.is_none());
        assert_eq!(receiver.try_recv(), Ok(()));
        server.stop();
        let (sender, mut receiver) = oneshot::channel();
        drop(stopped_server(Transport::Http, Some(sender)));
        assert_eq!(receiver.try_recv(), Ok(()));
    }

    #[test]
    fn gamma_survivor_scripted_response_preserves_status_headers_and_stalling() {
        let plan = ResponsePlan::status(StatusCode::ACCEPTED)
            .header(http::HeaderName::from_static("x-marker"), http::HeaderValue::from_static("value"));
        let response = into_response(plan);
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()["x-marker"], "value");
        let mut body = into_response(ResponsePlan::ok("abc").stall_after_frames()).into_body();
        let mut cx = Context::from_waker(Waker::noop());
        let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx) else {
            panic!("scripted data must be ready");
        };
        assert_eq!(frame.into_data().unwrap(), b"abc".as_slice());
        assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
        let mut body = into_response(ResponsePlan::chunks([Bytes::from_static(b"a"), Bytes::from_static(b"b")])).into_body();
        for expected in [b"a", b"b"] {
            let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx) else {
                panic!("each scripted chunk must be ready");
            };
            assert_eq!(frame.into_data().unwrap(), expected.as_slice());
        }
        assert!(matches!(Pin::new(&mut body).poll_frame(&mut cx), Poll::Ready(None)));
    }
}

#[derive(Clone, Copy, Debug)]
enum Transport {
    Http,
    Https,
}

#[derive(Debug)]
struct State {
    script: ResponseScript,
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
        Self::start(
            ResponseScript::Sequence(responses.into_iter().collect()),
            Transport::Http,
            None,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
    }

    /// Serves plaintext HTTP over the IPv4 loopback, answering every request with `plan`.
    pub fn http_repeating(plan: ResponsePlan) -> Self {
        Self::start(ResponseScript::Repeat(plan), Transport::Http, None, IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    /// Serves plaintext HTTP over the IPv6 loopback, so a caller reaches it through a bracketed
    /// authority.
    pub fn http_ipv6(responses: impl IntoIterator<Item = ResponsePlan>) -> Self {
        Self::start(
            ResponseScript::Sequence(responses.into_iter().collect()),
            Transport::Http,
            None,
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
    }

    /// Serves TLS over the IPv4 loopback with a self-signed certificate issued
    /// for `certificate_names`.
    ///
    /// Naming something other than `localhost` is how a test produces a
    /// hostname mismatch, since callers reach the fixture through `localhost`.
    pub fn https(responses: impl IntoIterator<Item = ResponsePlan>, certificate_names: &[&str]) -> Self {
        Self::start_tls(ResponseScript::Sequence(responses.into_iter().collect()), certificate_names)
    }

    /// Serves TLS over the IPv4 loopback, answering every request with `plan`.
    pub fn https_repeating(plan: ResponsePlan, certificate_names: &[&str]) -> Self {
        Self::start_tls(ResponseScript::Repeat(plan), certificate_names)
    }

    fn start_tls(script: ResponseScript, certificate_names: &[&str]) -> Self {
        Self::start_tls_with(
            script,
            certificate_names,
            |certificate_names| {
                let CertifiedKey { cert, signing_key } = generate_simple_self_signed(certificate_names).unwrap();
                let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
                ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(vec![cert.der().clone()], private_key)
                    .unwrap()
            },
            Self::start,
        )
    }

    fn start_tls_with<R>(
        script: ResponseScript,
        certificate_names: &[&str],
        configure: impl FnOnce(Vec<String>) -> ServerConfig,
        start: impl FnOnce(ResponseScript, Transport, Option<TlsAcceptor>, IpAddr) -> R,
    ) -> R {
        let certificate_names = certificate_names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        let mut config = configure(certificate_names);
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        start(
            script,
            Transport::Https,
            Some(TlsAcceptor::from(Arc::new(config))),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
    }

    fn start(script: ResponseScript, transport: Transport, tls_acceptor: Option<TlsAcceptor>, bind: IpAddr) -> Self {
        #[cfg(test)]
        use scripted_start::ServerStarter;

        #[cfg(not(test))]
        use self::TestServer as ServerStarter;
        #[cfg_attr(
            not(test),
            expect(clippy::use_self, reason = "the type alias selects a scripted callee in unit tests")
        )]
        let startup = ServerStarter::start_with(
            script,
            tls_acceptor,
            bind,
            |address| StdTcpListener::bind(address).unwrap(),
            run_server,
            |name, work| launch_worker(name, tokio::runtime::Builder::new_multi_thread, work),
        );
        Self {
            address: startup.address,
            transport,
            state: startup.state,
            shutdown: startup.shutdown,
            thread: startup.thread,
        }
    }

    fn start_with<L: StartupListener, W, F: Future<Output = ()>>(
        script: ResponseScript,
        tls_acceptor: Option<TlsAcceptor>,
        bind: IpAddr,
        bind_tcp: impl FnOnce(SocketAddr) -> L,
        serve: impl FnOnce(L, Arc<State>, Option<TlsAcceptor>, oneshot::Receiver<()>) -> F,
        launch: impl FnOnce(String, F) -> W,
    ) -> Startup<Arc<State>, W> {
        let (listener, address) = bind_listener(bind, bind_tcp);
        let state = Arc::new(State {
            script,
            next_response: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        });
        let (shutdown, shutdown_rx) = oneshot::channel();
        let server_state = Arc::clone(&state);
        let work = serve(listener, server_state, tls_acceptor, shutdown_rx);
        let thread = launch("fetch-winhttp-test-server".to_owned(), work);

        Startup {
            address,
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

#[cfg(test)]
mod scripted_start {
    use super::*;

    pub(super) struct ServerStarter;

    thread_local! {
        pub(super) static OBSERVED: std::cell::RefCell<Vec<Option<Vec<Vec<u8>>>>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    impl ServerStarter {
        pub(super) fn start_with<L: StartupListener, W, F: Future<Output = ()>>(
            script: ResponseScript,
            tls_acceptor: Option<TlsAcceptor>,
            bind: IpAddr,
            _bind_tcp: impl FnOnce(SocketAddr) -> L,
            _serve: impl FnOnce(L, Arc<State>, Option<TlsAcceptor>, oneshot::Receiver<()>) -> F,
            _launch: impl FnOnce(String, F) -> W,
        ) -> Startup<Arc<State>, W> {
            OBSERVED.with_borrow_mut(|observed| {
                observed.push(tls_acceptor.map(|acceptor| acceptor.config().alpn_protocols.clone()));
            });
            Startup {
                address: SocketAddr::new(bind, 8123),
                state: Arc::new(State {
                    script,
                    next_response: AtomicUsize::new(0),
                    requests: Mutex::new(Vec::new()),
                    connections: AtomicUsize::new(0),
                }),
                shutdown: None,
                thread: None,
            }
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

// #[gamma::skip(fn_value.unit, parameter.default_shadow, reason = "this adapter accepts native TCP sockets and performs optional TLS handshakes; tls_acceptor is its only defaultable parameter, and controlled startup tests assert that it is forwarded intact without opening a listener")]
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

    super::stop_connections(&mut connections).await;
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
    if state.script.records() {
        state.requests.lock().unwrap().push((index, recorded));
    }

    Ok(into_response(state.script.plan(index)))
}
