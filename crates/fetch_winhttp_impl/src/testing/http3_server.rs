// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bytes::{Buf as _, BytesMut};
use h3_quinn::Connection;
use http::header::CONTENT_LENGTH;
use http::{HeaderValue, Response, Version};
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, Incoming, ServerConfig, VarInt};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use super::recording::{RecordedRequest, ResponseFrame, ResponsePlan, ResponseScript, ServerSnapshot};
use super::startup::{Startup, ThreadLauncher, WorkerLauncher};

#[derive(Debug)]
struct State {
    script: ResponseScript,
    next_response: AtomicUsize,
    requests: Mutex<Vec<RecordedRequest>>,
    connections: AtomicUsize,
}

/// Scripted HTTP/3 origin server, the QUIC counterpart of `server.rs`.
///
/// Serves the same [`ResponsePlan`] vocabulary and produces the same [`ServerSnapshot`], so an
/// HTTP/3 test can script an error status, a response header, a multi-frame body or trailers the
/// way a TCP test does.
///
/// Unlike `server.rs`, requests are served strictly sequentially per QUIC connection, so a
/// stalling plan parks that connection for the remainder of the fixture's life. A test that needs
/// a stalled HTTP/3 response followed by a further request would hang rather than fail; giving
/// each accepted stream its own task is a prerequisite for that scenario.
#[derive(Debug)]
pub struct Http3Server {
    address: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Http3Server {
    /// Starts the fixture on an ephemeral loopback UDP port with a self-signed
    /// `localhost` certificate, serving `responses` in order.
    pub fn start(responses: impl IntoIterator<Item = ResponsePlan>) -> Self {
        Self::start_scripted(ResponseScript::Sequence(responses.into_iter().collect()))
    }

    /// Starts the fixture answering every request with `plan`.
    pub fn start_repeating(plan: ResponsePlan) -> Self {
        Self::start_scripted(ResponseScript::Repeat(plan))
    }

    fn start_scripted(script: ResponseScript) -> Self {
        let startup = Self::start_scripted_with(
            script,
            || {
                let endpoint = create_endpoint();
                let address = endpoint.local_addr().unwrap();
                (endpoint, address)
            },
            run_server,
            ThreadLauncher,
        );
        Self {
            address: startup.address,
            state: startup.state,
            shutdown: startup.shutdown,
            thread: startup.thread,
        }
    }

    fn start_scripted_with<E: Send + 'static, W, F: Future<Output = ()> + Send + 'static>(
        script: ResponseScript,
        endpoint: impl FnOnce() -> (E, SocketAddr) + Send + 'static,
        serve: impl FnOnce(E, Arc<State>, oneshot::Receiver<()>) -> F + Send + 'static,
        launch: impl WorkerLauncher<Worker = W>,
    ) -> Startup<Arc<State>, W> {
        let state = Arc::new(State {
            script,
            next_response: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        });
        // #[gamma::skip(literal.int_decrement, tag = "resource", reason = "Nov 4 SARIF 736cd5f0b796 timed out with a zero-capacity startup channel before the receiver could run")]
        // #[gamma::skip(literal.int_increment, tag = "equivalent", reason = "exactly one address is sent and received; capacity one and capacity two both accept that send without blocking")]
        let (address_tx, address_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let server_state = Arc::clone(&state);
        let work = async move {
            let (endpoint, address) = endpoint();
            address_tx.send(address).unwrap();
            serve(endpoint, server_state, shutdown_rx).await;
        };
        let thread = launch.launch("fetch-winhttp-http3-test-server".to_owned(), work);
        let address = address_rx.recv().unwrap();

        Startup {
            address,
            state,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    /// The absolute URL that reaches `path` on this fixture.
    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.address.port())
    }

    /// Shuts the fixture down and returns what it observed.
    pub fn finish(mut self) -> ServerSnapshot {
        self.stop();
        ServerSnapshot {
            requests: self.state.requests.lock().unwrap().clone(),
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

impl Drop for Http3Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn create_endpoint() -> Endpoint {
    create_endpoint_with(
        |names| {
            let CertifiedKey { cert, signing_key } = generate_simple_self_signed(names).unwrap();
            let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert.der().clone()], private_key)
                .unwrap()
        },
        |tls, bind| {
            let crypto = QuicServerConfig::try_from(tls).unwrap();
            let server_config = ServerConfig::with_crypto(Arc::new(crypto));
            Endpoint::server(server_config, bind).unwrap()
        },
    )
}

fn create_endpoint_with<E>(
    configure: impl FnOnce([String; 1]) -> rustls::ServerConfig,
    bind_endpoint: impl FnOnce(rustls::ServerConfig, SocketAddr) -> E,
) -> E {
    let mut tls = configure(["localhost".to_owned()]);
    tls.alpn_protocols = vec![b"h3".to_vec()];
    bind_endpoint(tls, (Ipv4Addr::LOCALHOST, 0).into())
}

// #[gamma::skip(fn_value.unit, reason = "this adapter owns the native QUIC accept loop and endpoint; request-loop, response and shutdown arguments are tested with scripted collaborators without UDP sockets")]
async fn run_server(endpoint: Endpoint, state: Arc<State>, mut shutdown: oneshot::Receiver<()>) {
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                state.connections.fetch_add(1, Ordering::SeqCst);
                let state = Arc::clone(&state);
                connections.spawn(async move {
                    serve_connection(incoming, state).await;
                });
            }
        }
    }

