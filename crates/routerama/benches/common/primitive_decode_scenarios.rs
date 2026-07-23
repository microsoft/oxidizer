// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Percent-encoded primitive decoding through generated static and dynamic
// paths. Query scenarios remain as controls for the path-only optimization,
// and generic FromStr scenarios preserve the fallback contract.

use std::str::FromStr;
use std::sync::OnceLock;

use routerama::query::{ErrorKind, FromQuery};
use routerama::resolve::{HttpMethod, ResolveError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GenericSigned(i8);

impl FromStr for GenericSigned {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GenericUnsigned(u8);

impl FromStr for GenericUnsigned {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GenericBool(bool);

impl FromStr for GenericBool {
    type Err = std::str::ParseBoolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GenericValue(u16);

impl FromStr for GenericValue {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[::routerama::resolve::resolver]
#[derive(Debug)]
enum StaticPrimitivePath {
    #[route(GET, "/signed/{value}")]
    Signed { value: i8 },
    #[route(GET, "/unsigned/{value}")]
    Unsigned { value: u8 },
    #[route(GET, "/bool/{value}")]
    Boolean { value: bool },
    #[route(GET, "/generic/{value}")]
    Generic { value: GenericValue },
}

#[::routerama::resolve::resolver]
#[derive(Debug)]
enum StaticControlPath {
    #[route(GET, "/signed/{value}")]
    Signed { value: GenericSigned },
    #[route(GET, "/unsigned/{value}")]
    Unsigned { value: GenericUnsigned },
    #[route(GET, "/bool/{value}")]
    Boolean { value: GenericBool },
}

#[::routerama::resolve::resolver]
#[derive(Debug)]
enum DynamicPrimitivePath {
    #[route(dynamic)]
    Signed { value: i8 },
    #[route(dynamic)]
    Unsigned { value: u8 },
    #[route(dynamic)]
    Boolean { value: bool },
    #[route(dynamic)]
    Generic { value: GenericValue },
}

#[::routerama::resolve::resolver]
#[derive(Debug)]
enum DynamicControlPath {
    #[route(dynamic)]
    Signed { value: GenericSigned },
    #[route(dynamic)]
    Unsigned { value: GenericUnsigned },
    #[route(dynamic)]
    Boolean { value: GenericBool },
}

#[derive(routerama::query::FromQuery)]
struct SignedQuery {
    value: i8,
}

#[derive(routerama::query::FromQuery)]
struct UnsignedQuery {
    value: u8,
}

#[derive(routerama::query::FromQuery)]
struct BooleanQuery {
    value: bool,
}

#[derive(routerama::query::FromQuery)]
struct GenericQuery {
    value: GenericValue,
}

#[derive(routerama::query::FromQuery)]
struct SignedControlQuery {
    value: GenericSigned,
}

#[derive(routerama::query::FromQuery)]
struct UnsignedControlQuery {
    value: GenericUnsigned,
}

#[derive(routerama::query::FromQuery)]
struct BooleanControlQuery {
    value: GenericBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Path,
    DynamicPath,
    Query,
}

impl Source {
    const ALL: [Self; 3] = [Self::Path, Self::DynamicPath, Self::Query];
    const COUNT: usize = Self::ALL.len();

    const fn name(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::DynamicPath => "dynamic_path",
            Self::Query => "query",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    SignedSuccess,
    SignedSuccessControl,
    SignedZero,
    SignedPlus,
    SignedMin,
    SignedMax,
    SignedOverflow,
    UnsignedSuccess,
    UnsignedSuccessControl,
    UnsignedZero,
    UnsignedPlus,
    UnsignedMax,
    UnsignedOverflow,
    BoolSuccess,
    BoolSuccessControl,
    BoolFalse,
    BoolInvalid,
    MalformedEncoding,
    InvalidUtf8,
    GenericFromStr,
    SignedUnescaped,
    SignedUnescapedControl,
    UnsignedUnescaped,
    UnsignedUnescapedControl,
    BoolUnescaped,
    BoolUnescapedControl,
    GenericUnescaped,
}

impl Scenario {
    const ALL: [Self; 27] = [
        Self::SignedSuccess,
        Self::SignedSuccessControl,
        Self::SignedZero,
        Self::SignedPlus,
        Self::SignedMin,
        Self::SignedMax,
        Self::SignedOverflow,
        Self::UnsignedSuccess,
        Self::UnsignedSuccessControl,
        Self::UnsignedZero,
        Self::UnsignedPlus,
        Self::UnsignedMax,
        Self::UnsignedOverflow,
        Self::BoolSuccess,
        Self::BoolSuccessControl,
        Self::BoolFalse,
        Self::BoolInvalid,
        Self::MalformedEncoding,
        Self::InvalidUtf8,
        Self::GenericFromStr,
        Self::SignedUnescaped,
        Self::SignedUnescapedControl,
        Self::UnsignedUnescaped,
        Self::UnsignedUnescapedControl,
        Self::BoolUnescaped,
        Self::BoolUnescapedControl,
        Self::GenericUnescaped,
    ];
    const COUNT: usize = Self::ALL.len();

    const fn name(self) -> &'static str {
        match self {
            Self::SignedSuccess => "signed_success",
            Self::SignedSuccessControl => "signed_success_control",
            Self::SignedZero => "signed_zero",
            Self::SignedPlus => "signed_plus",
            Self::SignedMin => "signed_min",
            Self::SignedMax => "signed_max",
            Self::SignedOverflow => "signed_overflow",
            Self::UnsignedSuccess => "unsigned_success",
            Self::UnsignedSuccessControl => "unsigned_success_control",
            Self::UnsignedZero => "unsigned_zero",
            Self::UnsignedPlus => "unsigned_plus",
            Self::UnsignedMax => "unsigned_max",
            Self::UnsignedOverflow => "unsigned_overflow",
            Self::BoolSuccess => "bool_success",
            Self::BoolSuccessControl => "bool_success_control",
            Self::BoolFalse => "bool_false",
            Self::BoolInvalid => "bool_invalid",
            Self::MalformedEncoding => "malformed_encoding",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::GenericFromStr => "generic_from_str",
            Self::SignedUnescaped => "signed_unescaped",
            Self::SignedUnescapedControl => "signed_unescaped_control",
            Self::UnsignedUnescaped => "unsigned_unescaped",
            Self::UnsignedUnescapedControl => "unsigned_unescaped_control",
            Self::BoolUnescaped => "bool_unescaped",
            Self::BoolUnescapedControl => "bool_unescaped_control",
            Self::GenericUnescaped => "generic_unescaped",
        }
    }

    const fn static_path(self) -> &'static str {
        match self {
            Self::SignedSuccess | Self::SignedSuccessControl => "/signed/%2D%34%32",
            Self::SignedZero => "/signed/%30",
            Self::SignedPlus => "/signed/%2B%34%32",
            Self::SignedMin => "/signed/%2D%31%32%38",
            Self::SignedMax => "/signed/%31%32%37",
            Self::SignedOverflow => "/signed/%31%32%38",
            Self::UnsignedSuccess | Self::UnsignedSuccessControl => "/unsigned/%34%32",
            Self::UnsignedZero => "/unsigned/%30",
            Self::UnsignedPlus => "/unsigned/%2B%34%32",
            Self::UnsignedMax => "/unsigned/%32%35%35",
            Self::UnsignedOverflow => "/unsigned/%32%35%36",
            Self::BoolSuccess | Self::BoolSuccessControl => "/bool/%74%72%75%65",
            Self::BoolFalse => "/bool/%66%61%6C%73%65",
            Self::BoolInvalid => "/bool/%79%65%73",
            Self::MalformedEncoding => "/signed/%2G",
            Self::InvalidUtf8 => "/signed/%FF",
            Self::GenericFromStr => "/generic/%34%32",
            Self::SignedUnescaped | Self::SignedUnescapedControl => "/signed/-42",
            Self::UnsignedUnescaped | Self::UnsignedUnescapedControl => "/unsigned/42",
            Self::BoolUnescaped | Self::BoolUnescapedControl => "/bool/true",
            Self::GenericUnescaped => "/generic/42",
        }
    }

    const fn dynamic_path(self) -> &'static str {
        match self {
            Self::SignedSuccess | Self::SignedSuccessControl => "/dynamic/signed/%2D%34%32",
            Self::SignedZero => "/dynamic/signed/%30",
            Self::SignedPlus => "/dynamic/signed/%2B%34%32",
            Self::SignedMin => "/dynamic/signed/%2D%31%32%38",
            Self::SignedMax => "/dynamic/signed/%31%32%37",
            Self::SignedOverflow => "/dynamic/signed/%31%32%38",
            Self::UnsignedSuccess | Self::UnsignedSuccessControl => "/dynamic/unsigned/%34%32",
            Self::UnsignedZero => "/dynamic/unsigned/%30",
            Self::UnsignedPlus => "/dynamic/unsigned/%2B%34%32",
            Self::UnsignedMax => "/dynamic/unsigned/%32%35%35",
            Self::UnsignedOverflow => "/dynamic/unsigned/%32%35%36",
            Self::BoolSuccess | Self::BoolSuccessControl => "/dynamic/bool/%74%72%75%65",
            Self::BoolFalse => "/dynamic/bool/%66%61%6C%73%65",
            Self::BoolInvalid => "/dynamic/bool/%79%65%73",
            Self::MalformedEncoding => "/dynamic/signed/%2G",
            Self::InvalidUtf8 => "/dynamic/signed/%FF",
            Self::GenericFromStr => "/dynamic/generic/%34%32",
            Self::SignedUnescaped | Self::SignedUnescapedControl => "/dynamic/signed/-42",
            Self::UnsignedUnescaped | Self::UnsignedUnescapedControl => "/dynamic/unsigned/42",
            Self::BoolUnescaped | Self::BoolUnescapedControl => "/dynamic/bool/true",
            Self::GenericUnescaped => "/dynamic/generic/42",
        }
    }

    const fn query(self) -> &'static str {
        match self {
            Self::SignedSuccess | Self::SignedSuccessControl => "value=%2D%34%32",
            Self::SignedZero | Self::UnsignedZero => "value=%30",
            Self::SignedPlus | Self::UnsignedPlus => "value=%2B%34%32",
            Self::SignedMin => "value=%2D%31%32%38",
            Self::SignedMax => "value=%31%32%37",
            Self::SignedOverflow => "value=%31%32%38",
            Self::UnsignedSuccess | Self::UnsignedSuccessControl | Self::GenericFromStr => "value=%34%32",
            Self::UnsignedMax => "value=%32%35%35",
            Self::UnsignedOverflow => "value=%32%35%36",
            Self::BoolSuccess | Self::BoolSuccessControl => "value=%74%72%75%65",
            Self::BoolFalse => "value=%66%61%6C%73%65",
            Self::BoolInvalid => "value=%79%65%73",
            Self::MalformedEncoding => "value=%2G",
            Self::InvalidUtf8 => "value=%FF",
            Self::SignedUnescaped | Self::SignedUnescapedControl => "value=-42",
            Self::UnsignedUnescaped | Self::UnsignedUnescapedControl | Self::GenericUnescaped => "value=42",
            Self::BoolUnescaped | Self::BoolUnescapedControl => "value=true",
        }
    }

    const fn expected(self) -> Observation {
        match self {
            Self::SignedSuccess | Self::SignedSuccessControl | Self::SignedUnescaped | Self::SignedUnescapedControl => {
                Observation::Signed(-42)
            }
            Self::SignedZero => Observation::Signed(0),
            Self::SignedPlus => Observation::Signed(42),
            Self::SignedMin => Observation::Signed(i8::MIN),
            Self::SignedMax => Observation::Signed(i8::MAX),
            Self::UnsignedSuccess
            | Self::UnsignedSuccessControl
            | Self::UnsignedPlus
            | Self::UnsignedUnescaped
            | Self::UnsignedUnescapedControl => {
                Observation::Unsigned(42)
            }
            Self::UnsignedZero => Observation::Unsigned(0),
            Self::UnsignedMax => Observation::Unsigned(u8::MAX),
            Self::BoolSuccess | Self::BoolSuccessControl | Self::BoolUnescaped | Self::BoolUnescapedControl => {
                Observation::Boolean(true)
            }
            Self::BoolFalse => Observation::Boolean(false),
            Self::GenericFromStr | Self::GenericUnescaped => Observation::Generic(42),
            Self::SignedOverflow | Self::UnsignedOverflow | Self::BoolInvalid => Observation::InvalidValue,
            Self::MalformedEncoding | Self::InvalidUtf8 => Observation::InvalidEncoding,
        }
    }

    const fn is_unescaped(self) -> bool {
        matches!(
            self,
            Self::SignedUnescaped
                | Self::SignedUnescapedControl
                | Self::UnsignedUnescaped
                | Self::UnsignedUnescapedControl
                | Self::BoolUnescaped
                | Self::BoolUnescapedControl
                | Self::GenericUnescaped
        )
    }

    const fn is_control(self) -> bool {
        matches!(
            self,
            Self::SignedSuccessControl
                | Self::UnsignedSuccessControl
                | Self::BoolSuccessControl
                | Self::SignedUnescapedControl
                | Self::UnsignedUnescapedControl
                | Self::BoolUnescapedControl
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Signed(i8),
    Unsigned(u8),
    Boolean(bool),
    Generic(u16),
    InvalidValue,
    InvalidEncoding,
}

struct Fixtures {
    path: StaticPrimitivePathResolver,
    dynamic_path: DynamicPrimitivePathResolver,
}

static DYNAMIC_CONTROL: OnceLock<DynamicControlPathResolver> = OnceLock::new();

fn prepare() -> Fixtures {
    let _ = dynamic_control();
    Fixtures {
        path: StaticPrimitivePath::resolver(),
        dynamic_path: DynamicPrimitivePath::builder()
            .add_signed(HttpMethod::GET, "/dynamic/signed/{value}")
            .add_unsigned(HttpMethod::GET, "/dynamic/unsigned/{value}")
            .add_boolean(HttpMethod::GET, "/dynamic/bool/{value}")
            .add_generic(HttpMethod::GET, "/dynamic/generic/{value}")
            .build()
            .expect("the primitive dynamic routes are valid"),
    }
}

fn dynamic_control() -> &'static DynamicControlPathResolver {
    DYNAMIC_CONTROL.get_or_init(|| {
        DynamicControlPath::builder()
            .add_signed(HttpMethod::GET, "/dynamic/signed/{value}")
            .add_unsigned(HttpMethod::GET, "/dynamic/unsigned/{value}")
            .add_boolean(HttpMethod::GET, "/dynamic/bool/{value}")
            .build()
            .expect("the primitive dynamic control routes are valid")
    })
}

fn run(fixtures: &Fixtures, source: Source, scenario: Scenario) -> Observation {
    match source {
        Source::Path if scenario.is_control() => run_static_control(&StaticControlPath::resolver(), scenario.static_path()),
        Source::Path => run_static_path(&fixtures.path, scenario.static_path()),
        Source::DynamicPath if scenario.is_control() => run_dynamic_control(dynamic_control(), scenario.dynamic_path()),
        Source::DynamicPath => run_dynamic_path(&fixtures.dynamic_path, scenario.dynamic_path()),
        Source::Query => run_query(scenario),
    }
}

#[expect(
    clippy::panic,
    reason = "unexpected resolver failures indicate a malformed benchmark fixture"
)]
fn run_static_path(resolver: &StaticPrimitivePathResolver, path: &str) -> Observation {
    match resolver.resolve("GET", path) {
        Ok(StaticPrimitivePath::Signed { value }) => Observation::Signed(value),
        Ok(StaticPrimitivePath::Unsigned { value }) => Observation::Unsigned(value),
        Ok(StaticPrimitivePath::Boolean { value }) => Observation::Boolean(value),
        Ok(StaticPrimitivePath::Generic { value }) => Observation::Generic(value.0),
        Err(ResolveError::InvalidCapture(_)) => Observation::InvalidValue,
        Err(ResolveError::UndecodableCapture(_)) => Observation::InvalidEncoding,
        Err(error) => panic!("the primitive path fixture produced an unexpected error: {error}"),
    }
}

#[expect(
    clippy::panic,
    reason = "unexpected resolver failures indicate a malformed benchmark fixture"
)]
fn run_static_control(resolver: &StaticControlPathResolver, path: &str) -> Observation {
    match resolver.resolve("GET", path) {
        Ok(StaticControlPath::Signed { value }) => Observation::Signed(value.0),
        Ok(StaticControlPath::Unsigned { value }) => Observation::Unsigned(value.0),
        Ok(StaticControlPath::Boolean { value }) => Observation::Boolean(value.0),
        Err(ResolveError::InvalidCapture(_)) => Observation::InvalidValue,
        Err(ResolveError::UndecodableCapture(_)) => Observation::InvalidEncoding,
        Err(error) => panic!("the primitive static control fixture produced an unexpected error: {error}"),
    }
}

