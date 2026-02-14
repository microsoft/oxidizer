// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use chumsky::prelude::*;

use crate::error::ParseError;

pub type Error<'a> = extra::Err<Rich<'a, char>>;

#[derive(Debug, Clone, PartialEq)]
pub struct UriTemplate<'a> {
    template_parts: Vec<TemplatePart<'a>>,
}

impl<'a> UriTemplate<'a> {
    /// The main parser body for the URI template.
    fn parser() -> impl Parser<'a, &'a str, UriTemplate<'a>, Error<'a>> {
        let template_parts = TemplatePart::parser()
            .repeated()
            .collect::<Vec<TemplatePart<'a>>>()
            .validate(|parts, e, em| {
                // make sure the template starts with a slash
                if let Some(TemplatePart::Content(first)) = parts.first()
                    && !first.starts_with('/')
                {
                    em.emit(Rich::custom(e.span(), "template has to start with '/'"));
                }
                parts
            });
        template_parts.map(|template_parts| UriTemplate { template_parts })
    }

    pub(crate) fn parse(input: &'a str) -> Result<Self, ParseError<'a>> {
        let parsed = Self::parser().parse(input);
        parsed.into_result().map_err(ParseError::new)
    }

    pub(crate) fn format_template(&self) -> String {
        self.template_parts
            .iter()
            .map(|part| match part {
                TemplatePart::ParamGroup(group) => group.raw_template(),
                TemplatePart::Content(content) => content.clone(),
            })
            .collect()
    }

    pub(crate) fn params(&self) -> impl Iterator<Item = Param<'a>> {
        self.template_parts
            .iter()
            .filter_map(|part| {
                let TemplatePart::ParamGroup(group) = part else {
                    return None;
                };
                Some(group.params())
            })
            .flatten()
    }

    pub(crate) fn param_names(&self) -> HashSet<String> {
        self.params().map(|param| param.name.to_string()).collect()
    }

    pub(crate) fn template_parts(&self) -> &[TemplatePart<'a>] {
        &self.template_parts
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TemplatePart<'a> {
    ParamGroup(ParamGroup<'a>),
    Content(String),
}

impl<'a> TemplatePart<'a> {
    /// Parse one part of the template, which can either be a parameter group or a content string.
    fn parser() -> impl Parser<'a, &'a str, TemplatePart<'a>, Error<'a>> {
        ParamGroup::parser()
            .map(TemplatePart::ParamGroup)
            .or(none_of(['{', '}'])
                .repeated()
                .at_least(1)
                .collect() // collect all characters that aren't part of a parameter group
                .map(|s: String| TemplatePart::Content(s)))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParamGroup<'a> {
    param_kind: ParamKind,
    param_names: Vec<&'a str>,
}

impl<'a> ParamGroup<'a> {
    fn parser() -> impl Parser<'a, &'a str, ParamGroup<'a>, Error<'a>> {
        let param_name = text::ascii::ident().labelled("parameter name");
        let params = ParamKind::parser().then(
            param_name
                .separated_by(just(','))
                .at_least(1)
                .collect::<Vec<&str>>()
                .labelled("comma separated parameters"),
        );
        params
            .delimited_by(just('{'), just('}'))
            .map(|(param_kind, param_names)| ParamGroup {
                param_kind,
                param_names,
            })
    }

    /// Checks if the parameter group allows restricted parameters.
    pub(crate) fn allows_restricted(&self) -> bool {
        matches!(self.param_kind, ParamKind::Unfiltered | ParamKind::Fragment)
    }

    /// Returns an iterator over the parameters in this group.
    pub(crate) fn params(&self) -> impl Iterator<Item = Param<'a>> {
        self.param_names.iter().map(|&name| Param {
            name,
            allows_restricted: self.allows_restricted(),
        })
    }

    /// Returns the parameter names in this group.
    pub(crate) fn param_names(&self) -> &[&'a str] {
        &self.param_names
    }

    /// Returns the raw template string for this parameter group.
    fn raw_template(&self) -> String {
        let params: Vec<String> = self
            .param_names
            .iter()
            .map(|param_name| {
                if self.param_kind.is_kv() {
                    format!("{param_name}={{{param_name}}}")
                } else {
                    format!("{{{param_name}}}")
                }
            })
            .collect();
        let values = params.join(self.param_kind.separator());
        format!("{}{values}", self.param_kind.prefix().unwrap_or_default())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Param<'a> {
    pub(crate) name: &'a str,
    pub(crate) allows_restricted: bool,
}

/// Represents a prefix for a prefixed parameter kind

#[derive(Debug, Clone, Eq, PartialEq)]
enum Prefix {
    Dot,
    Slash,
}

impl Prefix {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Dot => ".",
            Self::Slash => "/",
        }
    }
}

/// The kind of parameter group, which determines how the parameters are formatted in the URI template.
#[derive(Debug, Clone, Eq, PartialEq)]
enum ParamKind {
    Simple,
    Unfiltered,
    Fragment,
    SemicolonKV,
    /// Form parameters, which can either start with '?' (`start_char = true`) or continue with '&'
    Form {
        start_char: bool,
    },
    Prefixed(Prefix),
}

impl ParamKind {
    fn parser<'a>() -> impl Parser<'a, &'a str, Self, Error<'a>> {
        let unfiltered = just("+").to(Self::Unfiltered);
        let fragment = just("#").to(Self::Fragment);
        let semicolon_kv = just(";").to(Self::SemicolonKV);
        let form_start = just("?").to(Self::Form { start_char: true });
        let form_continue = just("&").to(Self::Form { start_char: false });
        let prefixed_dot = just(".").to(Self::Prefixed(Prefix::Dot));
        let prefixed_slash = just("/").to(Self::Prefixed(Prefix::Slash));
        let simple = empty().to(Self::Simple).labelled("no prefix");
        choice((
            unfiltered,
            fragment,
            semicolon_kv,
            form_start,
            form_continue,
            prefixed_dot,
            prefixed_slash,
            simple,
        ))
    }
}

impl ParamKind {
    /// Returns the separator used between values of this kind of parameter when filling the template.
    fn separator(&self) -> &'static str {
        match self {
            Self::Simple | Self::Unfiltered | Self::Fragment => ",",
            Self::SemicolonKV => ";",
            Self::Form { .. } => "&",
            Self::Prefixed(prefix) => prefix.as_str(),
        }
    }

