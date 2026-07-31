// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use bytes::{Buf as _, Bytes, BytesMut};
use h3_quinn::Connection;
use http::{HeaderMap, HeaderValue, Response, Version};
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, Incoming, ServerConfig, VarInt};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use super::server::{RecordedRequest, ServerSnapshot};

#[derive(Debug)]
struct State {
    responses: Vec<Bytes>,
    next_response: AtomicUsize,
    requests: Mutex<Vec<RecordedRequest>>,
    connections: AtomicUsize,
}

pub(crate) struct Http3Server {
    address: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Http3Server {
    pub(crate) fn start(responses: impl IntoIterator<Item = Bytes>) -> Self {
        let state = Arc::new(State {
            responses: responses.into_iter().collect(),
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
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("HTTP/3 test server Tokio runtime starts");
                runtime.block_on(async move {
                    let endpoint = create_endpoint();
                    let address = endpoint.local_addr().expect("HTTP/3 endpoint has a local address");
                    address_tx.send(address).expect("HTTP/3 server address is published");
                    run_server(endpoint, server_state, shutdown_rx).await;
                });
            })
            .expect("HTTP/3 test server thread starts");
        let address = address_rx.recv().expect("HTTP/3 test server publishes its address");

        Self {
            address,
            state,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.address.port())
    }

    pub(crate) fn finish(mut self) -> ServerSnapshot {
        self.stop();
        ServerSnapshot {
            requests: self
                .state
                .requests
                .lock()
                .expect("HTTP/3 request record lock is not poisoned")
                .clone(),
            connections: self.state.connections.load(Ordering::SeqCst),
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("HTTP/3 test server thread does not panic");
        }
    }
}

impl Drop for Http3Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn create_endpoint() -> Endpoint {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(["localhost".to_owned()]).expect("HTTP/3 test certificate generation succeeds");
    let private_key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], private_key)
        .expect("generated HTTP/3 certificate and key are compatible");
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = QuicServerConfig::try_from(tls).expect("rustls configuration supports QUIC");
    let server_config = ServerConfig::with_crypto(Arc::new(crypto));

    Endpoint::server(server_config, (Ipv4Addr::LOCALHOST, 0).into()).expect("HTTP/3 endpoint binds to loopback")
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
        state
            .requests
            .lock()
            .expect("HTTP/3 request record lock is not poisoned")
            .push(RecordedRequest {
                method: request.method().clone(),
                uri: request.uri().clone(),
                version: Version::HTTP_3,
                headers: request.headers().clone(),
                body: body.freeze(),
                trailers,
            });
        let response_body = state.responses.get(index).cloned().unwrap_or_default();
        let response = Response::builder()
            .status(200)
            .header("content-length", response_body.len())
            .body(())
            .expect("HTTP/3 response is valid");
        if stream.send_response(response).await.is_err() {
            return;
        }
        if stream.send_data(response_body).await.is_err() {
            return;
        }
        let mut trailers = HeaderMap::new();
        trailers.insert("x-trailer", HeaderValue::from_static("value"));
        if stream.send_trailers(trailers).await.is_err() {
            return;
        }
        if stream.finish().await.is_err() {
            return;
        }
    }
}
