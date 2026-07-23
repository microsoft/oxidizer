// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Mixed static/dynamic resolver scenarios. Resolver construction and path
// ownership are excluded. Boxed state prevents large deep-route setup values
// from being moved through Gungraun's measured function boundary.

use std::hint::black_box;

use routerama::resolve::HttpMethod;

const SHORT_STATIC_HIT: &str = "/short/static";
const SHORT_DYNAMIC_HIT: &str = "/short/plugin";
const SHORT_MISS: &str = "/short/missing";
const DEEP_17_STATIC_HIT: &str = "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/static";
const DEEP_17_DYNAMIC_HIT: &str = "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/plugin";
const DEEP_17_MISS: &str = "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/missing";
const DEEP_32_STATIC_HIT: &str =
    "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/s17/s18/s19/s20/s21/s22/s23/s24/s25/s26/s27/s28/s29/s30/s31/static";
const DEEP_32_DYNAMIC_HIT: &str =
    "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/s17/s18/s19/s20/s21/s22/s23/s24/s25/s26/s27/s28/s29/s30/s31/plugin";
const DEEP_32_MISS: &str =
    "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/s17/s18/s19/s20/s21/s22/s23/s24/s25/s26/s27/s28/s29/s30/s31/missing";

#[::routerama::resolve::resolver]
#[derive(Debug)]
enum MixedScenario {
    #[route(GET, "/short/static")]
    ShortStatic,
    #[route(GET, "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/static")]
    Deep17Static,
    #[route(
        GET,
        "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/s17/s18/s19/s20/s21/s22/s23/s24/s25/s26/s27/s28/s29/s30/s31/static"
    )]
    Deep32Static,
    #[route(dynamic)]
    ShortDynamic,
    #[route(dynamic)]
    Deep17Dynamic,
    #[route(dynamic)]
    Deep32Dynamic,
}

#[expect(
    clippy::unnecessary_box_returns,
    reason = "boxed resolver state keeps its runtime route tables out of the measured setup-state move"
)]
fn build_mixed_scenario() -> Box<MixedScenarioResolver> {
    Box::new(
        MixedScenario::builder()
            .add_short_dynamic(HttpMethod::GET, "/short/plugin")
            .add_deep17_dynamic(
                HttpMethod::GET,
                "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/plugin",
            )
            .add_deep32_dynamic(
                HttpMethod::GET,
                "/s01/s02/s03/s04/s05/s06/s07/s08/s09/s10/s11/s12/s13/s14/s15/s16/s17/s18/s19/s20/s21/s22/s23/s24/s25/s26/s27/s28/s29/s30/s31/plugin",
            )
            .build()
            .expect("mixed scenario builds"),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    ShortStaticHit,
    ShortDynamicHit,
    ShortMiss,
    Deep17StaticHit,
    Deep17DynamicHit,
    Deep17Miss,
    Deep32StaticHit,
    Deep32DynamicHit,
    Deep32Miss,
}

impl Scenario {
    const ALL: [Self; 9] = [
        Self::ShortStaticHit,
        Self::ShortDynamicHit,
        Self::ShortMiss,
        Self::Deep17StaticHit,
        Self::Deep17DynamicHit,
        Self::Deep17Miss,
        Self::Deep32StaticHit,
        Self::Deep32DynamicHit,
        Self::Deep32Miss,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::ShortStaticHit => "short_static_hit",
            Self::ShortDynamicHit => "short_dynamic_hit",
            Self::ShortMiss => "short_miss",
            Self::Deep17StaticHit => "segments_17_static_hit",
            Self::Deep17DynamicHit => "segments_17_dynamic_hit",
            Self::Deep17Miss => "segments_17_miss",
            Self::Deep32StaticHit => "segments_32_static_hit",
            Self::Deep32DynamicHit => "segments_32_dynamic_hit",
            Self::Deep32Miss => "segments_32_miss",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::ShortStaticHit => SHORT_STATIC_HIT,
            Self::ShortDynamicHit => SHORT_DYNAMIC_HIT,
            Self::ShortMiss => SHORT_MISS,
            Self::Deep17StaticHit => DEEP_17_STATIC_HIT,
            Self::Deep17DynamicHit => DEEP_17_DYNAMIC_HIT,
            Self::Deep17Miss => DEEP_17_MISS,
            Self::Deep32StaticHit => DEEP_32_STATIC_HIT,
            Self::Deep32DynamicHit => DEEP_32_DYNAMIC_HIT,
            Self::Deep32Miss => DEEP_32_MISS,
        }
    }

    const fn should_match(self) -> bool {
        !matches!(self, Self::ShortMiss | Self::Deep17Miss | Self::Deep32Miss)
    }
}

#[inline]
fn run_scenario(router: &MixedScenarioResolver, scenario: Scenario) -> bool {
    black_box(router.resolve("GET", black_box(scenario.path()))).is_ok()
}

fn assert_equivalent(router: &MixedScenarioResolver) {
    for scenario in Scenario::ALL {
        assert_eq!(
            run_scenario(router, scenario),
            scenario.should_match(),
            "mixed/{} changed hit or miss behavior",
            scenario.name()
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

fn allocation_diagnostics(router: &MixedScenarioResolver) -> [(Scenario, AllocationStats); 9] {
    Scenario::ALL.map(|scenario| {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let operation = session.operation("resolve");
        {
            let _span = operation.measure_thread().iterations(1);
            black_box(run_scenario(router, scenario));
        }
        let report = session.to_report();
        let (_, operation) = report
            .operations()
            .find(|(name, _)| *name == "resolve")
            .expect("the resolve allocation operation is recorded");
        (
            scenario,
            AllocationStats {
                allocations: operation.total_allocations_count(),
                bytes: operation.total_bytes_allocated(),
            },
        )
    })
}