    // #[gamma::skip(call.replace_with_default, stmt.delete_call, reason = "this call closes a native QUIC endpoint after its accept loop; the shutdown code and reason are tested through EndpointShutdown without opening a UDP socket")]
    close_endpoint(&endpoint);
    // Abort the in-flight connection tasks rather than waiting for them to observe the endpoint
    // closure. A task blocked in `h3` request handling would otherwise wedge the join loop, and
    // this loop runs on the shutdown path of `Http3Server::finish`/`Drop`, so a wedged task would
    // hang the test binary with no diagnostic instead of failing an assertion.
    super::stop_connections(&mut connections).await;
    endpoint.wait_idle().await;
}

trait EndpointShutdown {
    fn close(&self, code: VarInt, reason: &[u8]);
}

impl EndpointShutdown for Endpoint {
    // #[gamma::skip(fn_value.unit, stmt.delete_call, reason = "this leaf forwards shutdown to a native QUIC endpoint; the close policy is tested through a scripted EndpointShutdown without opening a UDP socket")]
    fn close(&self, code: VarInt, reason: &[u8]) {
        Self::close(self, code, reason);
    }
}

fn close_endpoint(endpoint: &impl EndpointShutdown) {
    endpoint.close(VarInt::from_u32(0), b"test shutdown");
}

// #[gamma::skip(fn_value.unit, reason = "this adapter awaits native QUIC and HTTP/3 handshakes; the subsequent request loop and response decisions run against scripted collaborators")]
async fn serve_connection(incoming: Incoming, state: Arc<State>) {
    let Ok(connection) = incoming.await else {
        return;
    };
    let Ok(mut connection) = h3::server::builder().build(Connection::new(connection)).await else {
        return;
    };

    serve_requests(&mut connection, &state).await;
}

trait RequestCycle {
    async fn next(&mut self, state: &State) -> Option<bool>;
}

impl RequestCycle for h3::server::Connection<Connection, bytes::Bytes> {
    // #[gamma::skip(fn_value.none, try.propagate_to_unwrap, option.some_to_none, reason = "This adapter accepts native QUIC request streams; scripted RequestCycle and RequestReader tests cover request-loop termination and read failures without network I/O")]
    async fn next(&mut self, state: &State) -> Option<bool> {
        let resolver = self.accept().await.ok()??;
        let (request, mut stream) = resolver.resolve_request().await.ok()?;
        Some(serve_request(&mut stream, &request, state).await)
    }
}

