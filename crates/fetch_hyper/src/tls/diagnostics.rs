// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-connection TLS context. Never include a full URI, which can contain credentials.

use std::net::SocketAddr;
#[cfg(any(feature = "rustls", test))]
use std::sync::{Arc, Mutex};

use http::{Extensions, Uri};
use hyper_util::client::legacy::connect::{Connection, HttpInfo};

use crate::PeerAddr;
#[cfg(any(feature = "rustls", test))]
use crate::{Connect, HyperIo};

pub(crate) fn remote_address(stream: &impl Connection) -> Option<SocketAddr> {
    let mut extensions = Extensions::new();
    stream.connected().get_extras(&mut extensions);
    extensions
        .get::<PeerAddr>()
        .map(|address| address.0)
        .or_else(|| extensions.get::<HttpInfo>().map(HttpInfo::remote_addr))
}

pub(crate) fn context(destination: &Uri, validation_name: &str, name_override: Option<&str>, remote: Option<SocketAddr>) -> String {
    let host = destination.host().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    let port = destination.port_u16().unwrap_or(443);
    let remote = remote.map_or_else(
        || "unavailable (connector did not expose a connected peer address)".to_owned(),
        |addr| addr.to_string(),
    );
    format!(
        "TLS handshake context: scheme=https, request_hostname={host:?}, request_port={port}, \
         tls_validation_name={validation_name:?}, tls_server_name_override={name_override:?}, \
         server_name_override_applied={}, remote_address={remote}",
        name_override.is_some()
    )
}

#[cfg(any(feature = "native-tls", test))]
pub(crate) const NATIVE_CERTIFICATE_UNAVAILABLE: &str = "peer_certificate_subject=unavailable, peer_certificate_sans=unavailable, peer_certificate_sha256=unavailable \
     (the native TLS backend did not expose a certificate for this handshake failure)";

#[cfg(any(feature = "rustls", test))]
pub(crate) const RUSTLS_CERTIFICATE_UNAVAILABLE: &str = "peer_certificate_subject=unavailable, peer_certificate_sans=unavailable, peer_certificate_sha256=unavailable \
     (no certificate details exposed by the failed handshake; preconfigured rustls verifiers cannot be inspected)";

#[cfg(any(feature = "rustls", test))]
#[derive(Clone)]
pub(crate) struct DiagnosticConnector<C> {
    pub(crate) inner: C,
    pub(crate) remote: Arc<Mutex<Option<SocketAddr>>>,
}

#[cfg(any(feature = "rustls", test))]
impl<C, S> layered::Service<templated_uri::BaseUri> for DiagnosticConnector<C>
where
    C: Connect<S> + layered::Service<templated_uri::BaseUri, Out = http_extensions::Result<S>>,
    S: HyperIo,
{
    type Out = http_extensions::Result<S>;

    async fn execute(&self, input: templated_uri::BaseUri) -> Self::Out {
        let stream = self.inner.execute(input).await?;
        *self
            .remote
            .lock()
            .expect("address recording runs no user code while holding the mutex") = remote_address(&stream);
        Ok(stream)
    }
}

#[cfg(any(feature = "rustls", test))]
pub(crate) fn has_certificate_details(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error.is::<fetch_tls::CertificateVerificationError>() {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

// rustls::Error does not forward Error::source() for its Other variant.
// Remove only our added carrier, keeping the original rustls error reachable
// through the certificate error and retaining the surrounding I/O error kinds.
#[cfg(any(feature = "rustls", test))]
pub(crate) fn restore_rustls_source(error: Box<dyn std::error::Error + Send + Sync>) -> Box<dyn std::error::Error + Send + Sync> {
    let error = match error.downcast::<std::io::Error>() {
        Ok(error) if error.get_ref().is_some() => {
            let kind = error.kind();
            let inner = error.into_inner().expect("get_ref returned Some immediately above");
            return Box::new(std::io::Error::new(kind, restore_rustls_source(inner)));
        }
        Ok(error) => return error,
        Err(error) => error,
    };
    match error.downcast::<rustls::Error>() {
        Ok(error) => match *error {
            rustls::Error::Other(other) if other.0.is::<fetch_tls::CertificateVerificationError>() => {
                Box::new(SharedCertificateError(other.0))
            }
            other => Box::new(other),
        },
        Err(error) => error,
    }
}

#[cfg(any(feature = "rustls", test))]
#[derive(Debug)]
struct SharedCertificateError(Arc<dyn std::error::Error + Send + Sync>);

#[cfg(any(feature = "rustls", test))]
impl std::fmt::Display for SharedCertificateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(any(feature = "rustls", test))]
impl std::error::Error for SharedCertificateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_excludes_uri_credentials_path_and_query() {
        let destination: Uri = "https://user:credential@example.test/private?key=hidden".parse().unwrap();
        let message = context(&destination, "expected.example.test", Some("expected.example.test"), None);
        assert!(message.contains("request_hostname=\"example.test\""), "{message}");
        assert!(message.contains("request_port=443"), "{message}");
        assert!(message.contains("remote_address=unavailable"), "{message}");
        for excluded in ["user", "credential", "private", "hidden"] {
            assert!(!message.contains(excluded), "{message}");
        }
    }

    #[test]
    fn context_keeps_ipv6_and_explicit_ports() {
        let destination: Uri = "https://[::1]:8443".parse().unwrap();
        let remote = "[::1]:8443".parse().unwrap();
        let message = context(&destination, "::1", None, Some(remote));
        assert!(message.contains("request_hostname=\"::1\""), "{message}");
        assert!(message.contains("request_port=8443"), "{message}");
        assert!(message.contains("remote_address=[::1]:8443"), "{message}");
        assert!(message.contains("server_name_override_applied=false"), "{message}");
    }
}
