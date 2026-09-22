// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authority inspection for routing, allow-list comparison, and forwarding.
//!
//! Each pair compares retained components with explicit consumer-side parsing
//! of the existing textual accessors. Both use the current header decoder, so
//! these are workload comparisons, not historical decoder baselines. The
//! `decode` groups include validation; `reads` groups reuse predecoded values.

use std::borrow::Cow;
use std::hint::black_box;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use criterion::{BatchSize, BenchmarkId, Criterion};
use http_headers::headers::{
    AccessControlAllowOrigin, AccessControlAllowOriginKind, AccessControlAllowOriginOwned, AccessControlAllowOriginView, Host, HostKind,
    HostOwned, HostView, OriginHost, OriginScheme, PortConversionError, PortConversionErrorKind,
};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeMode, Field, FieldName, FieldValue, SingleValueField};

#[derive(Debug, Eq, PartialEq)]
enum HostSemantic<'a> {
    Name(Cow<'a, str>),
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
    Future(&'a str, &'a str),
}

fn retained_host<'a>(view: &'a HostView<'_>) -> (HostSemantic<'a>, Result<Option<u16>, PortConversionErrorKind>) {
    let host = match view.kind() {
        HostKind::RegisteredName(name) => HostSemantic::Name(Cow::Borrowed(name.normalized())),
        HostKind::Ipv4(address) => HostSemantic::Ipv4(address),
        HostKind::Ipv6(address) => HostSemantic::Ipv6(address),
        HostKind::IpvFuture(future) => HostSemantic::Future(future.version(), future.address()),
    };
    (host, view.network_port().map_err(PortConversionError::kind))
}

fn consumer_host<'a>(view: &HostView<'a>) -> (HostSemantic<'a>, Result<Option<u16>, PortConversionErrorKind>) {
    let host = view.host();
    let semantic = if let Some(literal) = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')) {
        if let Some(future) = literal.strip_prefix('v').or_else(|| literal.strip_prefix('V')) {
            let (version, address) = future.split_once('.').expect("validated IPvFuture contains a version delimiter");
            HostSemantic::Future(version, address)
        } else {
            HostSemantic::Ipv6(literal.parse().expect("validated IPv6 parses"))
        }
    } else if !host.is_ascii() {
        HostSemantic::Name(Cow::Owned(
            idna::domain_to_ascii(host).expect("relaxed host already passed IDNA validation"),
        ))
    } else if let Ok(address) = host.parse::<Ipv4Addr>() {
        HostSemantic::Ipv4(address)
    } else {
        HostSemantic::Name(Cow::Borrowed(host))
    };
    let port = view
        .port()
        .map(|port| {
            if port.is_empty() {
                Err(PortConversionErrorKind::Empty)
            } else {
                port.parse::<u16>().map_err(|_invalid| PortConversionErrorKind::Overflow)
            }
        })
        .transpose();
    (semantic, port)
}

struct HostFixture {
    wire: FieldValue,
    mode: DecodeMode,
    owned: HostOwned,
}