    /// Returns the prefix used for this kind of parameter when filling the template.
    fn prefix(&self) -> Option<&'static str> {
        match self {
            Self::Simple | Self::Unfiltered => None,
            Self::Fragment => Some("#"),
            Self::SemicolonKV => Some(";"),
            Self::Form { start_char } => {
                if *start_char {
                    Some("?")
                } else {
                    Some("&")
                }
            }
            Self::Prefixed(prefix) => Some(prefix.as_str()),
        }
    }

    /// Checks if this parameter kind is a key-value pair (e.g., semicolon or form parameters).
    fn is_kv(&self) -> bool {
        matches!(self, Self::SemicolonKV | Self::Form { .. })
    }
}

#[cfg(test)]
mod test {
    use ohno::ErrorExt;

    use super::*;
    #[test]
    fn test_param_group_parser() {
        let input = "{+param1,param2}";
        let parsed = ParamGroup::parser().parse(input).unwrap();
        assert_eq!(
            parsed.param_names,
            vec!["param1".to_string(), "param2".to_string()]
        );
        assert_eq!(parsed.param_kind, ParamKind::Unfiltered);
        assert_eq!(parsed.raw_template(), "{param1},{param2}");

        let input = "{param1,param2}";
        let parsed = ParamGroup::parser().parse(input).unwrap();
        assert_eq!(
            parsed.param_names,
            vec!["param1".to_string(), "param2".to_string()]
        );
        assert_eq!(parsed.param_kind, ParamKind::Simple);
        assert_eq!(parsed.raw_template(), "{param1},{param2}");

        let parsed = ParamGroup::parser().parse("{/param1,param2}").unwrap();
        assert_eq!(parsed.raw_template(), "/{param1}/{param2}");

        let parsed = ParamGroup::parser().parse("{;param1,param2}").unwrap();
        assert_eq!(parsed.raw_template(), ";param1={param1};param2={param2}");

        let parsed = ParamGroup::parser().parse("{?param1,param2}").unwrap();
        assert_eq!(parsed.raw_template(), "?param1={param1}&param2={param2}");

        let parsed = ParamGroup::parser().parse("{&param1,param2}").unwrap();
        assert_eq!(parsed.raw_template(), "&param1={param1}&param2={param2}");

        let input = "{&}";
        let parsed = ParamGroup::parser().parse(input);
        let errors: Vec<_> = parsed.errors().collect();
        assert_eq!(
            errors[0].reason().to_string(),
            "found '}' expected comma separated parameters"
        );

        let input = "{@param1,param2}";
        let parsed = ParamGroup::parser().parse(input);
        let errors: Vec<_> = parsed.errors().collect();
        assert_eq!(errors[0].found(), Some(&'@'));
    }

    #[test]
    fn test_parser() {
        let input = "/{first}/{+param1,param2}/{;param3,param4}/{?query2,query3}/{.dot1,dot2}{/slash1,slash2}{#fragment}";
        let result = UriTemplate::parse(input).unwrap();
        let params = result
            .template_parts
            .iter()
            .filter(|part| matches!(part, TemplatePart::ParamGroup(_)))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            result.format_template(),
            "/{first}/{param1},{param2}/;param3={param3};param4={param4}/?query2={query2}&query3={query3}/.{dot1}.{dot2}/{slash1}/{slash2}#{fragment}"
        );
        assert_eq!(
            params,
            vec![
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Simple,
                    param_names: vec!["first"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Unfiltered,
                    param_names: vec!["param1", "param2"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::SemicolonKV,
                    param_names: vec!["param3", "param4"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Form { start_char: true },
                    param_names: vec!["query2", "query3"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Prefixed(Prefix::Dot),
                    param_names: vec!["dot1", "dot2"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Prefixed(Prefix::Slash),
                    param_names: vec!["slash1", "slash2"]
                }),
                TemplatePart::ParamGroup(ParamGroup {
                    param_kind: ParamKind::Fragment,
                    param_names: vec!["fragment"]
                })
            ]
        );
    }

    #[test]
    fn test_parser_err() {
        let input = "{first}/{+param1,param2}/{;param3,param4}/{^query2,query3}/{.dot1,dot2}{/slash1,slash2}{#fragment}";
        let result = UriTemplate::parse(input);
        assert_eq!(
            result.unwrap_err().message(),
            "Failed to parse URI: [found ''^'' at 43..44 expected ''+'', ''#'', '';'', ''?'', ''&'', ''.'', ''/'', or comma separated parameters]"
        );
    }
}
