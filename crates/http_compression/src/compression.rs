// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The compression handler and the layer that configures it.

use bytesbuf::mem::HasMemory as _;
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, DecompressorLimits, Level, Resources};
use futures::FutureExt as _;
use futures::future::{Either, ready};
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE, VARY};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{Layer, Service};
use mime::Mime;
use smallvec::SmallVec;

use crate::error::{invalid, unsupported};
use crate::{body, negotiate};

/// A short list of formats: a stacked `Content-Encoding` is rare.
type Formats = SmallVec<[Format; 2]>;

/// How to handle a compression format that is not enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UnsupportedCompression {
    /// Hand back the still-compressed body with its `Content-Encoding` intact.
    ///
    /// The caller can inspect `Content-Encoding` to identify the compression
    /// format that was left in place.
    #[default]
    PassThrough,

    /// Fail with the `compression_unsupported` error label.
    ///
    /// Use this when the body is always treated as decompressed, and compressed
    /// bytes would otherwise be parsed as though they were payload.
    Fail,
}

/// Metadata describing a body as it traveled on the wire, before decompression.
///
/// Decompression removes `Content-Encoding` and `Content-Length`, because neither
/// describes the body that is now held. This keeps what they said, and is
/// attached to the message only when its body was actually decompressed.
///
/// Servers attach it to the decompressed request's extensions; clients attach it to
/// the decompressed response's extensions. It does not retain the compressed body bytes.
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

    /// What `Content-Length` said, when the message carried one.
    ///
    /// This measures the compressed body, not its decompressed size.
    /// It is `None` for a message that arrived chunked. The decompressed length is
    /// not knowable until the body has been read to the end.
    #[must_use]
    pub const fn content_length(&self) -> Option<u64> {
        self.content_length
    }
}

/// Client-side configuration held by a [`CompressionLayer`].
///
/// A client sends requests and receives responses, so it decompresses what comes
/// back. Create its layer with [`Compression::client`].
///
/// Server-side response compression is not available on a client layer:
///
/// ```compile_fail,E0599
/// # use http_compression::Compression;
/// # use http_extensions::HttpBodyBuilder;
/// Compression::client(HttpBodyBuilder::new_fake()).compress_responses(&[]);
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
/// A server receives requests and sends responses, so it decompresses what arrives
/// and compresses what it returns, choosing a compression format the caller accepts.
/// Create its layer with [`Compression::server`].
///
/// Client-side response decompression is not available on a server layer:
///
/// ```compile_fail,E0599
/// # use http_compression::Compression;
/// # use http_extensions::HttpBodyBuilder;
/// Compression::server(HttpBodyBuilder::new_fake()).decompress_responses(&[]);
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
/// One handler covers both roles, because a client and a server want different
/// parts of the same job. Each part is off until it is configured, and which
/// parts exist at all is decided by the role:
///
/// | Role | Configured with | Transforms |
/// |---|---|---|
/// | [`Client`] | [`decompress_responses`][CompressionLayer::decompress_responses] | the response coming back |
/// | [`Server`] | [`decompress_requests`][CompressionLayer::decompress_requests] | the request coming in |
/// | [`Server`] | [`compress_responses`][CompressionLayer::compress_responses] | the response going out |
///
/// The role is chosen up front, with [`Compression::client`] or
/// [`Compression::server`], and only that role's toggles exist on the layer it
/// returns. A client therefore cannot ask to compress a response it has just
/// received, and a server cannot advertise `Accept-Encoding` on a request that
/// is arriving rather than leaving.
///
/// All three are lazy. Bodies are wrapped as the message passes, and the work
/// happens as the body is polled, so a malformed or oversized body fails when
/// it is read rather than when its headers arrive.
///
/// Apply its layer with [`Layer::layer`].
#[derive(Debug)]
pub struct Compression<T> {
    inner: T,
    config: Config,
    role: Role,
}

/// Layer for creating [`Compression`] instances, in the role `R`.
///
/// Does nothing until one of the role's transformations is configured. Build
/// one with [`Compression::client`] or [`Compression::server`].
#[derive(Debug, Clone)]
pub struct CompressionLayer<R> {
    config: Config,
    role: R,
}

impl Compression<()> {
    /// Creates a client-side layer drawing output from `body_builder`.
    ///
    /// Transformed bytes are allocated from the same memory the builder uses
    /// for every other body, and a wrapped body keeps the policies the original
    /// carried.
    #[must_use]
    pub fn client(body_builder: HttpBodyBuilder) -> CompressionLayer<Client> {
        CompressionLayer {
            config: Config::new(body_builder),
            role: Client {
                decompress_responses: Formats::new(),
                advertise: true,
                accept_encoding: None,
            },
        }
    }

