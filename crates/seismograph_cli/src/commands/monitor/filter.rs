// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-record selection using the symbol paths of complete captured stacks.
//!
//! A symbol's owning crate/module is not its execution lineage: generic type
//! arguments and implemented traits do not make a frame belong to those crates.

use std::fmt;

/// Which captured stack identifies runtime activity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RuntimeStackMode {
    #[default]
    Event,
    Spawn,
}

/// A definite selection decision, or a decision requiring unavailable frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Match {
    Included,
    Excluded,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Rule {
    Crate(String),
    Module(String),
    Function(String),
}

impl Rule {
    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Crate(name) => path.split("::").next() == Some(name.as_str()),
            Self::Module(name) => path.strip_prefix(name).is_some_and(|suffix| suffix.starts_with("::")),
            Self::Function(name) => path == name,
        }
    }
}

/// Parsed include/exclude rules, retaining their original text for editing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct FilterSpec {
    includes: Vec<Rule>,
    excludes: Vec<Rule>,
    include_text: String,
    exclude_text: String,
    show_unknown: bool,
    pub(super) runtime_stack: RuntimeStackMode,
}

impl FilterSpec {
    pub(super) fn parse(
        includes: &str,
        excludes: &str,
        show_unknown: bool,
        runtime_stack: RuntimeStackMode,
    ) -> Result<Self, FilterParseError> {
        Ok(Self {
            includes: parse_rules(includes)?,
            excludes: parse_rules(excludes)?,
            include_text: includes.to_owned(),
            exclude_text: excludes.to_owned(),
            show_unknown,
            runtime_stack,
        })
    }

    pub(super) fn includes(&self) -> &str {
        &self.include_text
    }

    pub(super) fn excludes(&self) -> &str {
        &self.exclude_text
    }

    pub(super) const fn show_unknown(&self) -> bool {
        self.show_unknown
    }

    pub(super) fn is_active(&self) -> bool {
        !self.includes.is_empty() || !self.excludes.is_empty()
    }

    pub(super) fn classify(&self, provenance: &StackProvenance) -> Match {
        if !self.is_active() {
            return Match::Included;
        }
        let matches = |rules: &[Rule]| rules.iter().any(|rule| provenance.paths.iter().any(|path| rule.matches(path)));
        if matches(&self.excludes) {
            return Match::Excluded;
        }
        let included = self.includes.is_empty() || matches(&self.includes);
        if provenance.incomplete && (!included || !self.excludes.is_empty()) {
            return Match::Unknown;
        }
        if included { Match::Included } else { Match::Excluded }
    }

    pub(super) fn accepts(&self, classification: Match) -> bool {
        !self.is_active() || classification == Match::Included || (classification == Match::Unknown && self.show_unknown)
    }
}

/// Cached normalized symbol owners plus whether any captured frame is unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StackProvenance {
    paths: Vec<String>,
    incomplete: bool,
}

impl StackProvenance {
    pub(super) fn from_symbols<'a>(symbols: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        let mut paths = Vec::new();
        let mut incomplete = false;
        for symbol in symbols {
            if let Some(path) = symbol.and_then(symbol_path) {
                paths.push(path);
            } else {
                incomplete = true;
            }
        }
        incomplete |= paths.is_empty();
        Self { paths, incomplete }
    }
}

/// Invalid rule text; the draft can be corrected without changing the active view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FilterParseError {
    rule: String,
    reason: &'static str,
}

impl fmt::Display for FilterParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Invalid filter rule {:?}: {}", self.rule, self.reason)
    }
}

impl std::error::Error for FilterParseError {}

fn parse_rules(text: &str) -> Result<Vec<Rule>, FilterParseError> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty())
        .map(|token| {
            let invalid = |reason| FilterParseError {
                rule: token.to_owned(),
                reason,
            };
            let (kind, target) = token.split_once(':').ok_or_else(|| invalid("use crate:, module:, or function:"))?;
            let segments: Vec<_> = target.split("::").collect();
            if !segments.iter().all(|segment| identifier(segment)) {
                return Err(invalid("target must contain nonempty Rust identifiers separated by ::"));
            }
            match kind {
                "crate" if segments.len() == 1 => Ok(Rule::Crate(target.to_owned())),
                "module" if segments.len() >= 2 => Ok(Rule::Module(target.to_owned())),
                "function" if segments.len() >= 2 => Ok(Rule::Function(target.to_owned())),
                "crate" => Err(invalid("crate target must be a single crate name")),
                "module" | "function" => Err(invalid("target must include its crate, for example my_crate::name")),
                _ => Err(invalid("use crate:, module:, or function:")),
            }
        })
        .collect()
}

fn identifier(segment: &str) -> bool {
    let segment = segment.strip_prefix("r#").unwrap_or(segment);
    let mut characters = segment.chars();
    characters.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && characters.all(|c| c == '_' || c.is_alphanumeric())
}

/// Removes generic arguments, retaining the implementing owner of qualified impls.
fn symbol_path(symbol: &str) -> Option<String> {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return None;
    }
    let mut path = if symbol.starts_with('<') {
        let end = angle_end(symbol, 0)?;
        let qualified = &symbol[1..end];
        let owner_end = qualified_owner_end(qualified);
        let owner = symbol_path(&qualified[..owner_end])?;
        let suffix = &symbol[end + 1..];
        if !suffix.starts_with("::") {
            return None;
        }
        format!("{owner}{}", strip_generics(suffix)?)
    } else {
        strip_generics(symbol)?
    };
    if let Some((owner, hash)) = path.rsplit_once("::")
        && hash.len() == 17
        && hash.starts_with('h')
        && hash[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        path.truncate(owner.len());
    }
    path.split("::")
        .all(|part| identifier(part) || part == "{{closure}}" || part == "{{async_block}}")
        .then_some(path)
}

