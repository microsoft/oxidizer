// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Defines the compression handler and its configuration layer.

use std::num::NonZeroU64;

use bytesbuf::BytesView;
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, DecompressorLimits, Level, Resources};
use futures::FutureExt as _;
use futures::future::{Either, ready};
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE, TRANSFER_ENCODING, VARY};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpBody, HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{Layer, Service};
use mime::Mime;
use smallvec::SmallVec;

use crate::error::{CompressibleTypeError, invalid, too_many_content_codings, unsupported};
use crate::{CONTENT_DIGEST_HEADER, REPR_DIGEST_HEADER, body, negotiate};

/// A short format list because stacked `Content-Encoding` values are rare.
type Formats = SmallVec<[Format; 2]>;

/// Reserved bytes per advertised format, including separators and a quality value.
const ACCEPT_ENCODING_ENTRY_CAPACITY: usize = 12;

/// Limits decoder allocation and recursive depth for peer-controlled headers.
const MAX_CONTENT_ENCODING_LAYERS: usize = 16;

/// Maximum decompressed size accepted by a layer unless the caller overrides it.
// Use the exact byte count. Mutation testing can otherwise change one
// multiplication to an undocumented value that boundary tests cannot distinguish.
const DEFAULT_MAX_DECOMPRESSED_BODY_LEN: u64 = 67_108_864;

/// Maximum compressed streams decoded by each decompression stage unless the caller overrides it.
const DEFAULT_MAX_DECOMPRESSED_STREAMS: u64 = 1024;

/// Largest input compressed synchronously before the handler returns.
///
/// This matches the compressor's default 64 KiB output chunk. Larger input uses
/// the bounded streaming adapter so one handler poll cannot drain an unbounded body.
const MAX_DIRECT_COMPRESSION_INPUT_LEN: usize = 64 * 1024;

const fn default_decompressor_limits() -> DecompressorLimits {
    DecompressorLimits::new()
        .max_output_len(
            NonZeroU64::new(DEFAULT_MAX_DECOMPRESSED_BODY_LEN).expect("DEFAULT_MAX_DECOMPRESSED_BODY_LEN is a non-zero constant"),
        )
        .max_streams(NonZeroU64::new(DEFAULT_MAX_DECOMPRESSED_STREAMS).expect("DEFAULT_MAX_DECOMPRESSED_STREAMS is a non-zero constant"))
}

/// Policy for a compression format that is not enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UnsupportedCompression {
    /// Returns the compressed body with its `Content-Encoding` unchanged.
    ///
    /// The caller can inspect `Content-Encoding` to identify the compression
    /// format that was left in place.
    #[default]
    PassThrough,

    /// Fail with the `compression_unsupported` error label.
    ///
    /// Use this policy when the caller requires a decompressed body. It prevents
    /// the caller from parsing compressed bytes as payload.
    Fail,
}

/// Metadata for a body before decompression.
///
/// Decompression removes `Content-Encoding`. It replaces or removes
/// `Content-Length`. This value stores the original header information when the
/// layer sets up decompression.
///
/// Servers add it to decompressed request extensions. Clients add it to
/// decompressed response extensions. It does not store compressed body bytes.
#[derive(Debug, Clone)]
pub struct OriginalBody {
    formats: Formats,
    content_length: Option<u64>,
}

impl OriginalBody {
    /// The applied compression formats, in the order listed by `Content-Encoding`.
    ///
    /// Never empty. Multiple formats describe successive compression steps,
    /// not alternatives.
    #[must_use]
    pub fn formats(&self) -> &[Format] {
        &self.formats
    }

    /// The first `Content-Length` field value, if it can be parsed as a `u64`.
    ///
    /// This measures the compressed body, not its decompressed size.
    /// It is `None` if this value is absent, unreadable, or not a `u64`. The
    /// decompressed length is unknown until the complete body is read.
    #[must_use]
    pub const fn content_length(&self) -> Option<u64> {
        self.content_length
    }
}

