// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The name a field is stored and looked up under.

use std::error::Error;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::{fmt, mem, str};

const STACK_NORMALIZATION_CAPACITY: usize = 64;

/// An error produced when bytes cannot form an HTTP field name.
///
/// # Examples
///
/// ```rust
/// use http_headers::{FieldName, InvalidFieldName};
///
/// let error: InvalidFieldName = FieldName::try_from_bytes(b"bad header").unwrap_err();
/// assert_eq!(error.to_string(), "invalid HTTP field name");
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct InvalidFieldName;

impl fmt::Display for InvalidFieldName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid HTTP field name")
    }
}

impl Error for InvalidFieldName {}

macro_rules! known_headers {
    ($(($variant:ident, $konst:ident, $text:literal),)+) => {
        /// An owned HTTP field name.
        ///
        /// Every name in [`FieldName::ALL_KNOWN`] has its own variant. Other
        /// valid names use [`FieldName::Custom`], allowing this type to
        /// represent extension and application-defined headers as well.
        ///
        /// Names compare and hash by their lowercase bytes. Recognition is
        /// ASCII case-insensitive, so a parsed `Accept` and the constant
        /// [`FieldName::Accept`] are the same value.
        ///
        /// # Examples
        ///
        /// ```rust
        /// use std::sync::LazyLock;
        ///
        /// use http_headers::FieldName;
        ///
        /// static CUSTOM: LazyLock<FieldName> =
        ///     LazyLock::new(|| FieldName::from_static("x-trace-id"));
        ///
        /// assert_eq!(FieldName::UserAgent.as_str(), "user-agent");
        /// assert_eq!(FieldName::try_from_bytes(b"User-Agent")?, FieldName::UserAgent);
        /// assert!(FieldName::UserAgent.index().is_some_and(|index| index < FieldName::COUNT));
        /// assert_eq!(CUSTOM.index(), None);
        /// # Ok::<(), http_headers::InvalidFieldName>(())
        /// ```
        #[derive(Clone)]
        #[non_exhaustive]
        pub enum FieldName {
            $(
                #[doc = concat!("The `", $text, "` header.")]
                $variant,
            )+
            /// A field name this crate does not recognize.
            ///
            /// Use [`FieldName::from_static`] for a static lowercase name or
            /// [`FieldName::try_from_bytes`] for runtime input.
            ///
            /// The shared representation makes cloning a custom name cheap.
            #[non_exhaustive]
            Custom(Arc<str>),
        }

        impl FieldName {
            /// Every well-known name, ordered by index.
            ///
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::FieldName;
            ///
            /// assert_eq!(FieldName::ALL_KNOWN.len(), FieldName::COUNT);
            /// ```
            pub const ALL_KNOWN: &'static [Self] = &[$(Self::$variant,)+];

            /// The number of well-known names.
            ///
            /// # Examples
            ///
            /// ```rust
            /// assert!(http_headers::FieldName::COUNT > 0);
            /// ```
            pub const COUNT: usize = Self::ALL_KNOWN.len();

            /// Returns the name's position in [`FieldName::ALL_KNOWN`].
            ///
            /// A custom name has no index. The returned value is always
            /// smaller than [`FieldName::COUNT`] and is suitable for indexing
            /// a table with one entry per well-known name.
            ///
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::FieldName;
            ///
            /// assert!(FieldName::Accept.index().is_some_and(|index| index < FieldName::COUNT));
            /// assert_eq!(FieldName::from_static("x-trace-id").index(), None);
            /// ```
            #[must_use]
            #[inline]
            pub fn index(&self) -> Option<usize> {
                #[repr(usize)]
                enum Index {
                    $($variant,)+
                }

                match self {
                    $(Self::$variant => Some(Index::$variant as usize),)+
                    Self::Custom(_) => None,
                }
            }

            /// Returns the lowercase field name.
            ///
            /// # Examples
            ///
            /// ```rust
            /// assert_eq!(http_headers::FieldName::Accept.as_str(), "accept");
            /// ```
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Custom(name) => name,
                }
            }

            /// Creates a name from a static lowercase field name.
            ///
            /// A recognized name becomes its corresponding well-known variant.
            ///
            /// # Panics
            ///
            /// Panics when `name` is not a lowercase HTTP token or is longer
            /// than 65,535 bytes.
            ///
            /// # Examples
            ///
            /// ```rust
            /// use std::sync::LazyLock;
            ///
            /// use http_headers::FieldName;
            ///
            /// static TRACE_ID: LazyLock<FieldName> =
            ///     LazyLock::new(|| FieldName::from_static("x-trace-id"));
            ///
            /// assert_eq!(TRACE_ID.as_str(), "x-trace-id");
            /// assert_eq!(FieldName::from_static("accept"), FieldName::Accept);
            /// ```
            #[must_use]
            pub fn from_static(name: &'static str) -> Self {
                assert!(
                    is_lowercase_name(name.as_bytes()) && name.len() <= MAX_NAME_LEN,
                    "invalid static HTTP field name: {name:?}"
                );
                Self::known_from_bytes(name.as_bytes())
                    .unwrap_or_else(|| Self::Custom(Arc::from(name)))
            }

            /// Returns the static `http` name of a well-known header.
            ///
            /// A custom name has no `http` constant. Converting the
            /// [`FieldName`] itself works for both well-known and custom
            /// names.
            ///
            /// # Examples
            ///
            /// ```rust
            /// # #[cfg(feature = "http")]
            /// # fn main() {
            /// use http_headers::FieldName;
            ///
            /// assert_eq!(FieldName::Accept.http_name(), Some(&http::header::ACCEPT));
            /// assert_eq!(FieldName::from_static("x-trace-id").http_name(), None);
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            #[cfg(feature = "http")]
            #[must_use]
            #[inline(always)]
            pub fn http_name(&self) -> Option<&'static http::HeaderName> {
                match self {
                    $(Self::$variant => Some(&http::header::$konst),)+
                    Self::Custom(_) => None,
                }
            }
        }

        /// The lowercase text of every well-known name, ordered by index.
        ///
        /// This is the key table the length-bucketed recognition dispatch
        /// below is derived from at compile time; it is generated from the
        /// same list as the variants, so the two can never drift.
        const KNOWN_TEXTS: &[&str] = &[$($text,)+];
    };
}