impl HostFixture {
    fn new(wire: &'static str, mode: DecodeMode) -> Self {
        let wire = FieldValue::try_from(wire).expect("fixture is a field value");
        let owned = <Host as SingleValueField>::decode_owned_with(wire.clone(), mode).expect("fixture is a valid host");
        let view = owned.as_view();
        assert_eq!(retained_host(&view), consumer_host(&view));
        assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        drop(view);
        Self { wire, mode, owned }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum OriginSemantic<'a> {
    Wildcard,
    Null,
    Tuple(OriginScheme, HostSemantic<'a>, Option<u16>, u16),
}

fn retained_origin(view: AccessControlAllowOriginView<'_>) -> OriginSemantic<'_> {
    match view.kind() {
        AccessControlAllowOriginKind::Wildcard => OriginSemantic::Wildcard,
        AccessControlAllowOriginKind::Null => OriginSemantic::Null,
        AccessControlAllowOriginKind::Origin(origin) => {
            let host = match origin.host() {
                OriginHost::Domain(domain) => HostSemantic::Name(Cow::Borrowed(domain.as_str())),
                OriginHost::Ipv4(address) => HostSemantic::Ipv4(address),
                OriginHost::Ipv6(address) => HostSemantic::Ipv6(address),
            };
            OriginSemantic::Tuple(origin.scheme(), host, origin.port(), origin.effective_port())
        }
    }
}

#[expect(clippy::panic, reason = "the benchmark consumes only previously validated tuple-origin schemes")]
fn consumer_origin(view: AccessControlAllowOriginView<'_>) -> OriginSemantic<'_> {
    let serialized = view.as_str();
    if serialized == "*" {
        return OriginSemantic::Wildcard;
    }
    if serialized == "null" {
        return OriginSemantic::Null;
    }
    let (scheme, authority) = serialized.split_once("://").expect("validated tuple origin has a scheme");
    let scheme = match scheme {
        "ftp" => OriginScheme::Ftp,
        "http" => OriginScheme::Http,
        "https" => OriginScheme::Https,
        "ws" => OriginScheme::Ws,
        "wss" => OriginScheme::Wss,
        _ => panic!("validated tuple-origin scheme"),
    };
    let (host, port) = if let Some(literal) = authority.strip_prefix('[') {
        let (host, suffix) = literal.split_once(']').expect("validated IPv6 has a closing bracket");
        let address = host.parse().expect("validated origin IPv6 parses");
        (HostSemantic::Ipv6(address), suffix.strip_prefix(':'))
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        let host = host
            .strip_suffix('.')
            .unwrap_or(host)
            .parse::<Ipv4Addr>()
            .map_or_else(|_| HostSemantic::Name(Cow::Borrowed(host)), HostSemantic::Ipv4);
        (host, port)
    };
    let port = port.map(|port| port.parse::<u16>().expect("validated origin port fits u16"));
    OriginSemantic::Tuple(scheme, host, port, port.unwrap_or_else(|| scheme.default_port()))
}

struct OriginFixture {
    wire: FieldValue,
    owned: AccessControlAllowOriginOwned,
}

impl OriginFixture {
    fn new(wire: &'static str) -> Self {
        let wire = FieldValue::try_from(wire).expect("fixture is a field value");
        let owned = AccessControlAllowOriginOwned::try_from(wire.clone()).expect("fixture is a valid origin");
        let view = owned.as_view();
        assert_eq!(retained_origin(view), consumer_origin(view));
        assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        Self { wire, owned }
    }
}

impl FieldSource for OriginFixture {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == &FieldName::AccessControlAllowOrigin).then(|| FieldLines::single(name, self.wire.as_bytes()))
    }
}

macro_rules! host_cases {
    ($(($id:ident, $wire:literal, $mode:ident)),+ $(,)?) => {
        $(
            fn $id() -> &'static HostFixture {
                static FIXTURE: OnceLock<HostFixture> = OnceLock::new();
                FIXTURE.get_or_init(|| HostFixture::new($wire, DecodeMode::$mode))
            }
        )+

        #[metabench::benchmark(HOST_DECODE_RETAINED, "http_headers_authority_semantics/host_decode", "retained")]
        $(#[bench::$id(setup = $id)])+
        fn host_decode_retained(fixture: &HostFixture) {
            let view = <Host as SingleValueField>::decode_view_with(black_box(fixture.wire.as_field_value_ref()), fixture.mode)
                .expect("validated fixture");
            drop(black_box(retained_host(black_box(&view))));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(HOST_DECODE_CONSUMER, "http_headers_authority_semantics/host_decode", "consumer_parse")]
        $(#[bench::$id(setup = $id)])+
        fn host_decode_consumer(fixture: &HostFixture) {
            let view = <Host as SingleValueField>::decode_view_with(black_box(fixture.wire.as_field_value_ref()), fixture.mode)
                .expect("validated fixture");
            drop(black_box(consumer_host(black_box(&view))));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(HOST_READS_RETAINED, "http_headers_authority_semantics/host_reads", "retained")]
        $(#[bench::$id(setup = $id)])+
        fn host_reads_retained(fixture: &HostFixture) {
            let view = black_box(&fixture.owned).as_view();
            drop(black_box(retained_host(black_box(&view))));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(HOST_READS_CONSUMER, "http_headers_authority_semantics/host_reads", "consumer_parse")]
        $(#[bench::$id(setup = $id)])+
        fn host_reads_consumer(fixture: &HostFixture) {
            let view = black_box(&fixture.owned).as_view();
            drop(black_box(consumer_host(black_box(&view))));
            black_box(view.as_field_value().as_bytes());
        }

        fn host_benchmarks(criterion: &mut Criterion) {
            let mut group = criterion.benchmark_group("http_headers_authority_semantics/host_decode");
            $(
                group.bench_function(BenchmarkId::new("retained", stringify!($id)),
                    |b| b.iter_batched($id, host_decode_retained, BatchSize::SmallInput));
                group.bench_function(BenchmarkId::new("consumer_parse", stringify!($id)),
                    |b| b.iter_batched($id, host_decode_consumer, BatchSize::SmallInput));
            )+
            group.finish();
            let mut group = criterion.benchmark_group("http_headers_authority_semantics/host_reads");
            $(
                group.bench_function(BenchmarkId::new("retained", stringify!($id)),
                    |b| b.iter_batched($id, host_reads_retained, BatchSize::SmallInput));
                group.bench_function(BenchmarkId::new("consumer_parse", stringify!($id)),
                    |b| b.iter_batched($id, host_reads_consumer, BatchSize::SmallInput));
            )+
            group.finish();
        }
    };
}

macro_rules! origin_cases {
    ($(($id:ident, $wire:literal)),+ $(,)?) => {
        $(
            fn $id() -> &'static OriginFixture {
                static FIXTURE: OnceLock<OriginFixture> = OnceLock::new();
                FIXTURE.get_or_init(|| OriginFixture::new($wire))
            }
        )+

        #[metabench::benchmark(ORIGIN_DECODE_RETAINED, "http_headers_authority_semantics/origin_decode", "retained")]
        $(#[bench::$id(setup = $id)])+
        fn origin_decode_retained(fixture: &OriginFixture) {
            let view = <AccessControlAllowOrigin as Field>::view(black_box(fixture)).expect("validated fixture").expect("present");
            black_box(retained_origin(black_box(view)));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(ORIGIN_DECODE_CONSUMER, "http_headers_authority_semantics/origin_decode", "consumer_parse")]
        $(#[bench::$id(setup = $id)])+
        fn origin_decode_consumer(fixture: &OriginFixture) {
            let view = <AccessControlAllowOrigin as Field>::view(black_box(fixture)).expect("validated fixture").expect("present");
            black_box(consumer_origin(black_box(view)));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(ORIGIN_READS_RETAINED, "http_headers_authority_semantics/origin_reads", "retained")]
        $(#[bench::$id(setup = $id)])+
        fn origin_reads_retained(fixture: &OriginFixture) {
            let view = black_box(&fixture.owned).as_view();
            black_box(retained_origin(black_box(view)));
            black_box(view.as_field_value().as_bytes());
        }

        #[metabench::benchmark(ORIGIN_READS_CONSUMER, "http_headers_authority_semantics/origin_reads", "consumer_parse")]
        $(#[bench::$id(setup = $id)])+
        fn origin_reads_consumer(fixture: &OriginFixture) {
            let view = black_box(&fixture.owned).as_view();
            black_box(consumer_origin(black_box(view)));
            black_box(view.as_field_value().as_bytes());
        }

        fn origin_benchmarks(criterion: &mut Criterion) {
            let mut group = criterion.benchmark_group("http_headers_authority_semantics/origin_decode");
            $(
                group.bench_function(BenchmarkId::new("retained", stringify!($id)),
                    |b| b.iter_batched($id, origin_decode_retained, BatchSize::SmallInput));
                group.bench_function(BenchmarkId::new("consumer_parse", stringify!($id)),
                    |b| b.iter_batched($id, origin_decode_consumer, BatchSize::SmallInput));
            )+
            group.finish();
            let mut group = criterion.benchmark_group("http_headers_authority_semantics/origin_reads");
            $(
                group.bench_function(BenchmarkId::new("retained", stringify!($id)),
                    |b| b.iter_batched($id, origin_reads_retained, BatchSize::SmallInput));
                group.bench_function(BenchmarkId::new("consumer_parse", stringify!($id)),
                    |b| b.iter_batched($id, origin_reads_consumer, BatchSize::SmallInput));
            )+
            group.finish();
        }
    };
}

host_cases!(
    (domain, "api.example.com:8443", Strict),
    (ipv4, "192.0.2.1:443", Strict),
    (ipv6, "[2001:db8::1]:443", Strict),
    (ipvfuture, "[vFFFFFFFFFFFFFFFFFFFF.alpha:beta]:443", Strict),
    (numeric_name, "192.168.001.1:80", Strict),
    (percent_name, "api%2Eexample.com:443", Strict),
    (empty_port, "example.com:", Strict),
    (overflow_port, "example.com:65536", Strict),
    (leading_zero_port, "example.com:0000000000000000000000443", Strict),
    (idna_name, "münich.example:443", Relaxed),
);

origin_cases!(
    (origin_wildcard, "*"),
    (origin_null, "null"),
    (origin_domain, "https://api.example.com"),
    (origin_explicit_port, "https://api.example.com:8443"),
    (origin_ipv4, "https://192.0.2.1:8443"),
    (origin_ipv6, "https://[2001:db8::1]:8443"),
);

fn criterion_benchmarks(criterion: &mut Criterion) {
    host_benchmarks(criterion);
    origin_benchmarks(criterion);
}

metabench::main!(
    criterion = {
        factory = Criterion::default,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [
        HOST_DECODE_RETAINED,
        HOST_DECODE_CONSUMER,
        HOST_READS_RETAINED,
        HOST_READS_CONSUMER,
        ORIGIN_DECODE_RETAINED,
        ORIGIN_DECODE_CONSUMER,
        ORIGIN_READS_RETAINED,
        ORIGIN_READS_CONSUMER,
    ],
);