/// Client-side configuration held by a [`CompressionLayer`].
///
/// A client can decompress received responses. Create this layer with
/// [`Compression::client`].
///
/// Server-side response compression is not available on a client layer:
///
/// ```compile_fail,E0599
/// # use http_compression::Compression;
/// Compression::client().compress_responses(&[]);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Client {
    decompress_responses: Formats,
    advertise: bool,
    accept_encoding: Option<HeaderValue>,
}

/// Server-side configuration held by a [`CompressionLayer`].
///
/// A server can decompress requests and compress responses. Response compression
/// selects a format that the caller accepts. Create this layer with
/// [`Compression::server`].
///
/// Client-side response decompression is not available on a server layer:
///
/// ```compile_fail,E0599
/// # use http_compression::Compression;
/// Compression::server().decompress_responses(&[]);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Server {
    decompress_requests: Formats,
    compress_responses: Formats,
    level: Level,
    compressible: Option<Vec<Mime>>,
}

#[derive(Debug, Clone)]
enum Role {
    Client(Client),
    Server(Server),
}

/// Compresses and decompresses message bodies around an inner handler.
///
/// The selected role controls the available transformations:
///
/// | Role | Configured with | Transforms |
/// |---|---|---|
/// | [`Client`] | [`decompress_responses`][CompressionLayer::decompress_responses] | the response coming back |
/// | [`Server`] | [`decompress_requests`][CompressionLayer::decompress_requests] | the request coming in |
/// | [`Server`] | [`compress_responses`][CompressionLayer::compress_responses] | the response going out |
///
/// Select the role with [`Compression::client`] or [`Compression::server`].
/// Each transformation is off until you configure it.
///
/// The layer compresses small buffered bodies before it passes the message to
/// the inner handler. A compression error for such a body fails the handler
/// call. Larger buffered bodies, streaming bodies, and decompression run when
/// the body is read. Their data errors fail that body read.
///
/// Apply its layer with [`Layer::layer`].
#[derive(Debug)]
pub struct Compression<T> {
    inner: T,
    config: Config,
    role: Role,
}

/// Creates [`Compression`] instances for role `R`.
///
/// The layer does not transform bodies until you configure a transformation.
/// Create it with [`Compression::client`] or [`Compression::server`].
#[derive(Debug, Clone)]
pub struct CompressionLayer<R> {
    config: Config,
    role: R,
}

impl Compression<()> {
    /// Creates a client compression layer.
    ///
    /// The layer uses the shared process-wide [`Resources`] by default.
    /// Use [`CompressionLayer::resources`] to select a different memory provider
    /// and engine pool.
    /// Streaming transformations keep the original body's policies.
    #[must_use]
    pub fn client() -> CompressionLayer<Client> {
        CompressionLayer {
            config: Config::new(),
            role: Client {
                decompress_responses: Formats::new(),
                advertise: true,
                accept_encoding: None,
            },
        }
    }

    /// Creates a server compression layer.
    ///
    /// The layer uses the shared process-wide [`Resources`] by default.
    /// Use [`CompressionLayer::resources`] to select a different memory provider
    /// and engine pool. Streaming transformations keep the original body's
    /// policies.
    #[must_use]
    pub fn server() -> CompressionLayer<Server> {
        CompressionLayer {
            config: Config::new(),
            role: Server {
                decompress_requests: Formats::new(),
                compress_responses: Formats::new(),
                level: Level::FAST,
                compressible: None,
            },
        }
    }
}

/// Content types that usually benefit from compression.
///
/// This list contains common text and text-based application types. It excludes
/// server-sent events and common types that are already compressed.
///
/// A base type covers its structured suffixes. For example,
/// `application/json` covers `application/ld+json`.
///
/// Pass to [`compressible_types`][CompressionLayer::compressible_types].
pub const DEFAULT_COMPRESSIBLE_TYPES: &[&str] = &[
    "text/plain",
    "text/html",
    "text/css",
    "text/javascript",
    "text/xml",
    "text/csv",
    "text/markdown",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/wasm",
    "image/svg+xml",
];