known_headers! {
    (Accept, ACCEPT, "accept"),
    (AcceptCharset, ACCEPT_CHARSET, "accept-charset"),
    (AcceptEncoding, ACCEPT_ENCODING, "accept-encoding"),
    (AcceptLanguage, ACCEPT_LANGUAGE, "accept-language"),
    (AcceptRanges, ACCEPT_RANGES, "accept-ranges"),
    (AccessControlAllowCredentials, ACCESS_CONTROL_ALLOW_CREDENTIALS, "access-control-allow-credentials"),
    (AccessControlAllowHeaders, ACCESS_CONTROL_ALLOW_HEADERS, "access-control-allow-headers"),
    (AccessControlAllowMethods, ACCESS_CONTROL_ALLOW_METHODS, "access-control-allow-methods"),
    (AccessControlAllowOrigin, ACCESS_CONTROL_ALLOW_ORIGIN, "access-control-allow-origin"),
    (AccessControlExposeHeaders, ACCESS_CONTROL_EXPOSE_HEADERS, "access-control-expose-headers"),
    (AccessControlMaxAge, ACCESS_CONTROL_MAX_AGE, "access-control-max-age"),
    (AccessControlRequestHeaders, ACCESS_CONTROL_REQUEST_HEADERS, "access-control-request-headers"),
    (AccessControlRequestMethod, ACCESS_CONTROL_REQUEST_METHOD, "access-control-request-method"),
    (Age, AGE, "age"),
    (Allow, ALLOW, "allow"),
    (AltSvc, ALT_SVC, "alt-svc"),
    (Authorization, AUTHORIZATION, "authorization"),
    (CacheControl, CACHE_CONTROL, "cache-control"),
    (CacheStatus, CACHE_STATUS, "cache-status"),
    (CdnCacheControl, CDN_CACHE_CONTROL, "cdn-cache-control"),
    (Connection, CONNECTION, "connection"),
    (ContentDisposition, CONTENT_DISPOSITION, "content-disposition"),
    (ContentEncoding, CONTENT_ENCODING, "content-encoding"),
    (ContentLanguage, CONTENT_LANGUAGE, "content-language"),
    (ContentLength, CONTENT_LENGTH, "content-length"),
    (ContentLocation, CONTENT_LOCATION, "content-location"),
    (ContentRange, CONTENT_RANGE, "content-range"),
    (ContentSecurityPolicy, CONTENT_SECURITY_POLICY, "content-security-policy"),
    (ContentSecurityPolicyReportOnly, CONTENT_SECURITY_POLICY_REPORT_ONLY, "content-security-policy-report-only"),
    (ContentType, CONTENT_TYPE, "content-type"),
    (Cookie, COOKIE, "cookie"),
    (Dnt, DNT, "dnt"),
    (Date, DATE, "date"),
    (Etag, ETAG, "etag"),
    (Expect, EXPECT, "expect"),
    (Expires, EXPIRES, "expires"),
    (Forwarded, FORWARDED, "forwarded"),
    (From, FROM, "from"),
    (Host, HOST, "host"),
    (IfMatch, IF_MATCH, "if-match"),
    (IfModifiedSince, IF_MODIFIED_SINCE, "if-modified-since"),
    (IfNoneMatch, IF_NONE_MATCH, "if-none-match"),
    (IfRange, IF_RANGE, "if-range"),
    (IfUnmodifiedSince, IF_UNMODIFIED_SINCE, "if-unmodified-since"),
    (LastModified, LAST_MODIFIED, "last-modified"),
    (Link, LINK, "link"),
    (Location, LOCATION, "location"),
    (MaxForwards, MAX_FORWARDS, "max-forwards"),
    (Origin, ORIGIN, "origin"),
    (Pragma, PRAGMA, "pragma"),
    (ProxyAuthenticate, PROXY_AUTHENTICATE, "proxy-authenticate"),
    (ProxyAuthorization, PROXY_AUTHORIZATION, "proxy-authorization"),
    (PublicKeyPins, PUBLIC_KEY_PINS, "public-key-pins"),
    (PublicKeyPinsReportOnly, PUBLIC_KEY_PINS_REPORT_ONLY, "public-key-pins-report-only"),
    (Range, RANGE, "range"),
    (Referer, REFERER, "referer"),
    (ReferrerPolicy, REFERRER_POLICY, "referrer-policy"),
    (Refresh, REFRESH, "refresh"),
    (RetryAfter, RETRY_AFTER, "retry-after"),
    (SecWebSocketAccept, SEC_WEBSOCKET_ACCEPT, "sec-websocket-accept"),
    (SecWebSocketExtensions, SEC_WEBSOCKET_EXTENSIONS, "sec-websocket-extensions"),
    (SecWebSocketKey, SEC_WEBSOCKET_KEY, "sec-websocket-key"),
    (SecWebSocketProtocol, SEC_WEBSOCKET_PROTOCOL, "sec-websocket-protocol"),
    (SecWebSocketVersion, SEC_WEBSOCKET_VERSION, "sec-websocket-version"),
    (Server, SERVER, "server"),
    (SetCookie, SET_COOKIE, "set-cookie"),
    (StrictTransportSecurity, STRICT_TRANSPORT_SECURITY, "strict-transport-security"),
    (Te, TE, "te"),
    (Trailer, TRAILER, "trailer"),
    (TransferEncoding, TRANSFER_ENCODING, "transfer-encoding"),
    (UserAgent, USER_AGENT, "user-agent"),
    (Upgrade, UPGRADE, "upgrade"),
    (UpgradeInsecureRequests, UPGRADE_INSECURE_REQUESTS, "upgrade-insecure-requests"),
    (Vary, VARY, "vary"),
    (Via, VIA, "via"),
    (Warning, WARNING, "warning"),
    (WwwAuthenticate, WWW_AUTHENTICATE, "www-authenticate"),
    (XContentTypeOptions, X_CONTENT_TYPE_OPTIONS, "x-content-type-options"),
    (XDnsPrefetchControl, X_DNS_PREFETCH_CONTROL, "x-dns-prefetch-control"),
    (XFrameOptions, X_FRAME_OPTIONS, "x-frame-options"),
    (XXssProtection, X_XSS_PROTECTION, "x-xss-protection"),
}