async fn serve_requests(connection: &mut impl RequestCycle, state: &State) {
    while let Some(sent) = connection.next(state).await {
        if !sent {
            return;
        }
    }
}

async fn serve_request(stream: &mut (impl RequestReader + ResponseWriter), request: &http::Request<()>, state: &State) -> bool {
    let Some((body, trailers)) = read_request_body(stream).await else {
        return false;
    };
    let (response, frames, stall_after_frames) = prepare_response(state, request, body, trailers);
    send_response_frames(stream, response, frames, stall_after_frames).await
}

trait RequestReader {
    async fn recv_data(&mut self) -> Result<Option<bytes::Bytes>, ()>;
    async fn recv_trailers(&mut self) -> Result<Option<http::HeaderMap>, ()>;
}

impl<S: h3::quic::BidiStream<bytes::Bytes>> RequestReader for h3::server::RequestStream<S, bytes::Bytes> {
    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf delegates to a native HTTP/3 stream; read EOF, data, trailer and failure policy is tested through scripted RequestReader values without QUIC traffic")]
    async fn recv_data(&mut self) -> Result<Option<bytes::Bytes>, ()> {
        Self::recv_data(self)
            .await
            .map(|data| {
                data.map(|mut data| {
                    let remaining = data.remaining();
                    data.copy_to_bytes(remaining)
                })
            })
            .map_err(|_error| ())
    }

    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf delegates to a native HTTP/3 stream; trailer success and failure policy is tested through scripted RequestReader values without QUIC traffic")]
    async fn recv_trailers(&mut self) -> Result<Option<http::HeaderMap>, ()> {
        Self::recv_trailers(self).await.map_err(|_error| ())
    }
}

async fn read_request_body(stream: &mut impl RequestReader) -> Option<(bytes::Bytes, Option<http::HeaderMap>)> {
    let mut body = BytesMut::new();
    loop {
        match stream.recv_data().await {
            Ok(Some(mut data)) => {
                let remaining = data.remaining();
                body.extend_from_slice(&data.copy_to_bytes(remaining));
            }
            Ok(None) => break,
            Err(()) => return None,
        }
    }
    let Ok(trailers) = stream.recv_trailers().await else {
        return None;
    };
    Some((body.freeze(), trailers))
}

trait ResponseWriter {
    async fn send_response(&mut self, response: Response<()>) -> Result<(), ()>;
    async fn send_data(&mut self, data: bytes::Bytes) -> Result<(), ()>;
    async fn send_trailers(&mut self, trailers: http::HeaderMap) -> Result<(), ()>;
    async fn finish(&mut self) -> Result<(), ()>;
}

impl<S: h3::quic::BidiStream<bytes::Bytes>> ResponseWriter for h3::server::RequestStream<S, bytes::Bytes> {
    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf sends native HTTP/3 headers; response success and failure policy is tested through scripted ResponseWriter values without QUIC traffic")]
    async fn send_response(&mut self, response: Response<()>) -> Result<(), ()> {
        Self::send_response(self, response).await.map_err(|_error| ())
    }

    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf sends native HTTP/3 data; frame ordering and failure policy is tested through scripted ResponseWriter values without QUIC traffic")]
    async fn send_data(&mut self, data: bytes::Bytes) -> Result<(), ()> {
        Self::send_data(self, data).await.map_err(|_error| ())
    }

    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf sends native HTTP/3 trailers; trailer ordering and failure policy is tested through scripted ResponseWriter values without QUIC traffic")]
    async fn send_trailers(&mut self, trailers: http::HeaderMap) -> Result<(), ()> {
        Self::send_trailers(self, trailers).await.map_err(|_error| ())
    }