/// Configuration shared by client and server layers.
impl<R> CompressionLayer<R> {
    /// Bounds the cost of decompressing a single body.
    ///
    /// Each decompression stage limits output to 64 MiB and 1,024 compressed
    /// streams by default. An unset bound keeps the default. An explicit bound
    /// replaces it. Use
    /// [`unbounded_output_len`][DecompressorLimits::unbounded_output_len] or
    /// [`unbounded_streams`][DecompressorLimits::unbounded_streams] to remove a
    /// bound.
    #[must_use]
    pub const fn limits(mut self, limits: DecompressorLimits) -> Self {
        self.config.limits = limits.with_fallbacks(default_decompressor_limits());
        self
    }

    /// Uses `resources` instead of the shared process-wide resources.
    #[must_use]
    pub fn resources(mut self, resources: Resources) -> Self {
        self.config.resources = resources;
        self
    }

    /// Sets how a compression format that is not enabled is handled.
    ///
    /// Defaults to [`UnsupportedCompression::PassThrough`].
    #[must_use]
    pub const fn on_unsupported(mut self, policy: UnsupportedCompression) -> Self {
        self.config.on_unsupported = policy;
        self
    }
}

/// Client-only configuration.
impl CompressionLayer<Client> {
    /// Decompresses response bodies compressed with any of `formats`.
    ///
    /// Unless [`advertise`][Self::advertise] is turned off, outgoing requests
    /// also get an `Accept-Encoding` header. The header lists the preferred
    /// format first.
    ///
    /// A response can contain at most 16 enabled compression coding layers.
    /// This limit is fixed.
    #[must_use]
    pub fn decompress_responses(mut self, formats: &[Format]) -> Self {
        self.role.decompress_responses = http_formats(formats);
        self.role.accept_encoding = self.role.build_accept_encoding();
        self
    }

    /// Sets whether `Accept-Encoding` is added to outgoing requests.
    ///
    /// This setting has an effect only after
    /// [`decompress_responses`][Self::decompress_responses] has named
    /// a format. Disable it if an upstream proxy or caller-supplied header
    /// controls negotiation, but responses still need decompression.
    #[must_use]
    pub fn advertise(mut self, enabled: bool) -> Self {
        self.role.advertise = enabled;
        self.role.accept_encoding = self.role.build_accept_encoding();
        self
    }
}

/// Server-only configuration.
impl CompressionLayer<Server> {
    /// Decompresses request bodies compressed with any of `formats`.
    ///
    /// A request with a known body length of zero stays unchanged, including its
    /// `Content-Encoding`. A decoder cannot produce a compressed member from an
    /// empty body.
    ///
    /// Each decompression stage rejects output beyond 64 MiB or 1,024 compressed
    /// streams by default. Use [`limits`][Self::limits] to change these bounds.
    /// A request can contain at most 16 enabled compression coding layers. This
    /// limit is fixed.
    #[must_use]
    pub fn decompress_requests(mut self, formats: &[Format]) -> Self {
        self.role.decompress_requests = http_formats(formats);
        self
    }

    /// Compresses outgoing response bodies with the best of `formats` the caller accepts.
    ///
    /// The request's `Accept-Encoding` controls the choice. The selection uses
    /// quality values and `*`. `formats` lists server preference order and
    /// resolves equal quality values. Invalid members do not remove valid ones.
    ///
    /// For an eligible successful response with a complete, nonempty body, the
    /// layer returns an empty `406 Not Acceptable` response if no content coding
    /// is acceptable and the caller rejects `identity`. It does the same if an
    /// existing `Content-Encoding` is not acceptable.
    ///
    /// `HEAD`, successful `CONNECT`, statuses that cannot carry a body,
    /// known-empty bodies, partial responses, and responses with `Content-Range`
    /// bypass this negotiation.
    ///
    /// The method does not compress responses that already have a
    /// `Content-Encoding`. It also skips server-sent events, native gRPC, most
    /// images, audio, video, and common compressed archives. An allowlist cannot
    /// override these exclusions.
    ///
    /// Compression uses [`Level::FAST`] unless [`level`][Self::level] is set.
    /// If the body producer pauses, the compressor flushes buffered output. A
    /// consumer can then decompress each part before the response ends.
    #[must_use]
    pub fn compress_responses(mut self, formats: &[Format]) -> Self {
        self.role.compress_responses = http_formats(formats);
        self
    }