/// The longest field name a [`FieldName`] can hold.
///
/// This implementation limit bounds storage and adapter conversion costs.
const MAX_NAME_LEN: usize = (1 << 16) - 1;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // `>` to `>=` is equivalent when assigning the same length.
const fn max_known_len() -> usize {
    let mut longest = 0;
    let mut index = 0;
    while index < KNOWN_TEXTS.len() {
        let length = KNOWN_TEXTS[index].len();
        if length > longest {
            longest = length;
        }
        index += 1;
    }
    longest
}

/// The length of the longest well-known name.
const MAX_KNOWN_LEN: usize = max_known_len();

/// Where the well-known names of each length start in [`KNOWN_BY_LENGTH`].
///
/// `LENGTH_BUCKETS[n]..LENGTH_BUCKETS[n + 1]` is the range of slots holding
/// the names of length `n`, so the table carries one extra terminating entry.
const LENGTH_BUCKETS: [usize; MAX_KNOWN_LEN + 2] = {
    let mut starts = [0; MAX_KNOWN_LEN + 2];
    let mut index = 0;
    while index < KNOWN_TEXTS.len() {
        starts[KNOWN_TEXTS[index].len() + 1] += 1;
        index += 1;
    }
    let mut length = 1;
    while length < starts.len() {
        starts[length] += starts[length - 1];
        length += 1;
    }
    starts
};