    // #[gamma::skip(fn_value.err_default, fn_value.ok, reason = "this leaf finishes a native HTTP/3 stream; finish success and failure policy is tested through scripted ResponseWriter values without QUIC traffic")]
    async fn finish(&mut self) -> Result<(), ()> {
        Self::finish(self).await.map_err(|_error| ())
    }
}

async fn send_response_frames(
    stream: &mut impl ResponseWriter,
    response: Response<()>,
    frames: Vec<ResponseFrame>,
    stall_after_frames: bool,
) -> bool {
    if stream.send_response(response).await.is_err() {
        return false;
    }
    for frame in frames {
        let sent = match frame {
            ResponseFrame::Data(data) => stream.send_data(data).await,
            ResponseFrame::Trailers(trailers) => stream.send_trailers(trailers).await,
        };
        if sent.is_err() {
            return false;
        }
    }
    if stall_after_frames {
        // The connection task is aborted on fixture shutdown.
        std::future::pending::<()>().await;
    }
    if stream.finish().await.is_err() {
        return false;
    }
    true
}

fn prepare_response(
    state: &State,
    request: &http::Request<()>,
    body: bytes::Bytes,
    trailers: Option<http::HeaderMap>,
) -> (Response<()>, Vec<ResponseFrame>, bool) {
    let index = state.next_response.fetch_add(1, Ordering::SeqCst);
    if state.script.records() {
        state.requests.lock().unwrap().push(RecordedRequest {
            method: request.method().clone(),
            uri: request.uri().clone(),
            version: Version::HTTP_3,
            headers: request.headers().clone(),
            body,
            trailers,
        });
    }
    let plan = state.script.plan(index);
    let body_length = plan.body_length();
    let ResponsePlan {
        status,
        mut headers,
        frames,
        stall_after_frames,
    } = plan;
    // `h3` writes exactly the header list it is handed, unlike `hyper`, which derives the
    // framing header for a body of known length. A stalling plan deliberately leaves the body
    // length undeclared: declaring it would tell the client the body is already complete,
    // which is the opposite of what a stall is scripted to observe.
    if !stall_after_frames && !headers.contains_key(CONTENT_LENGTH) {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(body_length));
    }
    let mut response = Response::new(());
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    (response, frames, stall_after_frames)
}

#[cfg(test)]
mod november4_response_tests {
    use std::collections::VecDeque;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    fn state(plan: ResponsePlan) -> State {
        State {
            script: ResponseScript::Repeat(plan),
            next_response: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        }
    }

    #[test]
    fn november6_http3_request_loop_continues_after_success_and_stops_at_failure_or_eof() {
        struct Cycle {
            outcomes: VecDeque<Option<bool>>,
            calls: usize,
        }
        impl RequestCycle for Cycle {
            async fn next(&mut self, _: &State) -> Option<bool> {
                self.calls += 1;
                self.outcomes.pop_front().unwrap()
            }
        }
        for (outcomes, expected_calls, remaining) in [
            (vec![Some(true), Some(true), None], 3, 0),
            (vec![Some(false), Some(true), None], 1, 2),
            (vec![Some(true), Some(false), None], 2, 1),
            (vec![None, Some(true)], 1, 1),
        ] {
            let mut cycle = Cycle {
                outcomes: outcomes.into(),
                calls: 0,
            };
            {
                let state = state(ResponsePlan::ok(""));
                let mut future = pin!(serve_requests(&mut cycle, &state));
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
            }
            assert_eq!(cycle.calls, expected_calls);
            assert_eq!(cycle.outcomes.len(), remaining);
        }
    }

    struct Stream(Reader, Writer);

    impl RequestReader for Stream {
        async fn recv_data(&mut self) -> Result<Option<bytes::Bytes>, ()> {
            self.0.recv_data().await
        }
        async fn recv_trailers(&mut self) -> Result<Option<http::HeaderMap>, ()> {
            self.0.recv_trailers().await
        }
    }