    /// Sets the compression effort for response bodies.
    ///
    /// The default is [`Level::FAST`]. Higher levels use more CPU to produce
    /// smaller bodies. Each format maps the portable level to its native range.
    /// The level does not affect decompression or format preference.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")] {
    /// use compressors::Level;
    /// use compressors::format::Format;
    /// use http_compression::Compression;
    /// let layer = Compression::server()
    ///     .compress_responses(&[Format::Gzip])
    ///     .level(Level::DEFAULT);
    /// # let _ = layer;
    /// # }
    /// ```
    #[must_use]
    pub const fn level(mut self, level: Level) -> Self {
        self.role.level = level;
        self
    }

    /// Compresses only bodies whose `Content-Type` matches one of `types`.
    ///
    /// `text/*` covers every text subtype. A base type covers its structured
    /// suffixes. For example, `application/json` covers
    /// `application/ld+json`.
    /// Parameters such as `; charset=utf-8` are ignored.
    ///
    /// An unreadable or invalid `Content-Type` prevents compression. A missing
    /// `Content-Type` prevents compression only when a list is set.
    ///
    /// Without a list, a missing `Content-Type` is permitted. The exclusions
    /// from [`compress_responses`][Self::compress_responses] still apply to a
    /// valid type. A list can add restrictions but cannot remove those
    /// exclusions. Start with
    /// [`DEFAULT_COMPRESSIBLE_TYPES`] for common compressible types.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")] {
    /// # use compressors::format::Format;
    /// # use http_compression::{Compression, DEFAULT_COMPRESSIBLE_TYPES};
    /// let layer = Compression::server()
    ///     .compress_responses(&[Format::Gzip])
    ///     .compressible_types(DEFAULT_COMPRESSIBLE_TYPES.iter().copied())
    ///     .unwrap();
    /// # let _ = layer;
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if any entry is not a valid media type.
    pub fn compressible_types<T: AsRef<str>>(
        mut self,
        types: impl IntoIterator<Item = T>,
    ) -> std::result::Result<Self, CompressibleTypeError> {
        self.role.compressible = Some(
            types
                .into_iter()
                .map(|pattern| {
                    let pattern = pattern.as_ref();
                    pattern
                        .parse::<Mime>()
                        .map_err(|source| CompressibleTypeError::new(pattern.to_owned(), source))
                })
                .collect::<std::result::Result<_, _>>()?,
        );

        Ok(self)
    }
}

impl<S> Layer<S> for CompressionLayer<Client> {
    type Service = Compression<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Compression {
            inner,
            config: self.config.clone(),
            role: Role::Client(self.role.clone()),
        }
    }
}

impl<S> Layer<S> for CompressionLayer<Server> {
    type Service = Compression<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Compression {
            inner,
            config: self.config.clone(),
            role: Role::Server(self.role.clone()),
        }
    }
}

impl<T: RequestHandler> Compression<T> {
    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let response = match &self.role {
            Role::Client(client) => {
                client.advertise(&mut input);
                let head = input.method() == Method::HEAD;
                let connect = input.method() == Method::CONNECT;

                Either::Left(
                    self.inner
                        .execute(input)
                        .map(move |response| client.decompress_response(&self.config, response?, head, connect)),
                )
            }
            Role::Server(server) => {
                let head = input.method() == Method::HEAD;
                let connect = input.method() == Method::CONNECT;
                let input = match server.decompress_request(&self.config, input) {
                    Ok(input) => input,
                    Err(error) => return Either::Left(ready(Err(error))),
                };

                // Choose before the inner handler runs to avoid cloning all headers.
                let chosen = server.choose_response_format(&input);

                Either::Right(
                    self.inner
                        .execute(input)
                        .map(move |response| server.compress_response(&self.config, response?, &chosen, head, connect)),
                )
            }
        };

        Either::Right(response)
    }
}

impl<T: RequestHandler> Service<HttpRequest> for Compression<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        Self::execute(self, input)
    }
}