fn qualified_owner_end(qualified: &str) -> usize {
    let mut position = 0;
    while position < qualified.len() {
        if qualified[position..].starts_with(" as ") {
            return position;
        }
        if qualified.as_bytes()[position] == b'<'
            && let Some(end) = angle_end(qualified, position)
        {
            position = end + 1;
        } else {
            // Only ASCII syntax is inspected; byte offsets need not be char boundaries.
            position += 1;
            while !qualified.is_char_boundary(position) {
                position += 1;
            }
        }
    }
    qualified.len()
}

fn angle_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, byte) in text.as_bytes()[start..].iter().enumerate() {
        let position = start + offset;
        match byte {
            b'<' => depth += 1,
            b'>' if position == 0 || text.as_bytes()[position - 1] != b'-' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(position);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_generics(text: &str) -> Option<String> {
    let mut path = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        path.push_str(&rest[..start]);
        if path.ends_with("::") {
            path.truncate(path.len() - 2);
        }
        let end = angle_end(rest, start)?;
        rest = &rest[end + 1..];
    }
    path.push_str(rest);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::{FilterSpec, Match, RuntimeStackMode, StackProvenance, symbol_path};

    fn classify(includes: &str, excludes: &str, symbols: &[Option<&str>]) -> Match {
        FilterSpec::parse(includes, excludes, false, RuntimeStackMode::Event)
            .unwrap()
            .classify(&StackProvenance::from_symbols(symbols.iter().copied()))
    }

    #[test]
    fn normalizes_generic_qualified_and_hashed_paths() {
        assert_eq!(
            [
                symbol_path("foo::run::<other::Arg>::h0123456789abcdef"),
                symbol_path("<foo::Type<arg::Type> as other::Trait>::method"),
                symbol_path("<<foo::Type as other::Trait>::Assoc as third::Trait>::method"),
                symbol_path("foo::run<fn() -> other::Arg>"),
            ],
            [
                Some("foo::run".into()),
                Some("foo::Type::method".into()),
                Some("foo::Type::Assoc::method".into()),
                Some("foo::run".into()),
            ]
        );
    }

    #[test]
    fn type_arguments_and_trait_names_are_not_owners() {
        assert_eq!(
            [
                classify("crate:app", "", &[Some("core::ptr::drop_in_place<app::Type>")]),
                classify("crate:other", "", &[Some("<app::Type as other::Trait>::method")]),
                classify("function:app::Type::method", "", &[Some("<app::Type as other::Trait>::method")]),
            ],
            [Match::Excluded, Match::Excluded, Match::Included]
        );
    }

    #[test]
    fn matching_uses_segments_and_entire_stack() {
        assert_eq!(
            [
                classify("crate:app", "", &[Some("application::work")]),
                classify("module:app::net", "", &[Some("app::network::work")]),
                classify("module:app::net", "", &[Some("app::net::work")]),
                classify("function:app::run", "", &[Some("app::runner")]),
                classify("crate:absent, function:app::run", "", &[Some("core::work"), Some("app::run")]),
            ],
            [Match::Excluded, Match::Excluded, Match::Included, Match::Excluded, Match::Included]
        );
    }

    #[test]
    fn exclusions_win_even_with_unresolved_frames() {
        assert_eq!(
            classify(
                "crate:app",
                "module:app::internal",
                &[Some("app::run"), None, Some("app::internal::run")]
            ),
            Match::Excluded
        );
    }

    #[test]
    fn partial_symbolization_only_blocks_undecidable_results() {
        assert_eq!(
            [
                classify("crate:app", "", &[Some("app::run"), None]),
                classify("crate:app", "", &[Some("core::run"), None]),
                classify("crate:app", "crate:internal", &[Some("app::run"), None]),
                classify("", "crate:internal", &[None]),
                classify("crate:app", "", &[]),
                classify("crate:app", "", &[Some("???")]),
                classify("crate:app", "crate:internal", &[Some("core::run")]),
            ],
            [
                Match::Included,
                Match::Unknown,
                Match::Unknown,
                Match::Unknown,
                Match::Unknown,
                Match::Unknown,
                Match::Excluded,
            ]
        );
    }

    #[test]
    fn empty_rules_pass_through_and_unknown_policy_is_explicit() {
        let hidden = FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap();
        let shown = FilterSpec::parse("crate:app", "", true, RuntimeStackMode::Spawn).unwrap();
        assert_eq!(
            [classify("", "", &[]), classify("", "", &[None]),],
            [Match::Included, Match::Included]
        );
        assert_eq!([hidden.accepts(Match::Unknown), shown.accepts(Match::Unknown)], [false, true]);
    }

    #[test]
    fn malformed_rules_have_actionable_errors() {
        for text in [
            "crate:",
            "crate:foo::bar",
            "module:foo",
            "function:foo",
            "crate:foo-bar",
            "crate:foo:",
            "foo",
            "other:foo",
        ] {
            let error = FilterSpec::parse(text, "", false, RuntimeStackMode::Event).unwrap_err();
            assert!(error.to_string().contains(text), "{error}");
        }
    }
}