    impl ResponseWriter for Stream {
        async fn send_response(&mut self, response: Response<()>) -> Result<(), ()> {
            self.1.send_response(response).await
        }
        async fn send_data(&mut self, data: bytes::Bytes) -> Result<(), ()> {
            self.1.send_data(data).await
        }
        async fn send_trailers(&mut self, trailers: http::HeaderMap) -> Result<(), ()> {
            self.1.send_trailers(trailers).await
        }
        async fn finish(&mut self) -> Result<(), ()> {
            self.1.finish().await
        }
    }

    #[test]
    fn request_read_failure_does_not_consume_a_response_or_report_success() {
        for (reader, expected_calls) in [
            (
                Reader {
                    data: [Err(())].into(),
                    trailers: None,
                    calls: Vec::new(),
                },
                vec!["data"],
            ),
            (
                Reader {
                    data: [Ok(None)].into(),
                    trailers: Some(Err(())),
                    calls: Vec::new(),
                },
                vec!["data", "trailers"],
            ),
        ] {
            let state = state(ResponsePlan::ok("unused"));
            let mut stream = Stream(reader, Writer::default());
            {
                let request = http::Request::new(());
                let mut future = pin!(serve_request(&mut stream, &request, &state));
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(false));
            }
            assert_eq!(stream.0.calls, expected_calls);
            assert!(stream.1.calls.is_empty());
            assert_eq!(state.next_response.load(Ordering::SeqCst), 0);
            assert!(state.requests.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn november6_http3_request_dispatch_preserves_the_scripted_stall_choice() {
        for stall in [false, true] {
            let mut plan = ResponsePlan::status(http::StatusCode::CREATED)
                .header(http::HeaderName::from_static("x-response"), HeaderValue::from_static("preserved"));
            plan.frames = frames();
            if stall {
                plan = plan.stall_after_frames();
            }
            let state = state(plan);
            let mut stream = Stream(
                Reader {
                    data: [Ok(None)].into(),
                    trailers: Some(Ok(None)),
                    calls: Vec::new(),
                },
                Writer::default(),
            );
            {
                let request = http::Request::new(());
                let mut future = pin!(serve_request(&mut stream, &request, &state));
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(future.as_mut().poll(&mut cx), if stall { Poll::Pending } else { Poll::Ready(true) });
            }
            assert_eq!(stream.0.calls, ["data", "trailers"]);
            assert_eq!(
                stream.1.calls,
                if stall {
                    vec!["response", "data", "data", "trailers"]
                } else {
                    vec!["response", "data", "data", "trailers", "finish"]
                }
            );
            assert_eq!(state.next_response.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn november4_http3_shutdown_sends_exactly_one_success_code_and_reason() {
        #[derive(Default)]
        struct Shutdown(Mutex<Vec<(u64, Vec<u8>)>>);

        impl EndpointShutdown for Shutdown {
            fn close(&self, code: VarInt, reason: &[u8]) {
                self.0.lock().unwrap().push((code.into_inner(), reason.to_vec()));
            }
        }
        let endpoint = Shutdown::default();
        close_endpoint(&endpoint);
        assert_eq!(*endpoint.0.lock().unwrap(), [(0, b"test shutdown".to_vec())]);
    }

    struct Reader {
        data: VecDeque<Result<Option<bytes::Bytes>, ()>>,
        trailers: Option<Result<Option<http::HeaderMap>, ()>>,
        calls: Vec<&'static str>,
    }

    impl RequestReader for Reader {
        async fn recv_data(&mut self) -> Result<Option<bytes::Bytes>, ()> {
            self.calls.push("data");
            match self.data.pop_front() {
                Some(result) => result,
                None => std::future::pending().await,
            }
        }

        async fn recv_trailers(&mut self) -> Result<Option<http::HeaderMap>, ()> {
            self.calls.push("trailers");
            self.trailers.take().unwrap()
        }
    }

    #[test]
    fn november4_http3_request_body_stops_at_eof_and_preserves_every_frame() {
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-request-trailer", HeaderValue::from_static("preserved"));
        let mut reader = Reader {
            data: [
                Ok(Some(bytes::Bytes::from_static(b"first"))),
                Ok(Some(bytes::Bytes::new())),
                Ok(Some(bytes::Bytes::from_static(b"last"))),
                Ok(None),
            ]
            .into(),
            trailers: Some(Ok(Some(trailers.clone()))),
            calls: Vec::new(),
        };
        {
            let mut future = pin!(read_request_body(&mut reader));
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Some((bytes::Bytes::from_static(b"firstlast"), Some(trailers))))
            );
        }
        assert_eq!(reader.calls, ["data", "data", "data", "data", "trailers"]);
        assert!(reader.data.is_empty());
        assert!(reader.trailers.is_none());
    }

    #[test]
    fn november4_http3_request_read_failures_never_advance_past_the_failed_stage() {
        for data_failed in [true, false] {
            let mut reader = Reader {
                data: [if data_failed { Err(()) } else { Ok(None) }].into(),
                trailers: Some(Err(())),
                calls: Vec::new(),
            };
            {
                let mut future = pin!(read_request_body(&mut reader));
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(None));
            }
            assert_eq!(reader.calls, if data_failed { vec!["data"] } else { vec!["data", "trailers"] });
            assert_eq!(reader.trailers.is_some(), data_failed);
        }
    }

    #[derive(Default)]
    struct Writer {
        calls: Vec<&'static str>,
        fail_at: usize,
    }

    impl Writer {
        fn record(&mut self, call: &'static str) -> Result<(), ()> {
            self.calls.push(call);
            if self.calls.len() == self.fail_at { Err(()) } else { Ok(()) }
        }
    }

    impl ResponseWriter for Writer {
        async fn send_response(&mut self, response: Response<()>) -> Result<(), ()> {
            assert_eq!(response.status(), http::StatusCode::CREATED);
            assert_eq!(response.headers()["x-response"], "preserved");
            self.record("response")
        }

        async fn send_data(&mut self, data: bytes::Bytes) -> Result<(), ()> {
            match self.calls.len() {
                1 => assert_eq!(data.as_ref(), b"first"),
                2 => assert_eq!(data.as_ref(), b"second"),
                _ => panic!("data must follow the response and precede trailers"),
            }
            self.record("data")
        }

        async fn send_trailers(&mut self, trailers: http::HeaderMap) -> Result<(), ()> {
            assert_eq!(trailers["x-trailer"], "last");
            self.record("trailers")
        }

        async fn finish(&mut self) -> Result<(), ()> {
            self.record("finish")
        }
    }

    fn response() -> Response<()> {
        let mut response = Response::new(());
        *response.status_mut() = http::StatusCode::CREATED;
        response.headers_mut().insert("x-response", HeaderValue::from_static("preserved"));
        response
    }

    fn frames() -> Vec<ResponseFrame> {
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-trailer", HeaderValue::from_static("last"));
        vec![
            ResponseFrame::Data(bytes::Bytes::from_static(b"first")),
            ResponseFrame::Data(bytes::Bytes::from_static(b"second")),
            ResponseFrame::Trailers(trailers),
        ]
    }

    #[test]
    fn november4_http3_response_stops_at_each_exact_failed_operation() {
        let expected = ["response", "data", "data", "trailers", "finish"];
        for fail_at in 0..=expected.len() {
            let mut writer = Writer {
                fail_at,
                ..Writer::default()
            };
            {
                let mut future = pin!(send_response_frames(&mut writer, response(), frames(), false));
                let mut cx = Context::from_waker(Waker::noop());
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(fail_at == 0));
            }
            assert_eq!(writer.calls, expected[..if fail_at == 0 { expected.len() } else { fail_at }]);
        }
    }

    #[test]
    fn november4_http3_stall_sends_all_frames_but_never_finishes() {
        let mut writer = Writer::default();
        {
            let mut future = pin!(send_response_frames(&mut writer, response(), frames(), true));
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        }
        assert_eq!(writer.calls, ["response", "data", "data", "trailers"]);
    }
}

#[cfg(test)]
mod gamma_survivor_tests {
    use std::cell::Cell;
    use std::thread;

