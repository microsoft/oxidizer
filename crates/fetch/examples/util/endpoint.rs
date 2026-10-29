// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs network examples against an isolated HTTPS origin during automated checks.

#![allow(dead_code, reason = "individual examples select different TLS backends")]

use std::convert::Infallible;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use fetch_tls::TlsOptions;
use http::{Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;

pub(crate) const RESPONSE_BODY: &[u8] = b"automated example response";

pub(crate) struct Endpoint {
    pub(crate) url: String,
    pub(crate) root_certificate: Option<CertificateDer<'static>>,
    // This independent counter publishes no response data, so relaxed access is sufficient.
    requests: Arc<AtomicUsize>,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<io::Result<()>>>,
}

impl Endpoint {
    pub(crate) async fn new(original_url: &str) -> io::Result<Self> {
        let requests = Arc::new(AtomicUsize::new(0));
        if std::env::var_os("ANVIL_EXAMPLE").is_none() {
            return Ok(Self {
                url: original_url.to_owned(),
                root_certificate: None,
                requests,
                shutdown: None,
                server: None,
            });
        }

        let mut root_params = CertificateParams::default();
        root_params.distinguished_name.push(DnType::CommonName, "Anvil example root");
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let root = CertifiedIssuer::self_signed(root_params, KeyPair::generate().map_err(io::Error::other)?).map_err(io::Error::other)?;
        let key = KeyPair::generate().map_err(io::Error::other)?;
        let mut params = CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).map_err(io::Error::other)?;
        params.distinguished_name.push(DnType::CommonName, "localhost");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let certificate = params.signed_by(&key, &root).map_err(io::Error::other)?;
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .map_err(io::Error::other)?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let url = format!("https://{}/", listener.local_addr()?);
        let (shutdown, stopping) = oneshot::channel();
        let server = tokio::spawn(serve(
            listener,
            TlsAcceptor::from(Arc::new(config)),
            Arc::clone(&requests),
            stopping,
        ));
        Ok(Self {
            url,
            root_certificate: Some(root.der().clone()),
            requests,
            shutdown: Some(shutdown),
            server: Some(server),
        })
    }

    pub(crate) fn is_automated(&self) -> bool {
        self.root_certificate.is_some()
    }

    pub(crate) fn rustls_options(&self) -> io::Result<TlsOptions> {
        let Some(root) = &self.root_certificate else {
            return Ok(TlsOptions::new_rustls());
        };
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(
            rustls_platform_verifier::Verifier::new_with_extra_roots([root.clone()], Arc::clone(&provider)).map_err(io::Error::other)?,
        );
        Ok(TlsOptions::builder_rustls()
            .crypto_provider(provider)
            .server_certificate_verifier(move |_| Arc::clone(&verifier))
            .build())
    }

    pub(crate) fn native_tls_options(&self) -> io::Result<TlsOptions> {
        let Some(root) = &self.root_certificate else {
            return Ok(TlsOptions::new_native_tls());
        };
        Ok(TlsOptions::builder_native_tls()
            .root_certificate(native_tls::Certificate::from_der(root.as_ref()).map_err(io::Error::other)?)
            .build())
    }

    pub(crate) async fn finish(mut self, expected_requests: usize) -> io::Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            // The server may already have failed. Joining below preserves that error.
            let _ = shutdown.send(());
        }
        if let Some(server) = self.server.take() {
            server
                .await
                .map_err(io::Error::other)?
                .inspect_err(|error| eprintln!("automated HTTPS origin failed: {error:?}"))?;
            let actual = self.requests.load(Ordering::Relaxed);
            if actual != expected_requests {
                return Err(io::Error::other(format!(
                    "automated example sent {actual} requests; expected {expected_requests}"
                )));
            }
        }
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    requests: Arc<AtomicUsize>,
    mut stopping: oneshot::Receiver<()>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopping => break,
            finished = connections.join_next(), if !connections.is_empty() => {
                finished
                    .expect("this select branch is enabled only while the connection set is nonempty")
                    .map_err(io::Error::other)?;
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let acceptor = acceptor.clone();
                let requests = Arc::clone(&requests);
                connections.spawn(async move {
                    // Pools can abandon redundant connections. Client results and
                    // the final request count still detect failed operations.
                    let stream = match acceptor.accept(stream).await {
                        Ok(stream) => stream,
                        Err(error) => {
                            eprintln!("automated HTTPS peer disconnected during TLS: {error:?}");
                            return;
                        }
                    };
                    let service = service_fn(move |_: Request<Incoming>| {
                        requests.fetch_add(1, Ordering::Relaxed);
                        async { Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(RESPONSE_BODY)))) }
                    });
                    let builder = auto::Builder::new(TokioExecutor::new());
                    if let Err(error) = builder
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        eprintln!("automated HTTPS connection ended: {error:?}");
                    }
                });
            }
        }
    }
    connections.abort_all();
    while let Some(finished) = connections.join_next().await {
        match finished {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    Ok(())
}