#[expect(
    clippy::panic,
    reason = "unexpected resolver failures indicate a malformed benchmark fixture"
)]
fn run_dynamic_path(resolver: &DynamicPrimitivePathResolver, path: &str) -> Observation {
    match resolver.resolve("GET", path) {
        Ok(DynamicPrimitivePath::Signed { value }) => Observation::Signed(value),
        Ok(DynamicPrimitivePath::Unsigned { value }) => Observation::Unsigned(value),
        Ok(DynamicPrimitivePath::Boolean { value }) => Observation::Boolean(value),
        Ok(DynamicPrimitivePath::Generic { value }) => Observation::Generic(value.0),
        Err(ResolveError::InvalidCapture(_)) => Observation::InvalidValue,
        Err(ResolveError::UndecodableCapture(_)) => Observation::InvalidEncoding,
        Err(error) => panic!("the primitive dynamic path fixture produced an unexpected error: {error}"),
    }
}

#[expect(
    clippy::panic,
    reason = "unexpected resolver failures indicate a malformed benchmark fixture"
)]
fn run_dynamic_control(resolver: &DynamicControlPathResolver, path: &str) -> Observation {
    match resolver.resolve("GET", path) {
        Ok(DynamicControlPath::Signed { value }) => Observation::Signed(value.0),
        Ok(DynamicControlPath::Unsigned { value }) => Observation::Unsigned(value.0),
        Ok(DynamicControlPath::Boolean { value }) => Observation::Boolean(value.0),
        Err(ResolveError::InvalidCapture(_)) => Observation::InvalidValue,
        Err(ResolveError::UndecodableCapture(_)) => Observation::InvalidEncoding,
        Err(error) => panic!("the primitive dynamic control fixture produced an unexpected error: {error}"),
    }
}