    use super::super::startup::gamma_round2::{GammaLauncher, tls_config};
    use super::*;

    #[test]
    fn gamma_round2_http3_endpoint_configures_localhost_h3_and_ephemeral_loopback() {
        let configured = Cell::new(0);
        let endpoint = create_endpoint_with(
            |names| {
                assert_eq!(names, ["localhost"]);
                configured.set(configured.get() + 1);
                tls_config()
            },
            |tls, bind| {
                assert_eq!(tls.alpn_protocols, [b"h3".to_vec()]);
                assert_eq!(bind, "127.0.0.1:0".parse().unwrap());
                71
            },
        );
        assert_eq!(endpoint, 71);
        assert_eq!(configured.get(), 1);
    }

    #[test]
    fn gamma_round2_http3_startup_announces_address_and_retains_worker_state_and_shutdown() {
        let address: SocketAddr = "127.0.0.1:5123".parse().unwrap();
        let worker_state = Arc::new(Mutex::new(None));
        let shutdown_receiver = Arc::new(Mutex::new(None));
        let served = Arc::new(AtomicUsize::new(0));
        let captured_state = Arc::clone(&worker_state);
        let captured_receiver = Arc::clone(&shutdown_receiver);
        let captured_served = Arc::clone(&served);
        let mut startup = Http3Server::start_scripted_with(
            ResponseScript::Sequence(vec![ResponsePlan::status(http::StatusCode::ACCEPTED)]),
            move || (73, address),
            move |endpoint, state, receiver| {
                assert_eq!(endpoint, 73);
                *captured_state.lock().unwrap() = Some(state);
                *captured_receiver.lock().unwrap() = Some(receiver);
                async move {
                    captured_served.fetch_add(1, Ordering::SeqCst);
                }
            },
            GammaLauncher {
                name: "fetch-winhttp-http3-test-server",
                worker: 79,
            },
        );
        assert_eq!(served.load(Ordering::SeqCst), 1);
        assert_eq!(startup.address, address);
        assert_eq!(startup.thread.take(), Some(79));
        assert!(Arc::ptr_eq(&startup.state, worker_state.lock().unwrap().as_ref().unwrap()));
        assert_eq!(startup.state.next_response.load(Ordering::SeqCst), 0);
        assert_eq!(startup.state.connections.load(Ordering::SeqCst), 0);
        assert!(startup.state.requests.lock().unwrap().is_empty());
        assert_eq!(startup.state.script.plan(0).status, http::StatusCode::ACCEPTED);
        startup.shutdown.take().unwrap().send(()).unwrap();
        assert_eq!(shutdown_receiver.lock().unwrap().as_mut().unwrap().try_recv(), Ok(()));
    }