/// Whether `actual` is covered by the `allowed` pattern.
fn matches_type(allowed: &Mime, actual: &Mime) -> bool {
    if allowed.type_() != mime::STAR && allowed.type_() != actual.type_() {
        return false;
    }

    if allowed.subtype() == mime::STAR {
        return true;
    }

    if let Some(allowed_suffix) = allowed.suffix() {
        return allowed.subtype() == actual.subtype() && Some(allowed_suffix) == actual.suffix();
    }

    match actual.suffix() {
        // A base type covers each matching structured suffix.
        Some(actual_suffix) => actual_suffix == allowed.subtype(),
        None => allowed.subtype() == actual.subtype(),
    }
}

/// Whether the parsed content type passes the built-in exclusions.
fn allows_type(actual: &Mime) -> bool {
    // `mime` accepts a trailing slash with no subtype.
    if actual.subtype().as_str().is_empty() {
        return false;
    }

    if actual.essence_str().eq_ignore_ascii_case("text/event-stream") {
        return false;
    }
    if actual.type_() == mime::IMAGE {
        return actual.essence_str().eq_ignore_ascii_case("image/svg+xml");
    }
    if actual.type_() == mime::AUDIO || actual.type_() == mime::VIDEO {
        return false;
    }

    if actual.type_() == mime::APPLICATION {
        if actual.subtype() == "grpc" {
            return false;
        }

        return ![
            "application/gzip",
            "application/x-gzip",
            "application/zip",
            "application/zstd",
            "application/x-bzip2",
            "application/x-xz",
            "application/x-7z-compressed",
            "application/vnd.rar",
            "application/x-rar-compressed",
        ]
        .iter()
        .any(|excluded| actual.essence_str().eq_ignore_ascii_case(excluded));
    }

    true
}

/// Whether there is a body here worth compressing.
///
/// No-content statuses cannot carry a body. Compression can create bytes that
/// the caller must not receive. Compression also skips an empty body because a
/// format header only makes it larger.
fn carries_a_body(response: &HttpResponse) -> bool {
    let status = response.status();
    if status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::RESET_CONTENT
        || status == StatusCode::NOT_MODIFIED
    {
        return false;
    }

    // Compression makes `Content-Range` offsets invalid.
    if status == StatusCode::PARTIAL_CONTENT || response.headers().contains_key(CONTENT_RANGE) {
        return false;
    }

    response.body().content_length() != Some(0)
}

/// Returns whether decompression can be necessary.
fn wants_decompression(enabled: &[Format], headers: &HeaderMap) -> bool {
    !enabled.is_empty() && headers.contains_key(CONTENT_ENCODING)
}

/// Weakens a strong entity tag after compression.
///
/// Compression creates different bytes for the same resource. A shared strong
/// tag lets a cache treat both representations as byte-identical. A weak
/// tag keeps equivalence without claiming identical bytes.
fn weaken_etag(headers: &mut HeaderMap) {
    let Some(etag) = headers.get(ETAG) else {
        return;
    };

    if etag.as_bytes().starts_with(b"W/") {
        return;
    }

    let sensitive = etag.is_sensitive();
    let mut weakened = weak_etag_value(etag);
    weakened.set_sensitive(sensitive);

    headers.insert(ETAG, weakened);
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn weak_etag_value(etag: &HeaderValue) -> HeaderValue {
    let mut bytes = Vec::with_capacity(etag.as_bytes().len() + 2);
    bytes.extend_from_slice(b"W/");
    bytes.extend_from_slice(etag.as_bytes());

    match HeaderValue::from_bytes(&bytes) {
        Ok(value) => value,
        Err(_) => unreachable!("prefixing a valid entity tag with 'W/' preserves header validity"),
    }
}

/// Adds `Accept-Encoding` to `Vary` without removing existing values.
fn mark_varies_on_accept_encoding(headers: &mut HeaderMap) {
    let already = headers
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| {
            let token = token.trim();
            token == "*" || token.eq_ignore_ascii_case("accept-encoding")
        });

    if !already {
        headers.append(VARY, HeaderValue::from_static("accept-encoding"));
    }
}