    /// Creates a server-side layer drawing output from `body_builder`.
    ///
    /// See [`client`][Self::client] for where the bytes come from.
    #[must_use]
    pub fn server(body_builder: HttpBodyBuilder) -> CompressionLayer<Server> {
        CompressionLayer {
            config: Config::new(body_builder),
            role: Server {
                decompress_requests: Formats::new(),
                compress_responses: Formats::new(),
                level: Level::FAST,
                compressible: None,
            },
        }
    }
}

/// The content types usually worth compressing.
///
/// Common text types, plus application types that are textual underneath.
/// What is left out is the point: images, video and archives are already
/// compressed, so compressing them again spends CPU to make the body slightly
/// larger. Server-sent events are not included.
///
/// A type with a structured suffix is covered by the type it is built on, so
/// `application/ld+json` matches `application/json` without being listed.
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

/// Toggles that mean the same thing in either role.
impl<R> CompressionLayer<R> {
    /// Bounds the cost of decompressing a single body.
    ///
    /// The streaming decompressor applies no bounds of its own, so whatever is set
    /// here is the only thing standing between the reader and a decompression
    /// bomb.
    #[must_use]
    pub const fn limits(mut self, limits: DecompressorLimits) -> Self {
        self.config.limits = limits;
        self
    }

    /// Draws transformed output from `resources` instead of the body builder's memory.
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

/// What only a client can ask for: it sends requests and reads responses.
impl CompressionLayer<Client> {
    /// Decompresses response bodies compressed with any of `formats`.
    ///
    /// Unless [`advertise`][Self::advertise] is turned off, outgoing requests
    /// also gain an `Accept-Encoding` naming these formats, most preferred
    /// first.
    #[must_use]
    pub fn decompress_responses(mut self, formats: &[Format]) -> Self {
        self.role.decompress_responses = http_formats(formats);
        self.role.accept_encoding = self.role.build_accept_encoding();
        self
    }

    /// Sets whether `Accept-Encoding` is added to outgoing requests.
    ///
    /// Only has an effect once
    /// [`decompress_responses`][Self::decompress_responses] has named
    /// something. Turn it off when an upstream proxy or a caller-supplied
    /// header owns content negotiation but responses should still be decompressed.
    #[must_use]
    pub fn advertise(mut self, enabled: bool) -> Self {
        self.role.advertise = enabled;
        self.role.accept_encoding = self.role.build_accept_encoding();
        self
    }
}

/// What only a server can ask for: it reads requests and sends responses.
impl CompressionLayer<Server> {
    /// Decompresses request bodies compressed with any of `formats`.
    #[must_use]
    pub fn decompress_requests(mut self, formats: &[Format]) -> Self {
        self.role.decompress_requests = http_formats(formats);
        self
    }

    /// Compresses outgoing response bodies with the best of `formats` the caller accepts.
    ///
    /// The choice is made from the request's `Accept-Encoding`, respecting
    /// quality values and `*`. `formats` is in preference order, which decides
    /// between compression formats the caller rates equally. Malformed members are
    /// skipped without discarding valid alternatives.
    ///
    /// Responses that already have a `Content-Encoding` are left alone, as are
    /// server-sent events, native gRPC, images other than `image/svg+xml`, audio, video and
    /// common compressed archive types. A content-type allowlist cannot override
    /// these exclusions.
    ///
    /// Compression uses [`Level::FAST`] unless [`level`][Self::level] is set.
    /// Buffered output is flushed when the body producer pauses, so consumers can
    /// decompress each burst without waiting for the response to end.
    #[must_use]
    pub fn compress_responses(mut self, formats: &[Format]) -> Self {
        self.role.compress_responses = http_formats(formats);
        self
    }

    /// Sets the compression effort for response bodies.
    ///
    /// Defaults to [`Level::FAST`]. Higher levels trade more CPU work for smaller
    /// bodies. The portable level is mapped onto each negotiated format's native
    /// range; it does not affect decompression or format preference.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")] {
    /// use compressors::Level;
    /// use compressors::format::Format;
    /// use http_compression::Compression;
    /// # use http_extensions::HttpBodyBuilder;
    /// # let body_builder = HttpBodyBuilder::new_fake();
    ///
    /// let layer = Compression::server(body_builder)
    ///     .compress_responses(&[Format::Gzip])
    ///     .level(Level::DEFAULT);
    /// # }
    /// ```
    #[must_use]
    pub const fn level(mut self, level: Level) -> Self {
        self.role.level = level;
        self
    }

