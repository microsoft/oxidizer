// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process connector and stream fakes for downstream tests.

use std::fmt;
use std::io::{Cursor, Read as _, Write as _};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use rcgen::{CertificateParams, KeyPair, PKCS_RSA_SHA256};
use templated_uri::BaseUri;

/// A connector that returns a stream with one raw HTTP response.
#[derive(Clone, Debug)]
pub struct ScriptedConnector {
    response: Bytes,
}

impl ScriptedConnector {
    /// Creates a connector that returns `response` after Hyper writes a request.
    #[must_use]
    pub fn new(response: impl Into<Bytes>) -> Self {
        Self { response: response.into() }
    }
}

impl layered::Service<BaseUri> for ScriptedConnector {
    type Out = http_extensions::Result<ScriptedStream>;

    fn execute(&self, _input: BaseUri) -> impl Future<Output = Self::Out> + Send {
        std::future::ready(Ok(ScriptedStream {
            response: Some(self.response.clone()),
            state: Arc::new(Mutex::new(StreamState {
                request_received: false,
                read_waker: None,
            })),
        }))
    }
}

/// An in-process Hyper stream that returns one raw HTTP response.
pub struct ScriptedStream {
    response: Option<Bytes>,
    state: Arc<Mutex<StreamState>>,
}

#[derive(Debug)]
struct StreamState {
    request_received: bool,
    read_waker: Option<Waker>,
}

impl fmt::Debug for ScriptedStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Read for ScriptedStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        let mut state = self
            .state
            .lock()
            .expect("scripted stream state cannot be poisoned because no user code runs while the lock is held");
        if !state.request_received {
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        state.read_waker = None;
        drop(state);

        if let Some(mut response) = self.as_mut().response.take() {
            let length = response.len().min(buf.remaining());
            buf.put_slice(&response.split_to(length));
            if !response.is_empty() {
                self.as_mut().response = Some(response);
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl Write for ScriptedStream {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let mut state = self
            .state
            .lock()
            .expect("scripted stream state cannot be poisoned because no user code runs while the lock is held");
        let first_write = !state.request_received;
        state.request_received = true;
        if first_write && let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Connection for ScriptedStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

/// A certificate generated for an in-process TLS server.
#[derive(Clone, Debug)]
pub struct ScriptedCertificate {
    der: Vec<u8>,
}

impl ScriptedCertificate {
    /// Returns the DER-encoded certificate.
    #[must_use]
    pub fn as_der(&self) -> &[u8] {
        &self.der
    }
}

/// A connector that drives a rustls server inside its returned stream.
#[derive(Clone)]
pub struct ScriptedTlsConnector {
    configuration: Arc<rustls::ServerConfig>,
    response: Bytes,
}

impl fmt::Debug for ScriptedTlsConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("response_len", &self.response.len())
            .finish_non_exhaustive()
    }
}

impl ScriptedTlsConnector {
    /// Creates a connector with a self-signed certificate for `127.0.0.1`.
    ///
    /// # Errors
    ///
    /// Returns an error if certificate or TLS server configuration fails.
    pub fn self_signed(response: impl AsRef<[u8]>) -> std::io::Result<(Self, ScriptedCertificate)> {
        let key = KeyPair::generate_for(&PKCS_RSA_SHA256).map_err(std::io::Error::other)?;
        let certificate = CertificateParams::new(vec!["127.0.0.1".to_owned()])
            .map_err(std::io::Error::other)?
            .self_signed(&key)
            .map_err(std::io::Error::other)?;
        let certificate_der = certificate.der().clone();
        let private_key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut configuration = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(std::io::Error::other)?
            .with_no_client_auth()
            .with_single_cert(vec![certificate_der.clone()], private_key)
            .map_err(std::io::Error::other)?;
        configuration.alpn_protocols = vec![b"http/1.1".to_vec()];

        Ok((
            Self {
                configuration: Arc::new(configuration),
                response: Bytes::copy_from_slice(response.as_ref()),
            },
            ScriptedCertificate {
                der: certificate_der.as_ref().to_vec(),
            },
        ))
    }
}

impl layered::Service<BaseUri> for ScriptedTlsConnector {
    type Out = http_extensions::Result<ScriptedTlsStream>;

    fn execute(&self, _input: BaseUri) -> impl Future<Output = Self::Out> + Send {
        let result = rustls::ServerConnection::new(Arc::clone(&self.configuration))
            .map(|connection| ScriptedTlsStream {
                connection,
                response: Some(self.response.clone()),
                request: Vec::new(),
                outbound: Vec::new(),
                read_waker: None,
                response_sent: false,
            })
            .map_err(|error| std::io::Error::other(error).into());
        std::future::ready(result)
    }
}

/// An in-process stream that acts as a rustls HTTP server.
pub struct ScriptedTlsStream {
    connection: rustls::ServerConnection,
    response: Option<Bytes>,
    request: Vec<u8>,
    outbound: Vec<u8>,
    read_waker: Option<Waker>,
    response_sent: bool,
}

impl fmt::Debug for ScriptedTlsStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("request_len", &self.request.len())
            .field("outbound_len", &self.outbound.len())
            .field("response_sent", &self.response_sent)
            .finish_non_exhaustive()
    }
}