/// Keeps only compression formats supported by HTTP `Content-Encoding`.
///
/// Raw DEFLATE has no HTTP token, so this function removes it.
fn http_formats(formats: &[Format]) -> Formats {
    formats.iter().copied().filter(|f| f.content_encoding().is_some()).collect()
}

impl Client {
    /// Builds one `Accept-Encoding` value for reuse by each request.
    fn build_accept_encoding(&self) -> Option<HeaderValue> {
        if !self.advertise || self.decompress_responses.is_empty() {
            return None;
        }

        // Descending quality values state the preference. Token order alone does
        // not assign different quality values.
        let mut joined = String::with_capacity(self.decompress_responses.len() * ACCEPT_ENCODING_ENTRY_CAPACITY);

        for (index, format) in self.decompress_responses.iter().filter_map(|f| f.content_encoding()).enumerate() {
            if !joined.is_empty() {
                joined.push_str(", ");
            }

            joined.push_str(format);

            // The first value uses the implicit q=1. Later values decrease to a
            // floor of q=0.1 because q=0 rejects a format.
            if index > 0 {
                use std::fmt::Write as _;

                let quality = 10_usize.saturating_sub(index).max(1);
                let _ = write!(joined, ";q=0.{quality}");
            }
        }

        HeaderValue::from_str(&joined).ok()
    }

    /// Advertises supported formats unless the caller already set the header.
    ///
    /// The method preserves a caller-supplied `Accept-Encoding`. It does not add
    /// the header to a `Range` request because a partial compressed stream
    /// cannot be decompressed.
    fn advertise(&self, request: &mut HttpRequest) {
        let Some(value) = self.accept_encoding.as_ref() else {
            return;
        };

        let headers = request.headers();
        if headers.contains_key(ACCEPT_ENCODING) || headers.contains_key(RANGE) {
            return;
        }

        request.headers_mut().insert(ACCEPT_ENCODING, value.clone());
    }

    fn decompress_response(&self, config: &Config, response: HttpResponse, head: bool, connect: bool) -> Result<HttpResponse> {
        // These responses do not contain a complete body for decompression.
        // Preserve their metadata so the caller can interpret it.
        if head || (connect && response.status().is_success()) || !carries_a_body(&response) {
            return Ok(response);
        }

        if !wants_decompression(&self.decompress_responses, response.headers()) {
            return Ok(response);
        }

        let (mut parts, body) = response.into_parts();
        let body = config.decompress(&mut parts.headers, &mut parts.extensions, body, &self.decompress_responses)?;

        Ok(HttpResponse::from_parts(parts, body))
    }
}

#[derive(Debug)]
struct ResponseNegotiation {
    selection: negotiate::Selection,
    accept_encoding: HeaderMap,
}

impl Server {
    /// Selects the response format before the inner handler runs.
    ///
    /// A `HEAD` response has no body to compress, even if its headers describe
    /// one.
    fn choose_response_format(&self, request: &HttpRequest) -> ResponseNegotiation {
        let mut accept_encoding = HeaderMap::new();
        for value in request.headers().get_all(ACCEPT_ENCODING) {
            accept_encoding.append(ACCEPT_ENCODING, value.clone());
        }

        if self.compress_responses.is_empty() || request.method() == Method::HEAD {
            return ResponseNegotiation {
                selection: negotiate::Selection::Identity,
                accept_encoding,
            };
        }

        ResponseNegotiation {
            selection: negotiate::select(&accept_encoding, &self.compress_responses),
            accept_encoding,
        }
    }