/// Well-known name indexes, grouped by name length.
const KNOWN_BY_LENGTH: [usize; KNOWN_TEXTS.len()] = {
    let mut grouped = [0; KNOWN_TEXTS.len()];
    let mut cursors = LENGTH_BUCKETS;
    let mut index = 0;
    while index < KNOWN_TEXTS.len() {
        let length = KNOWN_TEXTS[index].len();
        grouped[cursors[length]] = index;
        cursors[length] += 1;
        index += 1;
    }
    grouped
};

/// Returns the well-known names that are `length` bytes long.
///
/// The result is empty for a length no well-known name has, and `None` for a
/// length longer than every well-known name.
#[inline]
fn known_candidates(length: usize) -> Option<&'static [usize]> {
    let start = *LENGTH_BUCKETS.get(length)?;
    let end = *LENGTH_BUCKETS.get(length + 1)?;
    KNOWN_BY_LENGTH.get(start..end)
}

/// Proves the recognition tables address the same names the enum does.
///
/// Every slot of a bucket holds a name of that bucket's length, and the
/// buckets cover every well-known name exactly once, so recognition can
/// compare only the names of the input's length and still see all of them.
const _: () = {
    assert!(
        KNOWN_TEXTS.len() == FieldName::COUNT,
        "KNOWN_TEXTS length must equal FieldName::COUNT"
    );
    assert!(
        LENGTH_BUCKETS[MAX_KNOWN_LEN + 1] == KNOWN_TEXTS.len(),
        "the final length bucket must end at KNOWN_TEXTS.len()"
    );

    let mut length = 0;
    while length <= MAX_KNOWN_LEN {
        let mut slot = LENGTH_BUCKETS[length];
        assert!(
            slot <= LENGTH_BUCKETS[length + 1],
            "field-name length bucket offsets must be nondecreasing"
        );
        while slot < LENGTH_BUCKETS[length + 1] {
            assert!(
                KNOWN_TEXTS[KNOWN_BY_LENGTH[slot]].len() == length,
                "each known field name must occupy the bucket matching its length"
            );
            slot += 1;
        }
        length += 1;
    }
};

impl FieldName {
    /// Creates a name from arbitrary bytes, lowercasing them.
    ///
    /// A name is at most 65,535 bytes long.
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` is not an HTTP token, or is longer than
    /// 65,535 bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    ///
    /// assert_eq!(FieldName::try_from_bytes(b"Accept")?, FieldName::Accept);
    /// assert!(FieldName::try_from_bytes(b"bad name").is_err());
    /// # Ok::<(), http_headers::InvalidFieldName>(())
    /// ```
    pub fn try_from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidFieldName> {
        let bytes = bytes.as_ref();
        if bytes.is_empty() || bytes.len() > MAX_NAME_LEN || !bytes.iter().copied().all(crate::validate::token_byte) {
            return Err(InvalidFieldName);
        }
        if let Some(known) = Self::known_from_bytes(bytes) {
            return Ok(known);
        }
        if bytes.iter().all(|byte| !byte.is_ascii_uppercase()) {
            let lowercase = str::from_utf8(bytes).map_err(|_invalid| InvalidFieldName)?;
            return Ok(Self::Custom(Arc::from(lowercase)));
        }
        Ok(Self::Custom(lowercase_custom_name(bytes)))
    }

