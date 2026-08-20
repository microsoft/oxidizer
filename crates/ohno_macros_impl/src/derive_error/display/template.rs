// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Splitting a `#[display(...)]` template into literal and placeholder segments.
//!
//! A template is parsed before it is judged, so "what the template says" stays apart from "whether
//! that names a field" and neither decision is made halfway through a scan.

/// One piece of a template.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Segment<'a> {
    /// Text copied to the output template verbatim, `{{` and `}}` escapes included.
    Literal(&'a str),
    /// A `{...}` placeholder.
    Placeholder(Placeholder<'a>),
}

/// A `{...}` placeholder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Placeholder<'a> {
    /// The field named by the placeholder. `None` for `{}`, which consumes the next argument.
    pub name: Option<&'a str>,
    /// The format spec after the `:`, without the colon.
    pub spec: Option<&'a str>,
}

impl Placeholder<'_> {
    /// The placeholder as it is written in the lowered template, with the name dropped.
    ///
    /// The spec is carried through as written and never inspected, so one that refers to another
    /// argument is reported by `rustc` against the derive rather than by the macro against the
    /// template. See the limits section of `docs/design.md`.
    #[must_use]
    pub(crate) fn lowered(&self) -> String {
        self.spec.map_or_else(|| "{}".to_owned(), |spec| format!("{{:{spec}}}"))
    }
}

/// Why a template could not be split.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// A `{` that no `}` closes.
    UnclosedPlaceholder,
    /// A `}` that no `{` opens.
    StrayClosingBrace,
}

impl Fault {
    /// The diagnostic this fault renders as.
    #[must_use]
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::UnclosedPlaceholder => {
                "`#[display(...)]` template has a `{` with no matching `}`. \
                 Close the placeholder, or write `{{` for a literal brace"
            }
            Self::StrayClosingBrace => {
                "`#[display(...)]` template has a `}` with no matching `{`. \
                 Open the placeholder, or write `}}` for a literal brace"
            }
        }
    }
}

/// Splits `template` into segments.
///
/// An unbalanced brace is reported rather than repaired. Letting an unterminated `{` run to the end
/// would honor `"{path"` as `"{path}"`, so a typo would render as a working message and never
/// surface. A stray `}` would otherwise be copied into the generated `format!` string, where
/// `rustc` reports it against code the user cannot see.
///
/// The scan is driven by an iterator rather than by an index it increments. An index the scan
/// computes can be wrong, but an iterator always moves forward, so a wrong computation shows up as
/// a wrong segment a test can assert on rather than as a scan that never ends.
pub(crate) fn split(template: &str) -> Result<Vec<Segment<'_>>, Fault> {
    let bytes = template.as_bytes();
    let mut segments = Vec::new();
    let mut literal_start = 0;
    let mut cursor = bytes.iter().enumerate();

    while let Some((index, &byte)) = cursor.next() {
        match byte {
            // Both braces of an escape are consumed together, so the second one cannot read as
            // opening or closing a placeholder.
            b'{' | b'}' if bytes.get(index + 1) == Some(&byte) => _ = cursor.next(),
            b'}' => return Err(Fault::StrayClosingBrace),
            b'{' => {
                let length = bytes[index + 1..]
                    .iter()
                    .position(|&b| b == b'}')
                    .ok_or(Fault::UnclosedPlaceholder)?;
                let close = index + 1 + length;

                if literal_start < index {
                    segments.push(Segment::Literal(&template[literal_start..index]));
                }
                segments.push(Segment::Placeholder(placeholder(&template[index + 1..close])));

                literal_start = close + 1;
                // `nth` consumes one more than it skips, which lands the scan on the byte after
                // the closing brace.
                _ = cursor.nth(length);
            }
            _ => {}
        }
    }

    if literal_start < bytes.len() {
        segments.push(Segment::Literal(&template[literal_start..]));
    }

    Ok(segments)
}

/// Splits a placeholder's contents into its name and its format spec.
fn placeholder(contents: &str) -> Placeholder<'_> {
    let (name, spec) = match contents.find(':') {
        Some(colon) => (&contents[..colon], Some(&contents[colon + 1..])),
        None => (contents, None),
    };

    Placeholder {
        name: (!name.is_empty()).then_some(name),
        spec,
    }
}
