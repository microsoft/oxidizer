// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Probes `WinHTTP`'s separation of DNS resolution from TLS server identity.

#[cfg(not(windows))]
fn main() {
    eprintln!("This WinHTTP experiment only runs on Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use std::convert::Infallible;
    use std::ffi::c_void;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::ptr;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use anyhow::{Context, Result, anyhow, bail, ensure};
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use rcgen::{CertifiedKey as GeneratedCertificate, generate_simple_self_signed};
    use rustls::crypto::ring::sign::any_supported_type;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use tokio_rustls::TlsAcceptor;
    use windows_sys::Win32::Networking::WinHttp::{
        ERROR_WINHTTP_INVALID_OPTION, ERROR_WINHTTP_SECURE_CERT_CN_INVALID, ERROR_WINHTTP_SECURE_FAILURE, SECURITY_FLAG_IGNORE_UNKNOWN_CA,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_ADDREQ_FLAG_REPLACE, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_HTTP_PROTOCOL_USED,
        WINHTTP_OPTION_RESOLUTION_HOSTNAME, WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_PROTOCOL_FLAG_HTTP2, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    };

    const LOGICAL_HOST: &str = "winhttp-resolution.invalid";
    const RESOLUTION_HOST: &str = "localhost";

    pub(super) fn run() -> Result<()> {
        rustls::crypto::ring::default_provider()
            .install_default()
            .map_err(|provider| anyhow!("a rustls crypto provider is already installed: {provider:?}"))?;

        let positive = run_positive_case()?;
        println!(
            "positive: status={}, protocol={}, SNI={}, :authority={}",
            positive.status,
            positive.protocol,
            positive.sni.as_deref().unwrap_or("<none>"),
            positive.http_authority.as_deref().unwrap_or("<none>")
        );

        ensure!(positive.status == 200, "positive request returned HTTP {}", positive.status);
        ensure!(
            positive.protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
            "positive request did not negotiate HTTP/2"
        );
        ensure!(
            positive.sni.as_deref() == Some(LOGICAL_HOST),
            "positive request sent unexpected SNI"
        );
        ensure!(
            positive
                .http_authority
                .as_deref()
                .is_some_and(|authority| authority.starts_with(LOGICAL_HOST)),
            "positive request sent unexpected HTTP/2 :authority"
        );

        let authority_override = run_authority_override_case()?;
        println!(
            "authority override: status={}, protocol={}, SNI={}, :authority={}",
            authority_override.status,
            authority_override.protocol,
            authority_override.sni.as_deref().unwrap_or("<none>"),
            authority_override.http_authority.as_deref().unwrap_or("<none>")
        );
        ensure!(
            authority_override.status == 200 && authority_override.protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
            "authority-override request did not complete over HTTP/2"
        );
        ensure!(
            authority_override.sni.as_deref() == Some(LOGICAL_HOST),
            "authority-override request changed the TLS SNI"
        );
        ensure!(
            authority_override
                .http_authority
                .as_deref()
                .is_some_and(|authority| authority.starts_with(RESOLUTION_HOST)),
            "Host replacement did not become the HTTP/2 :authority"
        );

        let negative = run_negative_case()?;
        println!(
            "negative: WinHTTP error={}, SNI={}, HTTP request sent={}",
            negative.winhttp_error,
            negative.sni.as_deref().unwrap_or("<none>"),
            negative.http_authority.is_some()
        );

        ensure!(
            matches!(
                negative.winhttp_error,
                ERROR_WINHTTP_SECURE_CERT_CN_INVALID | ERROR_WINHTTP_SECURE_FAILURE
            ),
            "negative control failed with unexpected WinHTTP error {}",
            negative.winhttp_error
        );
        ensure!(
            negative.sni.as_deref() == Some(RESOLUTION_HOST),
            "negative control sent unexpected SNI"
        );
        ensure!(
            negative.http_authority.is_none(),
            "negative control sent an HTTP request despite hostname validation failure"
        );

        println!(
            "PASS: WinHTTP resolved {LOGICAL_HOST} through {RESOLUTION_HOST} while using \
             {LOGICAL_HOST} for SNI and certificate hostname validation. A replacement Host header \
             independently controlled the HTTP/2 :authority."
        );
        Ok(())
    }

    fn run_positive_case() -> Result<PositiveResult> {
        let server = Http2TestServer::start(LOGICAL_HOST)?;
        let client = WinHttpClient::open()?;
        let response = client
            .get(LOGICAL_HOST, server.port(), Some(RESOLUTION_HOST), None, true)
            .context("positive WinHTTP request failed")?;
        drop(client);
        let observation = server.join()?;

        Ok(PositiveResult {
            status: response.status,
            protocol: response.protocol,
            sni: observation.sni,
            http_authority: observation.http_authority,
        })
    }

    fn run_authority_override_case() -> Result<PositiveResult> {
        let server = Http2TestServer::start(LOGICAL_HOST)?;
        let authority = format!("{RESOLUTION_HOST}:{}", server.port());
        let client = WinHttpClient::open()?;
        let response = client
            .get(LOGICAL_HOST, server.port(), Some(RESOLUTION_HOST), Some(&authority), true)
            .context("authority-override WinHTTP request failed")?;
        drop(client);
        let observation = server.join()?;

        Ok(PositiveResult {
            status: response.status,
            protocol: response.protocol,
            sni: observation.sni,
            http_authority: observation.http_authority,
        })
    }

    fn run_negative_case() -> Result<NegativeResult> {
        let server = TestServer::start(LOGICAL_HOST)?;
        let client = WinHttpClient::open()?;
        let error = client
            .get(RESOLUTION_HOST, server.port(), None, None, false)
            .expect_err("hostname mismatch unexpectedly succeeded");
        let winhttp_error = error
            .downcast_ref::<WinHttpError>()
            .context("negative control did not return a WinHTTP error")?
            .code;
        let observation = server.join()?;

        Ok(NegativeResult {
            winhttp_error,
            sni: observation.sni,
            http_authority: observation.http_authority,
        })
    }

    struct PositiveResult {
        status: u32,
        protocol: u32,
        sni: Option<String>,
        http_authority: Option<String>,
    }

    struct NegativeResult {
        winhttp_error: u32,
        sni: Option<String>,
        http_authority: Option<String>,
    }

    #[derive(Default)]
    struct Observation {
        sni: Option<String>,
        http_authority: Option<String>,
    }

    struct TestServer {
        port: u16,
        thread: JoinHandle<Result<Observation>>,
    }

    impl TestServer {
        fn start(certificate_name: &str) -> Result<Self> {
            let observed_sni = Arc::new(Mutex::new(None));
            let config = server_config(certificate_name, Arc::clone(&observed_sni), Vec::new())?;
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();

            let thread = thread::spawn(move || serve_one(&listener, config, &observed_sni));
            Ok(Self { port, thread })
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn join(self) -> Result<Observation> {
            self.thread.join().map_err(|_panic| anyhow!("TLS server thread panicked"))?
        }
    }

    struct Http2TestServer {
        port: u16,
        thread: JoinHandle<Result<Observation>>,
    }

    impl Http2TestServer {
        fn start(certificate_name: &str) -> Result<Self> {
            let observed_sni = Arc::new(Mutex::new(None));
            let config = server_config(certificate_name, Arc::clone(&observed_sni), vec![b"h2".to_vec()])?;
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            listener.set_nonblocking(true)?;

            let thread = thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()?
                    .block_on(serve_http2(listener, config, observed_sni))
            });
            Ok(Self { port, thread })
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn join(self) -> Result<Observation> {
            self.thread.join().map_err(|_panic| anyhow!("HTTP/2 TLS server thread panicked"))?
        }
    }

    fn server_config(
        certificate_name: &str,
        observed_sni: Arc<Mutex<Option<String>>>,
        alpn_protocols: Vec<Vec<u8>>,
    ) -> Result<ServerConfig> {
        let GeneratedCertificate { cert, signing_key } = generate_simple_self_signed(vec![certificate_name.to_owned()])?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let signing_key = any_supported_type(&private_key)?;
        let resolver = Arc::new(RecordingResolver {
            certified_key: Arc::new(CertifiedKey::new(vec![CertificateDer::from(cert.der().to_vec())], signing_key)),
            observed_sni,
        });
        let mut config = ServerConfig::builder().with_no_client_auth().with_cert_resolver(resolver);
        config.alpn_protocols = alpn_protocols;
        Ok(config)
    }

    #[derive(Debug)]
    struct RecordingResolver {
        certified_key: Arc<CertifiedKey>,
        observed_sni: Arc<Mutex<Option<String>>>,
    }

    impl ResolvesServerCert for RecordingResolver {
        fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            *self.observed_sni.lock().expect("SNI recorder poisoned") = client_hello.server_name().map(ToOwned::to_owned);
            Some(Arc::clone(&self.certified_key))
        }
    }

    fn serve_one(listener: &TcpListener, config: ServerConfig, observed_sni: &Arc<Mutex<Option<String>>>) -> Result<Observation> {
        let (stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let mut tls = StreamOwned::new(ServerConnection::new(Arc::new(config))?, stream);
        let mut request = Vec::new();
        let read_result = read_http_headers(&mut tls, &mut request);

        let http_authority = match read_result {
            Ok(()) => {
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")?;
                parse_host_header(&request)
            }
            Err(error) if request.is_empty() => {
                eprintln!("server observed TLS termination before HTTP: {error}");
                None
            }
            Err(error) => return Err(error),
        };

        let sni = observed_sni.lock().expect("SNI recorder poisoned").clone();
        Ok(Observation { sni, http_authority })
    }

    async fn serve_http2(listener: TcpListener, config: ServerConfig, observed_sni: Arc<Mutex<Option<String>>>) -> Result<Observation> {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (stream, _) = listener.accept().await?;
        let tls = TlsAcceptor::from(Arc::new(config)).accept(stream).await?;
        let observed_authority = Arc::new(Mutex::new(None));
        let service_authority = Arc::clone(&observed_authority);
        let service = service_fn(move |request: Request<Incoming>| {
            *service_authority.lock().expect("HTTP/2 authority recorder poisoned") = request.uri().authority().map(ToString::to_string);
            async { Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"OK")))) }
        });

        hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls), service)
            .await?;

        let sni = observed_sni.lock().expect("SNI recorder poisoned").clone();
        let http_authority = observed_authority.lock().expect("HTTP/2 authority recorder poisoned").clone();
        Ok(Observation { sni, http_authority })
    }

    fn read_http_headers(stream: &mut StreamOwned<ServerConnection, TcpStream>, request: &mut Vec<u8>) -> Result<()> {
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                bail!("connection closed before complete HTTP headers");
            }
            request.extend_from_slice(&buffer[..read]);
            ensure!(request.len() <= 64 * 1024, "HTTP headers exceeded 64 KiB");
        }
        Ok(())
    }

    fn parse_host_header(request: &[u8]) -> Option<String> {
        String::from_utf8_lossy(request)
            .lines()
            .find_map(|line| line.strip_prefix("Host: "))
            .map(ToOwned::to_owned)
    }

    #[derive(Debug)]
    struct WinHttpError {
        operation: &'static str,
        code: u32,
    }

    impl std::fmt::Display for WinHttpError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} failed with Win32 error {}", self.operation, self.code)
        }
    }

    impl std::error::Error for WinHttpError {}

    struct InternetHandle(*mut c_void);

    impl InternetHandle {
        fn new(handle: *mut c_void, operation: &'static str) -> Result<Self> {
            if handle.is_null() {
                return Err(last_error(operation));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for InternetHandle {
        fn drop(&mut self) {
            // SAFETY: The handle is non-null, owned by this wrapper, and closed exactly once here.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    struct WinHttpClient {
        session: InternetHandle,
    }

    #[derive(Debug)]
    struct WinHttpResponse {
        status: u32,
        protocol: u32,
    }

    impl WinHttpClient {
        fn open() -> Result<Self> {
            let agent = wide("fetch-winhttp-resolution-hostname-probe");
            // SAFETY: All pointers reference valid, null-terminated UTF-16 strings for the call.
            let session = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, ptr::null(), ptr::null(), 0) };
            Ok(Self {
                session: InternetHandle::new(session, "WinHttpOpen")?,
            })
        }

        fn get(
            &self,
            server_name: &str,
            port: u16,
            resolution_hostname: Option<&str>,
            http_host: Option<&str>,
            require_http2: bool,
        ) -> Result<WinHttpResponse> {
            let server_name = wide(server_name);
            // SAFETY: The session is live and the server-name pointer is valid for the call.
            let connection = unsafe { WinHttpConnect(self.session.0, server_name.as_ptr(), port, 0) };
            let connection = InternetHandle::new(connection, "WinHttpConnect")?;

            let verb = wide("GET");
            let path = wide("/");
            // SAFETY: The connection is live and all provided UTF-16 pointers remain valid.
            let request = unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    verb.as_ptr(),
                    path.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    WINHTTP_FLAG_SECURE,
                )
            };
            let request = InternetHandle::new(request, "WinHttpOpenRequest")?;

            let security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA;
            set_option(
                &request,
                WINHTTP_OPTION_SECURITY_FLAGS,
                (&raw const security_flags).cast(),
                size_of::<u32>().try_into()?,
                "WINHTTP_OPTION_SECURITY_FLAGS",
            )?;

            if require_http2 {
                let protocols = WINHTTP_PROTOCOL_FLAG_HTTP2;
                set_option(
                    &request,
                    WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
                    (&raw const protocols).cast(),
                    size_of::<u32>().try_into()?,
                    "WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL",
                )?;
                let required = 1_i32;
                set_option(
                    &request,
                    WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED,
                    (&raw const required).cast(),
                    size_of::<i32>().try_into()?,
                    "WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED",
                )?;
            }

            if let Some(hostname) = resolution_hostname {
                let hostname = wide(hostname);
                let byte_len = hostname.len() * size_of::<u16>();
                set_option(
                    &request,
                    WINHTTP_OPTION_RESOLUTION_HOSTNAME,
                    hostname.as_ptr().cast(),
                    byte_len.try_into()?,
                    "WINHTTP_OPTION_RESOLUTION_HOSTNAME",
                )
                .map_err(|error| {
                    if error
                        .downcast_ref::<WinHttpError>()
                        .is_some_and(|error| error.code == ERROR_WINHTTP_INVALID_OPTION)
                    {
                        anyhow!("WINHTTP_OPTION_RESOLUTION_HOSTNAME is unsupported on this Windows host")
                    } else {
                        error
                    }
                })?;
            }

            if let Some(host) = http_host {
                set_host_header(&request, host)?;
            }

            // SAFETY: The request is live; optional buffers are null because this GET has no body.
            if unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null(), 0, 0, 0) } == 0 {
                return Err(last_error("WinHttpSendRequest"));
            }

            // SAFETY: The request is live and the reserved argument is required to be null.
            if unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
                return Err(last_error("WinHttpReceiveResponse"));
            }

            let mut status = 0_u32;
            let mut status_size = size_of::<u32>().try_into()?;
            // SAFETY: The output pointers refer to initialized writable storage of the declared size.
            if unsafe {
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    ptr::null(),
                    (&raw mut status).cast(),
                    &raw mut status_size,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(last_error("WinHttpQueryHeaders"));
            }

            let protocol = query_option_u32(&request, WINHTTP_OPTION_HTTP_PROTOCOL_USED, "WINHTTP_OPTION_HTTP_PROTOCOL_USED")?;
            Ok(WinHttpResponse { status, protocol })
        }
    }

    fn set_host_header(request: &InternetHandle, host: &str) -> Result<()> {
        let header = wide(&format!("Host: {host}"));
        // SAFETY: The request is live and header is a valid null-terminated UTF-16 string.
        if unsafe {
            WinHttpAddRequestHeaders(
                request.0,
                header.as_ptr(),
                u32::MAX,
                WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
            )
        } == 0
        {
            return Err(last_error("WinHttpAddRequestHeaders(Host)"));
        }
        Ok(())
    }

    fn query_option_u32(handle: &InternetHandle, option: u32, operation: &'static str) -> Result<u32> {
        let mut value = 0_u32;
        let mut value_len = size_of::<u32>().try_into()?;
        // SAFETY: The handle is live and the output buffer has the declared writable size.
        if unsafe { WinHttpQueryOption(handle.0, option, (&raw mut value).cast(), &raw mut value_len) } == 0 {
            return Err(last_error(operation));
        }
        Ok(value)
    }

    fn set_option(handle: &InternetHandle, option: u32, value: *const c_void, value_len: u32, operation: &'static str) -> Result<()> {
        // SAFETY: The handle is live and value points to a buffer of value_len bytes for this call.
        if unsafe { WinHttpSetOption(handle.0, option, value, value_len) } == 0 {
            return Err(last_error(operation));
        }
        Ok(())
    }

    fn last_error(operation: &'static str) -> anyhow::Error {
        WinHttpError {
            operation,
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0).cast_unsigned(),
        }
        .into()
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