    #[test]
    fn gamma_round2_http3_plan_preparation_records_exact_requests_and_sequence() {
        let mut server = stopped_server(None);
        Arc::get_mut(&mut server.state).unwrap().script = ResponseScript::Sequence(vec![
            ResponsePlan::status(http::StatusCode::CREATED),
            ResponsePlan::status(http::StatusCode::ACCEPTED),
        ]);
        let request = http::Request::builder()
            .method("PATCH")
            .uri("/resource?q=7")
            .header("x-request", "marker")
            .body(())
            .unwrap();
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-trailer", HeaderValue::from_static("terminal"));
        for expected in [http::StatusCode::CREATED, http::StatusCode::ACCEPTED] {
            let (response, frames, stall) = prepare_response(
                &server.state,
                &request,
                bytes::Bytes::from_static(b"payload"),
                Some(trailers.clone()),
            );
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()[CONTENT_LENGTH], "0");
            assert_eq!(frames.len(), 1);
            assert!(!stall);
        }
        assert_eq!(server.state.next_response.load(Ordering::SeqCst), 2);
        let recorded = server.state.requests.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        for actual in recorded.iter() {
            assert_eq!(actual.method, http::Method::PATCH);
            assert_eq!(actual.uri, "/resource?q=7");
            assert_eq!(actual.version, Version::HTTP_3);
            assert_eq!(actual.headers["x-request"], "marker");
            assert_eq!(actual.body.as_ref(), b"payload");
            assert_eq!(actual.trailers.as_ref().unwrap(), &trailers);
        }
    }