    /// Returns the well-known name these bytes denote, if any.
    ///
    /// The comparison is ASCII case-insensitive. Only the well-known names
    /// whose length matches `bytes` are compared: [`LENGTH_BUCKETS`] maps a
    /// length to a range of [`KNOWN_BY_LENGTH`], so recognition never walks
    /// the whole table.
    fn known_from_bytes(bytes: &[u8]) -> Option<Self> {
        for &index in known_candidates(bytes.len())? {
            let text = KNOWN_TEXTS.get(index)?;
            if eq_ignore_ascii_case(text, bytes) {
                return Self::ALL_KNOWN.get(index).cloned();
            }
        }

        None
    }

    /// Returns the lowercase field-name bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(http_headers::FieldName::Accept.as_bytes(), b"accept");
    /// ```
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.as_str().as_bytes()
    }

    /// Creates a name from bytes another HTTP implementation already
    /// validated as a lowercase field name.
    ///
    /// The bytes are recognized but never revalidated. The `http` crate
    /// accepts one byte this crate's own parser does not — `"`, which its
    /// HTTP/2 name table admits — so revalidating a name that crate produced
    /// would reject a name it considers valid.
    #[cfg(feature = "http")]
    fn from_validated_lowercase(name: &str) -> Self {
        Self::known_from_bytes(name.as_bytes()).unwrap_or_else(|| Self::Custom(Arc::from(name)))
    }

    /// Converts this name into an `http::HeaderName` without panicking.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is not one the `http` crate accepts.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::InvalidFieldName> {
    /// use http_headers::FieldName;
    ///
    /// assert_eq!(FieldName::Accept.try_to_http_name()?, http::header::ACCEPT);
    /// # Ok::<(), http_headers::InvalidFieldName>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    #[cfg(feature = "http")]
    pub fn try_to_http_name(&self) -> Result<http::HeaderName, InvalidFieldName> {
        if let Some(known) = self.http_name() {
            return Ok(known.clone());
        }
        let bytes = self.as_bytes();
        http::HeaderName::from_bytes(bytes)
            // A name that came from the `http` crate can hold `"`, which only
            // that crate's HTTP/2 table accepts.
            .or_else(|_invalid| http::HeaderName::from_lowercase(bytes))
            .map_err(|_invalid| InvalidFieldName)
    }
}

/// Keeps ordinary names stack-backed while bounding the constructor's stack frame.
fn lowercase_custom_name(bytes: &[u8]) -> Arc<str> {
    if bytes.len() <= STACK_NORMALIZATION_CAPACITY {
        let mut lowercase = [0; STACK_NORMALIZATION_CAPACITY];
        for (destination, source) in lowercase.iter_mut().zip(bytes) {
            *destination = source.to_ascii_lowercase();
        }
        let lowercase = str::from_utf8(&lowercase[..bytes.len()]).expect("validated HTTP field-name bytes are always ASCII");
        return Arc::from(lowercase);
    }

    let mut lowercase = bytes.to_vec();
    lowercase.make_ascii_lowercase();
    Arc::from(String::from_utf8(lowercase).expect("validated HTTP field-name bytes are always ASCII"))
}

impl fmt::Debug for FieldName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for FieldName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq for FieldName {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Distinct well-known variants always carry distinct names, so comparing
        // them needs no text. A custom name may still spell a well-known one,
        // which only the text can settle.
        match (self, other) {
            (Self::Custom(_), _) | (_, Self::Custom(_)) => self.as_str().eq_ignore_ascii_case(other.as_str()),
            _ => mem::discriminant(self) == mem::discriminant(other),
        }
    }
}

impl Eq for FieldName {}

impl Hash for FieldName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().len().hash(state);
        for byte in self.as_bytes() {
            byte.to_ascii_lowercase().hash(state);
        }
    }
}

impl AsRef<str> for FieldName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<[u8]> for FieldName {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq<str> for FieldName {
    fn eq(&self, other: &str) -> bool {
        self.as_str().eq_ignore_ascii_case(other)
    }
}

impl PartialEq<&str> for FieldName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str().eq_ignore_ascii_case(other)
    }
}

impl PartialEq<FieldName> for str {
    fn eq(&self, other: &FieldName) -> bool {
        self.eq_ignore_ascii_case(other.as_str())
    }
}

impl TryFrom<&[u8]> for FieldName {
    type Error = InvalidFieldName;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::try_from_bytes(bytes)
    }
}