impl ScriptedTlsStream {
    fn process_client_bytes(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let consumed = self.connection.read_tls(&mut Cursor::new(bytes))?;
        self.connection.process_new_packets().map_err(std::io::Error::other)?;

        let mut plaintext = [0_u8; 1_024];
        loop {
            match self.connection.reader().read(&mut plaintext) {
                Ok(0) => break,
                Ok(length) => self.request.extend_from_slice(&plaintext[..length]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }

        if !self.response_sent
            && self.request.windows(4).any(|window| window == b"\r\n\r\n")
            && let Some(response) = self.response.take()
        {
            self.connection.writer().write_all(&response)?;
            self.connection.send_close_notify();
            self.response_sent = true;
        }

        while self.connection.wants_write() {
            let written = self.connection.write_tls(&mut self.outbound)?;
            if written == 0 {
                break;
            }
        }

        if !self.outbound.is_empty()
            && let Some(waker) = self.read_waker.take()
        {
            waker.wake();
        }

        Ok(consumed)
    }
}

impl Read for ScriptedTlsStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        if !self.outbound.is_empty() {
            let length = self.outbound.len().min(buf.remaining());
            buf.put_slice(&self.outbound[..length]);
            self.outbound.drain(..length);
            return Poll::Ready(Ok(()));
        }

        if self.response_sent {
            return Poll::Ready(Ok(()));
        }

        self.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Write for ScriptedTlsStream {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.process_client_bytes(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Connection for ScriptedTlsStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Waker};

    use hyper::rt::{Read as _, Write as _};
    use layered::Service as _;

    use super::ScriptedConnector;

    #[test]
    fn scripted_response_is_retained_across_small_reads() {
        let connector = ScriptedConnector::new(&b"abcdefgh"[..]);
        let mut stream = futures::executor::block_on(connector.execute(templated_uri::BaseUri::from_static("http://example.com"))).unwrap();
        let mut context = Context::from_waker(Waker::noop());
        assert!(pin!(&mut stream).poll_write(&mut context, b"GET / HTTP/1.1\r\n\r\n").is_ready());

        let mut first_bytes = [0_u8; 3];
        let mut first = hyper::rt::ReadBuf::new(&mut first_bytes);
        assert!(pin!(&mut stream).poll_read(&mut context, first.unfilled()).is_ready());
        assert_eq!(first.filled(), b"abc");

        let mut second_bytes = [0_u8; 5];
        let mut second = hyper::rt::ReadBuf::new(&mut second_bytes);
        assert!(pin!(&mut stream).poll_read(&mut context, second.unfilled()).is_ready());
        assert_eq!(second.filled(), b"defgh");
    }
}