    /// Compresses only bodies whose `Content-Type` matches one of `types`.
    ///
    /// An entry such as `text/*` covers every kind of text, and an entry whose
    /// second half carries a structured suffix covers the type it is built on, so `application/ld+json` is covered by `application/json`.
    /// Parameters such as `; charset=utf-8` are ignored, and an entry that is
    /// not a media type at all is dropped.
    ///
    /// Once a list is set, a body with a missing or unreadable `Content-Type` is
    /// left alone, since there is nothing to check it against.
    ///
    /// With no list set, only the built-in exclusions described by
    /// [`compress_responses`][Self::compress_responses] apply. A list can restrict
    /// compression further, but cannot override those exclusions.
    /// [`DEFAULT_COMPRESSIBLE_TYPES`] is a reasonable starting point.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")] {
    /// # use compressors::format::Format;
    /// # use http_compression::{Compression, DEFAULT_COMPRESSIBLE_TYPES};
    /// # use http_extensions::HttpBodyBuilder;
    /// # let body_builder = HttpBodyBuilder::new_fake();
    /// let layer = Compression::server(body_builder)
    ///     .compress_responses(&[Format::Gzip])
    ///     .compressible_types(DEFAULT_COMPRESSIBLE_TYPES.iter().copied());
    /// # }
    /// ```
    #[must_use]
    pub fn compressible_types<T: AsRef<str>>(mut self, types: impl IntoIterator<Item = T>) -> Self {
        self.role.compressible = Some(
            types
                .into_iter()
                .filter_map(|pattern| pattern.as_ref().parse::<Mime>().ok())
                .collect(),
        );

        self
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

impl<T: RequestHandler> Service<HttpRequest> for Compression<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let response = match &self.role {
            Role::Client(client) => {
                client.advertise(&mut input);
                let head = input.method() == Method::HEAD;

                Either::Left(
                    self.inner
                        .execute(input)
                        .map(move |response| client.decompress_response(&self.config, response?, head)),
                )
            }
            Role::Server(server) => {
                let input = match server.decompress_request(&self.config, input) {
                    Ok(input) => input,
                    Err(error) => return Either::Left(ready(Err(error))),
                };

                // Choose before handing off the request so its headers need not be cloned.
                let chosen = server.choose_response_format(&input);

                Either::Right(
                    self.inner
                        .execute(input)
                        .map(move |response| server.compress_response(&self.config, response?, chosen)),
                )
            }
        };

        Either::Right(response)
    }
}

/// Whether `actual` is covered by the `allowed` pattern.
fn matches_type(allowed: &Mime, actual: &Mime) -> bool {
    if allowed.type_() != mime::STAR && allowed.type_() != actual.type_() {
        return false;
    }

    if allowed.subtype() == mime::STAR || allowed.subtype() == actual.subtype() {
        return true;
    }

    // `application/ld+json` is JSON underneath, so the type it is built on
    // covers it without every variant having to be listed.
    actual.suffix().is_some_and(|suffix| suffix == allowed.subtype())
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
/// The no-content statuses are defined to carry no body at all, so compressing one
/// would invent bytes the caller must not receive. An already-empty body is
/// skipped too: every format has a header, so compressing nothing only makes it
/// bigger.
fn carries_a_body(response: &HttpResponse) -> bool {
    let status = response.status();
    if status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::RESET_CONTENT
        || status == StatusCode::NOT_MODIFIED
    {
        return false;
    }

    // A partial response is a byte range of the representation. Compressing it
    // would leave `Content-Range` describing offsets into something that no
    // longer exists.
    if status == StatusCode::PARTIAL_CONTENT || response.headers().contains_key(CONTENT_RANGE) {
        return false;
    }

    response.body().content_length() != Some(0)
}

/// Weakens a strong entity tag, which a compressed body may no longer claim.
///
/// Compression produces a different representation of the same resource, so the
/// two must not share a strong entity tag: a cache holding both would otherwise
/// treat them as byte-identical and could recombine ranges across them. Marking
/// the tag weak keeps it usable for equivalence without claiming that.
fn weaken_etag(headers: &mut HeaderMap) {
    let Some(etag) = headers.get(ETAG).and_then(|value| value.to_str().ok()) else {
        return;
    };

    if etag.starts_with("W/") {
        return;
    }

    let Ok(weakened) = HeaderValue::from_str(&format!("W/{etag}")) else {
        return;
    };

    headers.insert(ETAG, weakened);
}

/// Records that the body depends on `Accept-Encoding`, without losing what else it varies by.
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
/// Raw DEFLATE has none, so it can never appear in a header and is dropped
/// rather than silently never matching.
fn http_formats(formats: &[Format]) -> Formats {
    formats.iter().copied().filter(|f| f.content_encoding().is_some()).collect()
}

impl Client {
    /// Builds the `Accept-Encoding` value, so every request just clones it.
    fn build_accept_encoding(&self) -> Option<HeaderValue> {
        if !self.advertise || self.decompress_responses.is_empty() {
            return None;
        }

        // Bare tokens all rate the same, so the order would be a hint at best.
        // Descending quality values say the preference outright, which is what
        // a server needs to honour it.
        let mut joined = String::new();

        for (index, format) in self.decompress_responses.iter().filter_map(|f| f.content_encoding()).enumerate() {
            if !joined.is_empty() {
                joined.push_str(", ");
            }

            joined.push_str(format);

            // The first is left implicit at q=1, and the rest step down.
            // Anything past the tenth shares the floor rather than hitting zero,
            // which would read as a refusal.
            if index > 0 {
                use std::fmt::Write as _;

                let quality = 10_usize.saturating_sub(index).max(1);
                let _ = write!(joined, ";q=0.{quality}");
            }
        }

        HeaderValue::from_str(&joined).ok()
    }

    /// Advertises what can be decompressed, unless the caller already has.
    ///
    /// A caller-supplied `Accept-Encoding` is left alone. A `Range` request is
    /// never given one: a partial compressed stream cannot be decompressed, so
    /// asking for one only invites a response that must be handed back still compressed.
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

    fn decompress_response(&self, config: &Config, response: HttpResponse, head: bool) -> Result<HttpResponse> {
        // A `HEAD` response describes a body it does not carry, and a partial
        // one holds a byte range of the compressed representation rather than the
        // whole of it. Neither can be decompressed, and stripping the metadata would
        // leave the caller unable to decompress it either.
        if head || !carries_a_body(&response) {
            return Ok(response);
        }

        if !Config::wants_decompression(&self.decompress_responses, response.headers()) {
            return Ok(response);
        }

        let (mut parts, body) = response.into_parts();
        let body = config.decompress(&mut parts.headers, &mut parts.extensions, body, &self.decompress_responses)?;

        Ok(HttpResponse::from_parts(parts, body))
    }
}

impl Server {
    /// Picks the response's compression format before the inner handler runs.
    ///
    /// A `HEAD` response carries no body to compress however its headers describe
    /// one, so it is ruled out here rather than after the fact.
    fn choose_response_format(&self, request: &HttpRequest) -> Option<Format> {
        if self.compress_responses.is_empty() || request.method() == Method::HEAD {
            return None;
        }

        negotiate::select(request.headers(), &self.compress_responses)
    }

    fn compress_response(&self, config: &Config, mut response: HttpResponse, chosen: Option<Format>) -> Result<HttpResponse> {
        if self.compress_responses.is_empty() {
            return Ok(response);
        }

        // The body now depends on what the caller said it accepts, so any cache
        // between here and them has to key on that. This holds even when
        // nothing was compressed: the same request can still be answered
        // differently, and a cache that missed it would serve a compressed body
        // to a caller that never asked for one.
        mark_varies_on_accept_encoding(response.headers_mut());

        let Some(format) = chosen else {
            return Ok(response);
        };

        if !carries_a_body(&response) {
            return Ok(response);
        }

        let (mut parts, body) = response.into_parts();
        let body = self.compress(config, &mut parts.headers, body, format)?;

        Ok(HttpResponse::from_parts(parts, body))
    }

    fn decompress_request(&self, config: &Config, request: HttpRequest) -> Result<HttpRequest> {
        if !Config::wants_decompression(&self.decompress_requests, request.headers()) {
            return Ok(request);
        }

        let (mut parts, body) = request.into_parts();
        let body = config.decompress(&mut parts.headers, &mut parts.extensions, body, &self.decompress_requests)?;

        Ok(HttpRequest::from_parts(parts, body))
    }

    /// Replaces `body` with one that compresses as it is read.
    ///
    /// A message that already declares a `Content-Encoding` is left alone: it is
    /// already compressed, and stacking another format on top would be a surprise.
    fn compress(&self, config: &Config, headers: &mut HeaderMap, body: HttpBody, format: Format) -> Result<HttpBody> {
        let Some(token) = format.content_encoding() else {
            return Ok(body);
        };

        if headers.contains_key(CONTENT_ENCODING) || !self.is_compressible(headers) {
            return Ok(body);
        }

        let compressor = CompressorBuilder::new()
            .level(self.level)
            .build_format(format, &config.resources)
            .map_err(invalid)?;

        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(token));
        // The compressed length is not known until the body has been read.
        headers.remove(CONTENT_LENGTH);
        weaken_etag(headers);

        Ok(config.body_builder.rewrap(body, move |body| {
            body::CompressionBody::new(body::CompressionChain::new(body).compress(compressor))
        }))
    }

    /// Applies the built-in exclusions and optional allowlist to one parsed content type.
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

/// Settings shared by both roles.
#[derive(Debug, Clone)]
struct Config {
    limits: DecompressorLimits,
    resources: Resources,
    body_builder: HttpBodyBuilder,
    on_unsupported: UnsupportedCompression,
}

impl Config {
    fn new(body_builder: HttpBodyBuilder) -> Self {
        Self {
            limits: DecompressorLimits::new(),
            resources: Resources::new(body_builder.memory()),
            body_builder,
            on_unsupported: UnsupportedCompression::default(),
        }
    }

    /// The cheap test that keeps every uncompressed message off the slow path.
    fn wants_decompression(enabled: &[Format], headers: &HeaderMap) -> bool {
        !enabled.is_empty() && headers.contains_key(CONTENT_ENCODING)
    }

    /// Replaces `body` with one that decompresses as it is read.
    fn decompress(
        &self,
        headers: &mut HeaderMap,
        extensions: &mut http::Extensions,
        body: HttpBody,
        enabled: &[Format],
    ) -> Result<HttpBody> {
        let Some(formats) = self.resolve(headers, enabled)? else {
            // Something here cannot be decompressed. `Content-Encoding` stays intact
            // so the reader can still tell the body is compressed.
            return Ok(body);
        };

        if formats.is_empty() {
            // Only `identity`, which describes an uncompressed body.
            return Ok(body);
        }

        // Read before removing: this is the value the extension reports, and it
        // is the sender's claim about the compressed body rather than anything the
        // body itself knows.
        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());

        headers.remove(CONTENT_ENCODING);
        headers.remove(CONTENT_LENGTH);

        // Built before the body is touched, so an engine that cannot be
        // configured fails the message rather than the body halfway through it.
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

        // `rewrap` rather than `body`: the body already applies its own idle
        // timeout to the frames coming off the transport, and timing the decompressed
        // frames as well would trip on a body that decompresses many input frames
        // into few output ones.
        let decompressed = self.body_builder.rewrap(body, move |body| {
            let mut chain = body::CompressionChain::new(body);

            for decompressor in decompressors {
                chain = chain.decompress(decompressor);
            }

            body::CompressionBody::new(chain)
        });

        extensions.insert(OriginalBody { formats, content_length });

        Ok(decompressed)
    }

    /// Resolves `Content-Encoding` to the formats to strip, in header order.
    ///
    /// Returns `None` when the header cannot be read, is malformed, or names a
    /// compression format that is not enabled. In each case the body is left
    /// exactly as it arrived, with its headers intact: a `Content-Encoding` that could not be
    /// made sense of says nothing to act on, and is no reason to fail a message
    /// that is otherwise fine.
    ///
    /// Decompression is all-or-nothing. A partly decompressed body would need a rewritten
    /// header to stay honest, and a stacked `Content-Encoding` is rare enough
    /// not to be worth that.
    fn resolve(&self, headers: &HeaderMap, enabled: &[Format]) -> Result<Option<Formats>> {
        let mut formats = Formats::new();

        for value in headers.get_all(CONTENT_ENCODING) {
            let Ok(value) = value.to_str() else {
                return Ok(None);
            };

            for token in value.split(',').map(str::trim) {
                // RFC 9110 5.6.1.2 asks recipients to ignore empty members
                // rather than reject the field: they come from headers being
                // combined, not from the sender meaning anything by them.
                if token.is_empty() {
                    continue;
                }

                if token.eq_ignore_ascii_case("identity") {
                    continue;
                }

                match Format::from_content_encoding(token).filter(|format| enabled.contains(format)) {
                    Some(format) => formats.push(format),
                    // A recognisable compression format that is not enabled is a
                    // different matter: the caller may have asked to hear about it.
                    None if self.on_unsupported == UnsupportedCompression::Fail => return Err(unsupported(token)),
                    None => return Ok(None),
                }
            }
        }

        Ok(Some(formats))
    }
}