    fn compress_response(
        &self,
        config: &Config,
        mut response: HttpResponse,
        negotiation: &ResponseNegotiation,
        head: bool,
        connect: bool,
    ) -> Result<HttpResponse> {
        if self.compress_responses.is_empty() || head || (connect && response.status().is_success()) {
            return Ok(response);
        }

        // Preserve an origin failure instead of replacing it with a negotiation response.
        if !response.status().is_success() {
            return Ok(response);
        }

        // Every negotiated response depends on `Accept-Encoding`, including an
        // uncompressed response. A cache must keep these variants separate.
        mark_varies_on_accept_encoding(response.headers_mut());

        if !carries_a_body(&response) {
            return Ok(response);
        }

        if response.headers().contains_key(CONTENT_ENCODING) {
            return if negotiate::content_encoding_acceptable(&negotiation.accept_encoding, response.headers()) {
                Ok(response)
            } else {
                not_acceptable(response)
            };
        }

        let format = match negotiation.selection {
            negotiate::Selection::Format(format) if self.is_compressible(response.headers()) => format,
            negotiate::Selection::Format(_) | negotiate::Selection::NotAcceptable => {
                if negotiate::identity_acceptable(&negotiation.accept_encoding) {
                    return Ok(response);
                }
                return not_acceptable(response);
            }
            negotiate::Selection::Identity => return Ok(response),
        };

        let (mut parts, body) = response.into_parts();
        let body = self.compress(config, &mut parts.headers, body, format)?;

        Ok(HttpResponse::from_parts(parts, body))
    }

    fn decompress_request(&self, config: &Config, request: HttpRequest) -> Result<HttpRequest> {
        if request.body().content_length() == Some(0) || !wants_decompression(&self.decompress_requests, request.headers()) {
            return Ok(request);
        }

        let (mut parts, body) = request.into_parts();
        let body = config.decompress(&mut parts.headers, &mut parts.extensions, body, &self.decompress_requests)?;

        Ok(HttpRequest::from_parts(parts, body))
    }

    /// Compresses small buffered data directly and streams larger data.
    fn compress(&self, config: &Config, headers: &mut HeaderMap, body: HttpBody, format: Format) -> Result<HttpBody> {
        let Some(token) = format.content_encoding() else {
            return Ok(body);
        };

        let compressor = CompressorBuilder::new()
            .level(self.level)
            .build_format(format, &config.resources)
            .map_err(invalid)?;

        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(token));
        headers.remove(CONTENT_LENGTH);
        headers.remove(CONTENT_DIGEST_HEADER);
        headers.remove(REPR_DIGEST_HEADER);
        weaken_etag(headers);

        body.map(|content| {
            Ok(match content {
                Content::Streaming(inner) => Content::Streaming(StreamingBody::new(body::CompressionBody::new(
                    body::CompressionChain::new(inner).compress(compressor),
                ))),
                Content::Bytes(bytes) if bytes.len() <= MAX_DIRECT_COMPRESSION_INPUT_LEN => {
                    let bytes = compressors::compress(bytes, compressor).map_err(body::to_http_error)?;
                    headers.remove(TRANSFER_ENCODING);
                    headers.insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
                    Content::Bytes(bytes)
                }
                Content::Bytes(bytes) => {
                    let inner = StreamingBody::new(http_body_util::Full::new(bytes));
                    Content::Streaming(StreamingBody::new(body::CompressionBody::new(
                        body::CompressionChain::new(inner).compress(compressor),
                    )))
                }
            })
        })
    }

    /// Applies built-in exclusions and the optional allowlist.
    fn is_compressible(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers.get(CONTENT_TYPE) else {
            return self.compressible.is_none();
        };

        let Some(actual) = value.to_str().ok().and_then(|value| value.parse::<Mime>().ok()) else {
            return false;
        };

        allows_type(&actual)
            && self
                .compressible
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|allowed| matches_type(allowed, &actual)))
    }
}

fn not_acceptable(response: HttpResponse) -> Result<HttpResponse> {
    let (mut parts, body) = response.into_parts();
    parts.status = StatusCode::NOT_ACCEPTABLE;
    parts.headers.remove(CONTENT_ENCODING);
    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.remove(CONTENT_RANGE);
    parts.headers.remove(CONTENT_TYPE);
    parts.headers.remove(ETAG);
    parts.headers.remove(CONTENT_DIGEST_HEADER);
    parts.headers.remove(REPR_DIGEST_HEADER);

    let body = body.map(|_| Ok(Content::Bytes(BytesView::default())))?;
    Ok(HttpResponse::from_parts(parts, body))
}

/// Configuration shared by both roles.
#[derive(Debug, Clone)]
struct Config {
    limits: DecompressorLimits,
    resources: Resources,
    on_unsupported: UnsupportedCompression,
}