#[expect(
    clippy::panic,
    reason = "unexpected query failures indicate a malformed benchmark fixture"
)]
fn run_query(scenario: Scenario) -> Observation {
    let result = match scenario {
        Scenario::SignedSuccess
        | Scenario::SignedZero
        | Scenario::SignedPlus
        | Scenario::SignedMin
        | Scenario::SignedMax
        | Scenario::SignedOverflow
        | Scenario::MalformedEncoding
        | Scenario::InvalidUtf8
        | Scenario::SignedUnescaped => SignedQuery::from_query(scenario.query()).map(|value| Observation::Signed(value.value)),
        Scenario::SignedSuccessControl | Scenario::SignedUnescapedControl => {
            SignedControlQuery::from_query(scenario.query()).map(|value| Observation::Signed(value.value.0))
        }
        Scenario::UnsignedSuccess
        | Scenario::UnsignedZero
        | Scenario::UnsignedPlus
        | Scenario::UnsignedMax
        | Scenario::UnsignedOverflow
        | Scenario::UnsignedUnescaped => UnsignedQuery::from_query(scenario.query()).map(|value| Observation::Unsigned(value.value)),
        Scenario::UnsignedSuccessControl | Scenario::UnsignedUnescapedControl => {
            UnsignedControlQuery::from_query(scenario.query()).map(|value| Observation::Unsigned(value.value.0))
        }
        Scenario::BoolSuccess
        | Scenario::BoolFalse
        | Scenario::BoolInvalid
        | Scenario::BoolUnescaped => {
            BooleanQuery::from_query(scenario.query()).map(|value| Observation::Boolean(value.value))
        }
        Scenario::BoolSuccessControl | Scenario::BoolUnescapedControl => {
            BooleanControlQuery::from_query(scenario.query()).map(|value| Observation::Boolean(value.value.0))
        }
        Scenario::GenericFromStr | Scenario::GenericUnescaped => {
            GenericQuery::from_query(scenario.query()).map(|value| Observation::Generic(value.value.0))
        }
    };
    match result {
        Ok(observation) => observation,
        Err(error) if error.kind() == ErrorKind::InvalidValue => Observation::InvalidValue,
        Err(error) if matches!(error.kind(), ErrorKind::InvalidEncoding | ErrorKind::InvalidUtf8) => Observation::InvalidEncoding,
        Err(error) => panic!("the primitive query fixture produced an unexpected error: {error}"),
    }
}

fn assert_equivalent() {
    let fixtures = prepare();
    for source in Source::ALL {
        for scenario in Scenario::ALL {
            assert_eq!(
                run(&fixtures, source, scenario),
                scenario.expected(),
                "{}/{} changed its decoded value or failure category",
                source.name(),
                scenario.name()
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

fn allocation_diagnostics() -> [[AllocationStats; Scenario::COUNT]; Source::COUNT] {
    let fixtures = prepare();
    Source::ALL.map(|source| {
        Scenario::ALL.map(|scenario| {
            let session = alloc_tracker::Session::new().no_stdout().no_file();
            let operation = session.operation("decode");
            {
                let _span = operation.measure_thread().iterations(1);
                std::hint::black_box(run(&fixtures, source, scenario));
            }
            let report = session.to_report();
            let (_, operation) = report
                .operations()
                .find(|(name, _)| *name == "decode")
                .expect("the decode allocation operation is recorded");
            AllocationStats {
                allocations: operation.total_allocations_count(),
                bytes: operation.total_bytes_allocated(),
            }
        })
    })
}
