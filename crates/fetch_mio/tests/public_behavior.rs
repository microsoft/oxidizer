// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavior for the Arty Mio connector and Hyper stream.

#![allow(clippy::unwrap_used, reason = "test code")]

testing_aids::init_tracing!();

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::Duration;

use anyspawn::Spawner;
use arty::task::Builtins;
use arty_mio::{MioContext, MioOptions};
use fetch_hyper::{HyperIo, HyperTransportBuilder};
use fetch_mio::{MioConnector, MioConnectorOptions, MioStream};
use fetch_options::{RequestFilter, SocketOptions, TransportOptions};
use fetch_tls::TlsBackend;
use http::StatusCode;
use http_extensions::{HttpBodyBuilder, HttpRequestBuilder};
use layered::Service as _;
use ohno::Labeled as _;
use rcgen::{CertificateParams, KeyPair, PKCS_RSA_SHA256};
use seatbelt::{Recovery as _, RecoveryKind};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use templated_uri::BaseUri;
use thread_aware::{Thread, ThreadAware};

assert_impl_all!(MioConnector: Clone, Send, Sync);
assert_impl_all!(MioConnectorOptions: Clone, Send, Sync);
assert_impl_all!(MioStream: Send, Unpin, HyperIo);
assert_impl_all!(MioOptions: Clone, Send, Sync, Default);
assert_not_impl_any!(MioConnector: ThreadAware);
assert_not_impl_any!(MioConnectorOptions: Copy);
assert_not_impl_any!(MioOptions: Copy);
assert_not_impl_any!(MioStream: ThreadAware);

#[arty::test]
async fn connector_rejects_unsupported_scheme(builtins: Builtins) {
    let error = connector(&builtins, SocketOptions::default())
        .execute(BaseUri::from_static("ftp://127.0.0.1:21"))
        .await
        .unwrap_err();

    assert_eq!(error.label().as_str(), "scheme_not_allowed");
    assert_eq!(error.recovery().kind(), RecoveryKind::Never);
    assert!(error.to_string().contains("ftp"));
}

#[arty::test]
async fn spawner_preserves_bound_and_anywhere_semantics(builtins: Builtins) {
    let worker = builtins.thread().id();
    let runtime = Spawner::new_arty(builtins.scheduler().clone());

    let bound = runtime.spawn(async { std::thread::current().id() }).await;
    assert_eq!(bound, worker);

    let (relocated, destination, actual) = runtime
        .spawn_anywhere(RelocationProbe::default(), |probe| async move {
            (probe.relocated, probe.destination, std::thread::current().id())
        })
        .await;
    assert!(relocated);
    assert_eq!(destination, Some(actual));

    let blocking = runtime.spawn_blocking(|| std::thread::current().id()).await;
    assert_ne!(blocking, worker);
}

#[arty::test]
async fn hyper_transport_accepts_mio_stream_with_native_tls_path(builtins: Builtins) {
    let (port, certificate, server) = tls_server();
    let mut builder = native_tls::TlsConnector::builder();
    builder.add_root_certificate(native_tls::Certificate::from_der(certificate.as_ref()).unwrap());
    let response = request(&builtins, format!("https://127.0.0.1:{port}/"), builder.build().unwrap().into()).await;

    assert_eq!(response.status(), StatusCode::OK);
    server.join().unwrap();
}

#[arty::test]
async fn hyper_transport_accepts_mio_stream_with_rustls_path(builtins: Builtins) {
    let (port, certificate, server) = tls_server();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate).unwrap();
    let tls: TlsBackend = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth()
        .into();
    let response = request(&builtins, format!("https://127.0.0.1:{port}/"), tls).await;

    assert_eq!(response.status(), StatusCode::OK);
    server.join().unwrap();
}

async fn request(builtins: &Builtins, uri: String, tls: TlsBackend) -> http_extensions::HttpResponse {
    let mut options = TransportOptions::default();
    options.request_filter = RequestFilter::HttpAndHttps;
    options.connect_timeout = Duration::from_secs(5);
    options.supported_http_versions = vec![http::Version::HTTP_11];

    let handler = HyperTransportBuilder::new(
        connector(builtins, SocketOptions::default()),
        Spawner::new_arty(builtins.scheduler().clone()),
        builtins.clock().clone(),
        options,
    )
    .body_builder(HttpBodyBuilder::new_fake())
    .build(tls);
    let request = HttpRequestBuilder::new(&HttpBodyBuilder::new_fake()).get(uri).build().unwrap();
    handler.execute(request).await.unwrap()
}

fn connector(builtins: &Builtins, socket: SocketOptions) -> MioConnector {
    MioConnector::new(
        builtins.io().context::<MioContext>(&MioOptions::default()),
        builtins.scheduler().clone(),
        builtins.clock().clone(),
        MioConnectorOptions::default().socket(socket),
    )
}

fn tls_server() -> (u16, rustls::pki_types::CertificateDer<'static>, std::thread::JoinHandle<()>) {
    let key = KeyPair::generate_for(&PKCS_RSA_SHA256).unwrap();
    let certificate = CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let certificate_der = certificate.der().clone();
    let private_key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut configuration = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate_der.clone()], private_key)
        .unwrap();
    configuration.alpn_protocols = vec![b"http/1.1".to_vec()];

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(configuration)).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, stream);
        let mut request = Vec::new();
        let mut buffer = [0; 1_024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let length = stream.read(&mut buffer).unwrap();
            assert_ne!(length, 0);
            request.extend_from_slice(&buffer[..length]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
            .unwrap();
        stream.flush().unwrap();
    });
    (port, certificate_der, server)
}

#[derive(Clone, Default)]
struct RelocationProbe {
    relocated: bool,
    destination: Option<ThreadId>,
}

impl ThreadAware for RelocationProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.relocated = true;
        self.destination = Some(destination.id());
    }
}