impl Config {
    fn new() -> Self {
        Self {
            limits: default_decompressor_limits(),
            resources: Resources::default(),
            on_unsupported: UnsupportedCompression::default(),
        }
    }

    /// Decompresses both buffered and streaming input through a streaming body.
    fn decompress(
        &self,
        headers: &mut HeaderMap,
        extensions: &mut http::Extensions,
        body: HttpBody,
        enabled: &[Format],
    ) -> Result<HttpBody> {
        let Some(formats) = self.resolve(headers, enabled)? else {
            // Preserve `Content-Encoding` when decompression cannot proceed.
            return Ok(body);
        };

        if formats.is_empty() {
            // Only `identity`, which describes an uncompressed body.
            return Ok(body);
        }

        // Record the sender's compressed length before removing the header.
        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());

        headers.remove(CONTENT_ENCODING);
        headers.remove(CONTENT_LENGTH);
        headers.remove(CONTENT_DIGEST_HEADER);
        headers.remove(REPR_DIGEST_HEADER);

        // Build every engine before reading the body. Configuration failure then
        // fails the message before partial decompression.
        // `formats` is in header order, so decompression walks it backwards.
        let decompressors = formats
            .iter()
            .rev()
            .map(|format| {
                DecompressorBuilder::new()
                    .limits(self.limits)
                    .build_format(*format, &self.resources)
                    .map_err(invalid)
            })
            .collect::<Result<SmallVec<[_; 2]>>>()?;

        let decompressed = body.map(|content| {
            let inner = match content {
                Content::Streaming(inner) => inner,
                Content::Bytes(bytes) => StreamingBody::new(http_body_util::Full::new(bytes)),
            };
            let mut chain = body::CompressionChain::new(inner);
            for decompressor in decompressors {
                chain = chain.decompress(decompressor);
            }
            Ok(Content::Streaming(StreamingBody::new(body::CompressionBody::new(chain))))
        })?;

        extensions.insert(OriginalBody { formats, content_length });

        Ok(decompressed)
    }

    /// Resolves `Content-Encoding` to the formats to strip, in header order.
    ///
    /// Returns `None` if the header is unreadable, invalid, or names a disabled
    /// format. The caller then leaves the body and its headers unchanged.
    ///
    /// Decompression applies every coding or none. Partial decompression can
    /// require a rewritten `Content-Encoding`.
    fn resolve(&self, headers: &HeaderMap, enabled: &[Format]) -> Result<Option<Formats>> {
        let mut formats = Formats::new();

        for value in headers.get_all(CONTENT_ENCODING) {
            let Ok(value) = value.to_str() else {
                return Ok(None);
            };

            for token in value.split(',').map(str::trim) {
                // RFC 9110 section 5.6.1.2 requires recipients to ignore empty
                // members that result from combined headers.
                if token.is_empty() {
                    continue;
                }

                if token.eq_ignore_ascii_case("identity") {
                    continue;
                }

                let Some(format) = Format::from_content_encoding(token) else {
                    return Ok(None);
                };

                if !enabled.contains(&format) {
                    // The configured policy controls a known but disabled format.
                    return if self.on_unsupported == UnsupportedCompression::Fail {
                        Err(unsupported(token))
                    } else {
                        Ok(None)
                    };
                }

                if formats.len() == MAX_CONTENT_ENCODING_LAYERS {
                    return Err(too_many_content_codings(MAX_CONTENT_ENCODING_LAYERS));
                }
                formats.push(format);
            }
        }

        Ok(Some(formats))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn raw_deflate_has_no_http_content_encoding() {
        let layer = Compression::server();
        let mut headers = HeaderMap::new();
        let body = http_extensions::HttpResponseBuilder::new_fake()
            .text("unchanged")
            .build()
            .unwrap()
            .into_body();

        let body = layer.role.compress(&layer.config, &mut headers, body, Format::Deflate).unwrap();

        assert!(headers.get(CONTENT_ENCODING).is_none());
        assert_eq!(futures::executor::block_on(body.into_text()).unwrap(), "unchanged");
    }
}
