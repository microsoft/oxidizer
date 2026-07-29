// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::{String, ToString};

use http_path_template::{Grammar, PathTemplate};

/// Whether `value` is a non-empty RFC 9110 `token`.
#[doc(hidden)]
#[must_use]
pub fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
}

/// A single route definition to generate a resolver from.
///
/// Each distinct `name` becomes an enum variant and must be a valid Rust
/// identifier. Routes sharing a name must capture the same variables.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use routerama_build::Route;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let route = Route::new(
///     "GetBook",
///     "GET",
///     PathTemplate::parse("/v1/books/{book}", Grammar::default())?,
/// );
/// assert_eq!(route.name(), "GetBook");
/// assert_eq!(route.method(), "GET");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Route {
    name: String,
    method: String,
    pattern: String,
}

impl Route {
    /// Creates a route binding `name` to `method` + `template`.
    ///
    /// `name` becomes an enum variant; `method` is matched exactly.
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the template is rendered to its stored text; by-value keeps the builder call ergonomic"
    )]
    pub fn new(name: impl Into<String>, method: impl Into<String>, template: PathTemplate<'_>) -> Self {
        Self {
            name: name.into(),
            method: method.into(),
            pattern: template.to_string(),
        }
    }

    /// The generated enum variant name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The HTTP method token this route matches (e.g. `"GET"`).
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The parsed path template this route matches.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn template(&self) -> PathTemplate<'_> {
        // The pattern was validated when this `Route` was created, and the
        // affix-enabled grammar is a strict superset, so re-parsing cannot fail.
        PathTemplate::parse(&self.pattern, Grammar::default().with_segment_affixes())
            .expect("pattern was validated when the Route was created")
    }
}
