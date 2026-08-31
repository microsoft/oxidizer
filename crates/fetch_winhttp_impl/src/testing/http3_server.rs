// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

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
        let state = Arc::new(State {
            script,
            next_response: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            connections: AtomicUsize::new(0),
        });
        let (address_tx, address_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let server_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("fetch-winhttp-http3-test-server".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
                runtime.block_on(async move {
                    let endpoint = create_endpoint();
                    let address = endpoint.local_addr().unwrap();
                    address_tx.send(address).unwrap();
                    run_server(endpoint, server_state, shutdown_rx).await;
                });
            })
            .unwrap();
        let address = address_rx.recv().unwrap();

        Self {
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
    let CertifiedKey { cert, signing_key } = generate_simple_self_signed(["localhost".to_owned()]).unwrap();
    let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], private_key)
        .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = QuicServerConfig::try_from(tls).unwrap();
    let server_config = ServerConfig::with_crypto(Arc::new(crypto));

    Endpoint::server(server_config, (Ipv4Addr::LOCALHOST, 0).into()).unwrap()
}

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

    endpoint.close(VarInt::from_u32(0), b"test shutdown");
    // Abort the in-flight connection tasks rather than waiting for them to observe the endpoint
    // closure. A task blocked in `h3` request handling would otherwise wedge the join loop, and
    // this loop runs on the shutdown path of `Http3Server::finish`/`Drop`, so a wedged task would
    // hang the test binary with no diagnostic instead of failing an assertion.
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    endpoint.wait_idle().await;
}

async fn serve_connection(incoming: Incoming, state: Arc<State>) {
    let Ok(connection) = incoming.await else {
        return;
    };
    let Ok(mut connection) = h3::server::builder().build(Connection::new(connection)).await else {
        return;
    };

    loop {
        let Ok(Some(resolver)) = connection.accept().await else {
            return;
        };
        let Ok((request, mut stream)) = resolver.resolve_request().await else {
            return;
        };
        let mut body = BytesMut::new();
        loop {
            match stream.recv_data().await {
                Ok(Some(mut data)) => {
                    let remaining = data.remaining();
                    body.extend_from_slice(&data.copy_to_bytes(remaining));
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
        let Ok(trailers) = stream.recv_trailers().await else {
            return;
        };
        let index = state.next_response.fetch_add(1, Ordering::SeqCst);
        if state.script.records() {
            state.requests.lock().unwrap().push(RecordedRequest {
                method: request.method().clone(),
                uri: request.uri().clone(),
                version: Version::HTTP_3,
                headers: request.headers().clone(),
                body: body.freeze(),
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
        if stream.send_response(response).await.is_err() {
            return;
        }
        for frame in frames {
            let sent = match frame {
                ResponseFrame::Data(data) => stream.send_data(data).await,
                ResponseFrame::Trailers(trailers) => stream.send_trailers(trailers).await,
            };
            if sent.is_err() {
                return;
            }
        }
        if stall_after_frames {
            // Never resolves. The connection task is aborted on fixture shutdown, so this holds
            // the response open without any timer or wall-clock deadline.
            std::future::pending::<()>().await;
        }
        if stream.finish().await.is_err() {
            return;
        }
    }
}
