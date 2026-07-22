// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use data_privacy::RedactedDisplay;
use http::uri::PathAndQuery;

use crate::UriError;

/// Allows for the creation of URIs based on templates.
///
/// A `PathAndQueryTemplate` describes both the path and the optional query string
/// portion of a URI (everything after the authority and before any fragment).
/// Variables may appear in either part (e.g. `/users/{id}?filter={kind}`).
///
/// Use the `#[templated]` attribute macro to derive an implementation.
///
/// Templates are based on [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570) Level 3,
/// with additional constraints for valid HTTP URI construction:
///
/// - Variable names must be valid Rust identifiers (ASCII letters, digits, underscores)
/// - Templates must start with a leading `/`
/// - Fragment expansion (`{#var}`) is not supported (fragments are ignored by HTTP clients)
///
/// All template values must implement [`Escape`](crate::Escape), except for
/// unfiltered expansions (`{+foo}`). This ensures variables cannot contain reserved characters
/// as defined by the RFC.
///
/// # Examples
///
/// ```
/// use templated_uri::{EscapedString, PathAndQueryTemplate, templated};
///
/// #[templated(template = "/{org_id}/user/{user_id}/", unredacted)]
/// #[derive(Clone)]
/// struct UserPath {
///     org_id: EscapedString,
///     user_id: EscapedString,
/// }
///
/// let user_path = UserPath {
///     org_id: EscapedString::from_static("acme"),
///     user_id: EscapedString::from_static("john_doe"),
/// };
///
/// assert_eq!(user_path.render(), "/acme/user/john_doe/");
/// ```
///
/// # Classified fields
///
/// The `classified` attribute enables data classification via `data_privacy` types.
///
/// ```
/// #![allow(non_upper_case_globals)]
/// # const Pii: DataClass = DataClass::new("templated_uri", "pii");
/// use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
/// use data_privacy::{
///     Classified, DataClass, RedactedToString, RedactionEngine, RedactionEngineBuilder, Sensitive,
/// };
/// use templated_uri::{EscapedString, PathAndQueryTemplate, templated};
///
/// #[templated(template = "/{org_id}/user/{user_id}/")]
/// #[derive(Clone)]
/// struct UserPath {
///     #[unredacted]
///     org_id: EscapedString,
///     user_id: Sensitive<EscapedString>,
/// }
///
/// let user_path = UserPath {
///     org_id: EscapedString::from_static("acme"),
///     user_id: Sensitive::new(EscapedString::from_static("john_doe"), Pii),
/// };
/// assert_eq!(user_path.render(), "/acme/user/john_doe/");
///
/// let asterisk_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*'));
/// let redaction_engine = RedactionEngine::builder()
///     .set_fallback_redactor(asterisk_redactor)
///     .build();
///
/// assert_eq!(
///     user_path.to_redacted_string(&redaction_engine),
///     "/acme/user/********/"
/// )
/// ```
pub trait PathAndQueryTemplate: RedactedDisplay + Debug + Sync + Send
where
    Self: 'static,
{
    /// Renders the template with its current field values into a path-and-query string.
    ///
    /// For a validated [`PathAndQuery`] use [`PathAndQueryTemplate::to_path_and_query`] instead.
    fn render(&self) -> String;

    /// Appends the rendered path-and-query to an existing buffer.
    ///
    /// This is the allocation-free primitive underlying [`render`](PathAndQueryTemplate::render):
    /// it lets a caller (such as base-URI joining) render directly into a buffer that already
    /// holds a prefix, avoiding an intermediate `String` and a second allocation on the
    /// request hot path. The `#[templated]` macro overrides it to append field values
    /// directly; the default simply appends the result of [`render`](PathAndQueryTemplate::render).
    ///
    /// A manual implementer only needs to override [`render`](PathAndQueryTemplate::render);
    /// this default then works automatically. Do **not**, however, implement `render` *in terms
    /// of* `render_into` (e.g. `let mut s = String::new(); self.render_into(&mut s); s`) without
    /// also overriding `render_into`, or the two defaults will call each other and loop forever.
    #[doc(hidden)]
    fn render_into(&self, buf: &mut String) {
        buf.push_str(&self.render());
    }

    /// Returns a heuristic byte-capacity estimate for the rendered path-and-query.
    ///
    /// Used to size a buffer before calling [`render_into`](PathAndQueryTemplate::render_into)
    /// so the render completes without reallocating. The `#[templated]` macro overrides it
    /// with a compile-time estimate; the default is `0` (callers fall back to growth).
    #[doc(hidden)]
    fn render_capacity_hint(&self) -> usize {
        0
    }

    /// Converts to a validated [`PathAndQuery`].
    ///
    /// # Errors
    ///
    /// Returns a [`UriError`] if the rendered URI is not a valid path-and-query.
    fn to_path_and_query(&self) -> Result<PathAndQuery, UriError>;

    /// Returns the original RFC 6570 template string.
    ///
    /// This is the string the user wrote in `#[templated(template = "...")]`,
    /// containing variables in the form `{var}`, `{+var}`, `{/var}`, `{?var}`, etc.
    fn template(&self) -> &'static str;

    /// Returns the template in Rust format-string syntax.
    ///
    /// The original RFC 6570 expansions are flattened into bare `{var}` placeholders
    /// suitable for use with [`std::format!`] and friends. Used internally during expansion.
    fn format_template(&self) -> &'static str;

    /// Returns the optional label for this template.
    ///
    /// Set via `#[templated(template = "...", label = "my_label")]`.
    /// Useful for telemetry when the full template is too verbose.
    fn label(&self) -> Option<&'static str>;
}