impl TryFrom<&str> for FieldName {
    type Error = InvalidFieldName;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        Self::try_from_bytes(name.as_bytes())
    }
}

/// Compares an already lowercase name with arbitrary bytes, ignoring case.
const fn eq_ignore_ascii_case(lowercase: &str, bytes: &[u8]) -> bool {
    let lowercase = lowercase.as_bytes();
    if lowercase.len() != bytes.len() {
        return false;
    }
    let mut index = 0;
    while index < lowercase.len() {
        if lowercase[index] != bytes[index].to_ascii_lowercase() {
            return false;
        }
        index += 1;
    }
    true
}

/// Returns whether every byte is permitted in a lowercase field name.
const fn is_lowercase_name(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_uppercase() || !crate::validate::token_byte(byte) {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(feature = "http")]
mod http_conversions {
    use super::FieldName;

    impl From<&FieldName> for http::HeaderName {
        /// Converts a name into the `http` crate's own name type.
        fn from(name: &FieldName) -> Self {
            name.try_to_http_name().unwrap_or_else(|_invalid| invalid_custom_name())
        }
    }

    impl From<FieldName> for http::HeaderName {
        /// Converts a name into the `http` crate's own name type.
        fn from(name: FieldName) -> Self {
            Self::from(&name)
        }
    }

    impl From<&http::HeaderName> for FieldName {
        /// Converts an `http` name, preserving unrecognized names as
        /// [`FieldName::Custom`].
        fn from(name: &http::HeaderName) -> Self {
            Self::from_validated_lowercase(name.as_str())
        }
    }

    impl From<http::HeaderName> for FieldName {
        fn from(name: http::HeaderName) -> Self {
            Self::from(&name)
        }
    }

    /// Reports a `FieldName::Custom` that violates its structural invariant.
    ///
    /// The `Custom` variant is `#[non_exhaustive]`, so it is built only by
    /// this crate's own constructors, each of which enforces a lowercase
    /// field-name token of at most 65,535 bytes — exactly what the `http`
    /// name type accepts. No name a caller can construct reaches this, and
    /// in-crate code that built one would have broken the type's invariant.
    #[expect(
        clippy::panic,
        reason = "an infallible conversion cannot report a broken in-crate invariant any other way"
    )]
    fn invalid_custom_name() -> ! {
        panic!(
            "`FieldName::Custom` holds bytes that are not a valid HTTP field name; use `FieldName::try_to_http_name` to convert without panicking"
        );
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        #[test]
        fn invariant_failure_has_an_explicit_diagnostic() {
            let panic = std::panic::catch_unwind(super::invalid_custom_name);
            assert!(panic.is_err());
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::error::Error;
    use std::hash::{Hash, Hasher};
    use std::sync::Arc;

    use super::{FieldName, InvalidFieldName, STACK_NORMALIZATION_CAPACITY};

    #[test]
    fn known_names_are_dense_case_insensitive_and_round_trip() {
        assert_eq!(FieldName::ALL_KNOWN.len(), FieldName::COUNT);
        for (index, name) in FieldName::ALL_KNOWN.iter().enumerate() {
            assert_eq!(name.index(), Some(index));
            assert_eq!(
                FieldName::try_from_bytes(name.as_str().to_ascii_uppercase().as_bytes()).expect("known name parses"),
                *name
            );
            assert_eq!(name.as_bytes(), name.as_str().as_bytes());
            #[cfg(feature = "http")]
            assert_eq!(name.http_name().expect("known http constant").as_str(), name.as_str());
        }

        let disguised_known = FieldName::Custom(Arc::from("accept"));
        assert_eq!(disguised_known, FieldName::Accept);
        assert_eq!(FieldName::Accept, disguised_known);
        assert_eq!(
            FieldName::Custom(Arc::from("ACCEPT")),
            FieldName::Accept,
            "a custom name spelling a well-known one stays equal regardless of case"
        );
        assert_ne!(disguised_known, FieldName::AcceptEncoding);
        assert_ne!(FieldName::Accept, FieldName::AcceptEncoding);
        assert_ne!(
            FieldName::Custom(Arc::from("x-trace-id")),
            FieldName::Custom(Arc::from("x-request-id"))
        );
        assert_eq!(disguised_known.index(), None);
        #[cfg(feature = "http")]
        assert_eq!(disguised_known.http_name(), None);
    }

    #[test]
    fn custom_names_validation_formatting_hashing_and_conversions_are_consistent() {
        let error = InvalidFieldName;
        assert_eq!(error.to_string(), "invalid HTTP field name");
        let error: &dyn Error = &error;
        assert!(error.source().is_none());

        let custom = FieldName::try_from_bytes(b"X-Trace-ID").expect("valid custom name");
        assert_eq!(custom.as_str(), "x-trace-id");
        assert_eq!(custom.index(), None);
        assert_eq!(FieldName::from_static("x-trace-id"), custom);
        assert_eq!(format!("{custom:?}"), "\"x-trace-id\"");
        let lowercase = FieldName::try_from_bytes(b"x-trace-id").expect("valid lowercase custom");
        assert_eq!(lowercase, custom);
        assert_eq!(lowercase.as_str(), "x-trace-id");
        assert_eq!(custom.to_string(), "x-trace-id");
        assert_eq!(AsRef::<str>::as_ref(&custom), "x-trace-id");
        assert_eq!(AsRef::<[u8]>::as_ref(&custom), b"x-trace-id");
        assert!(PartialEq::<str>::eq(&custom, "X-TRACE-ID"));
        assert!(PartialEq::<&str>::eq(&custom, &"X-TRACE-ID"));
        assert!(<str as PartialEq<FieldName>>::eq("X-TRACE-ID", &custom));
        assert_eq!(FieldName::try_from(b"x-trace-id".as_slice()).expect("valid"), custom);
        assert_eq!(FieldName::try_from("x-trace-id").expect("valid"), custom);

        let mixed = FieldName::Custom(Arc::from("X-Trace-ID"));
        let mut lower_hash = DefaultHasher::new();
        custom.hash(&mut lower_hash);
        let mut mixed_hash = DefaultHasher::new();
        mixed.hash(&mut mixed_hash);
        assert_eq!(lower_hash.finish(), mixed_hash.finish());

        FieldName::try_from_bytes(b"").expect_err("empty name is invalid");
        FieldName::try_from_bytes(b"bad name").expect_err("space is invalid");
        std::panic::catch_unwind(|| FieldName::from_static("")).expect_err("empty static name panics");
        std::panic::catch_unwind(|| FieldName::from_static("Upper")).expect_err("uppercase static name panics");
        std::panic::catch_unwind(|| FieldName::from_static("bad name")).expect_err("invalid static token panics");

        for length in [STACK_NORMALIZATION_CAPACITY, STACK_NORMALIZATION_CAPACITY + 1] {
            let mut name = vec![b'a'; length];
            name[0] = b'X';
            assert_eq!(
                FieldName::try_from_bytes(&name).expect("valid mixed-case custom name").as_bytes(),
                name.iter().map(u8::to_ascii_lowercase).collect::<Vec<_>>()
            );
        }
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_name_conversions_cover_known_and_custom_owned_and_borrowed_paths() {
        let known_ref = http::HeaderName::from(&FieldName::Accept);
        assert_eq!(known_ref, http::header::ACCEPT);
        let known_owned = http::HeaderName::from(FieldName::Accept);
        assert_eq!(known_owned, http::header::ACCEPT);

        let custom = FieldName::from_static("x-trace-id");
        let custom_http = http::HeaderName::from(&custom);
        assert_eq!(custom_http.as_str(), "x-trace-id");
        assert_eq!(FieldName::from(&custom_http), custom);
        assert_eq!(FieldName::from(custom_http), custom);

        let invalid_custom = FieldName::Custom(Arc::from("bad name"));
        std::panic::catch_unwind(|| http::HeaderName::from(&invalid_custom))
            .expect_err("only a broken in-crate invariant reaches the panic");
        invalid_custom
            .try_to_http_name()
            .expect_err("the fallible conversion reports it instead of panicking");
    }

    #[test]
    fn case_insensitive_comparison_rejects_differing_lengths() {
        assert!(std::hint::black_box(super::eq_ignore_ascii_case)("accept", b"Accept"));
        assert!(!std::hint::black_box(super::eq_ignore_ascii_case)("accept", b"Accept-"));
        assert!(!std::hint::black_box(super::eq_ignore_ascii_case)("accept", b"Accep"));
        assert!(!std::hint::black_box(super::eq_ignore_ascii_case)("accept", b"Reject"));
    }
}