    #[test]
    fn gamma_round2_http3_framing_distinguishes_stalls_and_existing_lengths() {
        for (plan, expected, stalled) in [
            (ResponsePlan::ok("abc"), Some("3"), false),
            (ResponsePlan::ok("abc").stall_after_frames(), None, true),
            (
                ResponsePlan::ok("abc").header(CONTENT_LENGTH, HeaderValue::from_static("19")),
                Some("19"),
                false,
            ),
            (
                ResponsePlan::ok("abc")
                    .header(CONTENT_LENGTH, HeaderValue::from_static("19"))
                    .stall_after_frames(),
                Some("19"),
                true,
            ),
        ] {
            let mut server = stopped_server(None);
            Arc::get_mut(&mut server.state).unwrap().script = ResponseScript::Repeat(plan);
            let (response, frames, stall) = prepare_response(&server.state, &http::Request::new(()), bytes::Bytes::new(), None);
            assert_eq!(
                response.headers().get(CONTENT_LENGTH).map(|value| value.to_str().unwrap()),
                expected
            );
            assert_eq!(stall, stalled);
            assert!(matches!(frames.as_slice(), [ResponseFrame::Data(data)] if data.as_ref() == b"abc"));
            assert!(server.state.requests.lock().unwrap().is_empty());
        }
    }

    fn stopped_server(shutdown: Option<oneshot::Sender<()>>) -> Http3Server {
        Http3Server {
            address: SocketAddr::from(([127, 0, 0, 1], 8123)),
            state: Arc::new(State {
                script: ResponseScript::Sequence(Vec::new()),
                next_response: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                connections: AtomicUsize::new(0),
            }),
            shutdown,
            thread: None,
        }
    }

    #[test]
    fn gamma_round2_http3_finish_waits_before_taking_snapshot() {
        let (sender, receiver) = oneshot::channel();
        let mut server = stopped_server(Some(sender));
        let state = Arc::clone(&server.state);
        server.thread = Some(thread::spawn(move || {
            futures::executor::block_on(receiver).unwrap();
            state.connections.store(9, Ordering::SeqCst);
        }));
        assert_eq!(server.finish().connections, 9);
    }

    #[test]
    fn gamma_round2_http3_stop_propagates_a_worker_panic() {
        let mut server = stopped_server(None);
        server.thread = Some(thread::spawn(|| panic!("gamma_round2_worker")));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.stop())).is_err());
        assert!(server.thread.is_none());
    }

    #[test]
    fn gamma_survivor_stopped_http3_url_and_shutdown_sender_are_exact() {
        let (sender, mut receiver) = oneshot::channel();
        let mut server = stopped_server(Some(sender));
        assert_eq!(server.url("/h3?q=1"), "https://localhost:8123/h3?q=1");
        server.stop();
        assert!(server.shutdown.is_none());
        assert_eq!(receiver.try_recv(), Ok(()));
        server.stop();
        let (sender, mut receiver) = oneshot::channel();
        drop(stopped_server(Some(sender)));
        assert_eq!(receiver.try_recv(), Ok(()));
    }
}
