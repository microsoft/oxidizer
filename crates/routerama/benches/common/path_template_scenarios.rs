// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared template-shape scenarios measuring `PathTemplate::parse` directly.
// Template text construction is excluded from the measured region; only the
// parse call itself is timed.

use std::fmt::Write as _;

use http_path_template::{Grammar, PathTemplate, Segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    /// A pure-literal template with no captures.
    Literal,
    /// A template with several path captures.
    Captures,
    /// A template with an intra-segment affix (partly literal, partly
    /// capture) and a trailing verb.
    Affix,
    /// A long template alternating literal and capture segments.
    Deep,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::Literal, Self::Captures, Self::Affix, Self::Deep];

    const fn name(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Captures => "captures",
            Self::Affix => "affix",
            Self::Deep => "deep",
        }
    }

    /// Segment/capture pair count for the `Deep` scenario.
    const DEEP_PAIRS: usize = 24;
    fn template_text(self) -> String {
        match self {
            Self::Literal => "/v1/repos/rust-lang/cargo/releases".to_owned(),
            Self::Captures => "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}".to_owned(),
            Self::Affix => "/v1/repos/{owner}/{repo}/releases/{tag}.tar.gz".to_owned(),
            Self::Deep => {
                let mut template = String::new();
                for index in 0..Self::DEEP_PAIRS {
                    let _ = write!(template, "/segment{index:02}/{{v{index:02}}}");
                }
                template.push_str(":watch");
                template
            }
        }
    }

    const fn expected(self) -> Summary {
        match self {
            Self::Literal => Summary {
                segments: 5,
                verb: false,
                literals: 5,
                variables: 0,
                affixes: 0,
                rest: false,
            },
            Self::Captures => Summary {
                segments: 8,
                verb: false,
                literals: 4,
                variables: 4,
                affixes: 0,
                rest: false,
            },
            Self::Affix => Summary {
                segments: 6,
                verb: false,
                literals: 3,
                variables: 2,
                affixes: 1,
                rest: false,
            },
            Self::Deep => Summary {
                segments: Self::DEEP_PAIRS * 2,
                verb: true,
                literals: Self::DEEP_PAIRS,
                variables: Self::DEEP_PAIRS,
                affixes: 0,
                rest: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Summary {
    segments: usize,
    verb: bool,
    literals: usize,
    variables: usize,
    affixes: usize,
    rest: bool,
}

impl Summary {
    fn of(template: &PathTemplate<'_>) -> Self {
        let mut summary = Self {
            segments: template.segments().len(),
            verb: template.verb().is_some(),
            literals: 0,
            variables: 0,
            affixes: 0,
            rest: false,
        };
        for segment in template.segments() {
            match segment {
                Segment::Literal(_) => summary.literals += 1,
                Segment::Variable(_) => summary.variables += 1,
                Segment::Affix { .. } => summary.affixes += 1,
                Segment::Rest => summary.rest = true,
                _ => {}
            }
        }
        summary
    }
}

struct PreparedScenario {
    text: String,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    PreparedScenario {
        text: scenario.template_text(),
    }
}

fn run_parse_prepared(prepared: &PreparedScenario) -> Summary {
    let template = PathTemplate::parse(&prepared.text, Grammar::default().with_segment_affixes()).expect("benchmark template is valid");
    Summary::of(&template)
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        let observed = run_parse_prepared(&prepare(scenario));
        assert_eq!(observed, scenario.expected(), "PathTemplate::parse/{} changed its parsed shape", scenario.name());
    }
}
