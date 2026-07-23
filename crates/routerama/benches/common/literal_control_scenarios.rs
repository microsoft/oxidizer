// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Literal-only generated-router controls. Each size has independent generated
// source so compile time and section size can be measured separately. Runtime
// rows compare topology without response construction.

mod routes_16 {
    include!("../generated/literal_controls_16.rs");
}

mod routes_128 {
    include!("../generated/literal_controls_128.rs");
}

mod routes_1024 {
    include!("../generated/literal_controls_1024.rs");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteSetSize {
    Routes16,
    Routes128,
    Routes1024,
}

impl RouteSetSize {
    const ALL: [Self; 3] = [Self::Routes16, Self::Routes128, Self::Routes1024];

    const fn name(self) -> &'static str {
        match self {
            Self::Routes16 => "routes_16",
            Self::Routes128 => "routes_128",
            Self::Routes1024 => "routes_1024",
        }
    }

    const fn count(self) -> usize {
        match self {
            Self::Routes16 => routes_16::ROUTE_COUNT,
            Self::Routes128 => routes_128::ROUTE_COUNT,
            Self::Routes1024 => routes_1024::ROUTE_COUNT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    WideFanout,
    DeepChain,
    TerminalSuffix,
}

impl Shape {
    const ALL: [Self; 3] = [Self::WideFanout, Self::DeepChain, Self::TerminalSuffix];

    const fn name(self) -> &'static str {
        match self {
            Self::WideFanout => "wide_fanout",
            Self::DeepChain => "deep_chain",
            Self::TerminalSuffix => "terminal_suffix_shared_prefix",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::WideFanout => 0,
            Self::DeepChain => 1,
            Self::TerminalSuffix => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    First,
    Middle,
    Last,
    Miss,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::First, Self::Middle, Self::Last, Self::Miss];

    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Middle => "middle",
            Self::Last => "last",
            Self::Miss => "miss",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Middle => 1,
            Self::Last => 2,
            Self::Miss => 3,
        }
    }

    const fn expected(self, size: RouteSetSize) -> Option<usize> {
        match self {
            Self::First => Some(0),
            Self::Middle => Some(size.count() / 2),
            Self::Last => Some(size.count() - 1),
            Self::Miss => None,
        }
    }
}

enum PreparedRouters {
    Routes16(routes_16::Routers),
    Routes128(routes_128::Routers),
    Routes1024(routes_1024::Routers),
}

fn prepare(size: RouteSetSize) -> PreparedRouters {
    match size {
        RouteSetSize::Routes16 => PreparedRouters::Routes16(routes_16::Routers::new()),
        RouteSetSize::Routes128 => PreparedRouters::Routes128(routes_128::Routers::new()),
        RouteSetSize::Routes1024 => PreparedRouters::Routes1024(routes_1024::Routers::new()),
    }
}

fn run_prepared(routers: &PreparedRouters, shape: Shape, scenario: Scenario) -> Option<usize> {
    match routers {
        PreparedRouters::Routes16(routers) => routers.run(shape.index(), scenario.index()),
        PreparedRouters::Routes128(routers) => routers.run(shape.index(), scenario.index()),
        PreparedRouters::Routes1024(routers) => routers.run(shape.index(), scenario.index()),
    }
}

fn assert_equivalent() {
    for size in RouteSetSize::ALL {
        let routers = prepare(size);
        for shape in Shape::ALL {
            for scenario in Scenario::ALL {
                assert_eq!(
                    run_prepared(&routers, shape, scenario),
                    scenario.expected(size),
                    "{}/{}/{} selected a different literal route",
                    size.name(),
                    shape.name(),
                    scenario.name()
                );
            }
        }
    }
}