impl<T: PathAndQueryTemplate> From<T> for crate::Uri {
    fn from(value: T) -> Self {
        Self::from(crate::PathAndQuery::from_template(value))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Formatter;

    use data_privacy::Redactor;

    use super::*;

    /// A hand-written [`PathAndQueryTemplate`] that deliberately does *not* override
    /// `render_into` / `render_capacity_hint`, so calling them exercises the trait's
    /// default implementations (the `#[templated]` macro always overrides them).
    #[derive(Debug)]
    struct ManualTemplate;

    impl RedactedDisplay for ManualTemplate {
        fn fmt(&self, _redactor: &dyn Redactor, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.write_str("/manual/path")
        }
    }

    impl PathAndQueryTemplate for ManualTemplate {
        fn render(&self) -> String {
            String::from("/manual/path")
        }

        fn to_path_and_query(&self) -> Result<PathAndQuery, UriError> {
            Ok(PathAndQuery::try_from("/manual/path")?)
        }

        fn template(&self) -> &'static str {
            "/manual/path"
        }

        fn format_template(&self) -> &'static str {
            "/manual/path"
        }

        fn label(&self) -> Option<&'static str> {
            None
        }
    }

    #[test]
    fn default_render_into_appends_render_output() {
        // The default `render_into` appends the result of `render()` to the buffer.
        let template = ManualTemplate;
        let mut buf = String::from("/prefix");
        template.render_into(&mut buf);
        assert_eq!(buf, "/prefix/manual/path");
    }

    #[test]
    fn default_render_capacity_hint_is_zero() {
        // The default `render_capacity_hint` returns `0` (callers fall back to growth).
        let template = ManualTemplate;
        assert_eq!(template.render_capacity_hint(), 0);
    }

    #[test]
    fn manual_template_required_methods() {
        // Exercise the remaining (non-defaulted) trait methods of the hand-written impl so
        // the helper is fully covered and the default-method tests above stay meaningful.
        use data_privacy::{RedactedToString, RedactionEngine};

        let template = ManualTemplate;
        assert_eq!(template.render(), "/manual/path");
        assert_eq!(template.template(), "/manual/path");
        assert_eq!(template.format_template(), "/manual/path");
        assert_eq!(template.label(), None);
        assert_eq!(template.to_path_and_query().expect("valid path").as_str(), "/manual/path");
        assert_eq!(format!("{template:?}"), "ManualTemplate");

        let engine = RedactionEngine::builder().build();
        assert_eq!(template.to_redacted_string(&engine), "/manual/path");
    }
}
