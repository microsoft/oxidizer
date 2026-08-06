// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Bail macro for early return with `Error::new`
///
/// Usage:
/// - `bail!("message")` - uses `Span::call_site()`
/// - `bail!(span, "message")` - uses provided span
/// - `bail!("format string {}", value)` - format with `Span::call_site()`
/// - `bail!(span, "format string {}", value)` - format with provided span
macro_rules! bail {
    // Simple message with call_site span
    ($msg:literal) => {
        return Err(syn::Error::new(proc_macro2::Span::call_site(), format!($msg)))
    };

    ($msg:ident) => {
        return Err(syn::Error::new(proc_macro2::Span::call_site(), $msg))
    };

    // Message with custom span
    ($span:expr, $msg:literal) => {
        return Err(syn::Error::new($span, format!($msg)))
    };

    ($span:expr, $msg:ident) => {
        return Err(syn::Error::new($span, $msg))
    };

    // Formatted message with call_site span
    ($fmt:literal, $($arg:tt)*) => {
        return Err(syn::Error::new(proc_macro2::Span::call_site(), format!($fmt, $($arg)*)))
    };

    // Formatted message with custom span
    ($span:expr, $fmt:literal, $($arg:tt)*) => {
        return Err(syn::Error::new($span, format!($fmt, $($arg)*)))
    };
}

// Re-export the macro for use in other modules
pub(crate) use bail;

/// Bail macro for early return with `Error::new_spanned`
///
/// Prefer this over `bail!` whenever the diagnostic points at more than a single token. A span
/// taken from a multi-token node covers the whole node only where `Span::join` is available, and
/// collapses to the node's first token elsewhere, so the same diagnostic renders differently
/// between toolchains. `Error::new_spanned` carries the start and end tokens explicitly and
/// renders the full range either way.
///
/// As in `bail!`, a literal message is a format string, so implicit captures such as `{name}`
/// interpolate and a literal brace has to be escaped. A message that is mostly braces, such as one
/// quoting `#[display("{}")]`, is better declared as a `&'static str` const and passed by name,
/// which is handed to `syn` unchanged.
///
/// Usage:
/// - `bail_spanned!(tokens, "message")`
/// - `bail_spanned!(tokens, MESSAGE_CONST)`
/// - `bail_spanned!(tokens, "format string {}", value)`
macro_rules! bail_spanned {
    ($tokens:expr, $msg:literal) => {
        return Err(syn::Error::new_spanned($tokens, format!($msg)))
    };

    ($tokens:expr, $msg:ident) => {
        return Err(syn::Error::new_spanned($tokens, $msg))
    };

    ($tokens:expr, $fmt:literal, $($arg:tt)*) => {
        return Err(syn::Error::new_spanned($tokens, format!($fmt, $($arg)*)))
    };
}

pub(crate) use bail_spanned;

/// Doc marker `#[ohno::error]` puts on the `OhnoCore` field it injects
///
/// The nonce is load-bearing: a doc string cannot be rejected when a user writes it, only fail to
/// match. See `docs/error_error.md`.
pub(crate) const GENERATED_ERROR_FIELD_MARKER: &str = " ohno::generated-core@7f3d9c2a";

/// Generate a unique field name for `OhnoCore` that doesn't conflict with existing named fields
#[cfg_attr(test, mutants::skip)] // mutation testing leads to an infinite loop here...
pub(crate) fn generate_unique_field_name(existing_fields: &[&syn::Ident]) -> syn::Ident {
    let mut candidate = "ohno_core".to_string();
    let mut counter = 0;
    while existing_fields.iter().any(|ident| ident == &AsRef::<str>::as_ref(&candidate)) {
        counter += 1;
        candidate = format!("ohno_core_{counter}");
    }

    syn::Ident::new(&candidate, proc_macro2::Span::call_site())
}

/// Assert that a token stream matches its snapshot by parsing it
/// into `syn::File`, pretty-printing with `prettyplease` and comparing
/// using insta snapshots.
///
/// Usage:
/// ```ignore
/// assert_formatted_snapshot!(result);
/// ```
#[cfg(test)]
macro_rules! assert_formatted_snapshot {
    ($tokens:expr) => {{
        let ast: syn::File = syn::parse2($tokens).expect("tokenstream is not valid Rust");
        let formatted = prettyplease::unparse(&ast);
        insta::assert_snapshot!(formatted);
    }};
}

#[cfg(test)]
pub(crate) use assert_formatted_snapshot;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_inner_error_field_name() {
        let ident1 = syn::Ident::new("msg", proc_macro2::Span::call_site());
        let ident2 = syn::Ident::new("path", proc_macro2::Span::call_site());
        let fields = vec![&ident1, &ident2];
        let name = generate_unique_field_name(&fields);
        assert_eq!(name.to_string(), "ohno_core");
    }

    #[test]
    fn test_generate_inner_error_field_name_conflict() {
        let ident1 = syn::Ident::new("ohno_core", proc_macro2::Span::call_site());
        let ident2 = syn::Ident::new("path", proc_macro2::Span::call_site());
        let fields = vec![&ident1, &ident2];
        let name = generate_unique_field_name(&fields);
        assert_eq!(name.to_string(), "ohno_core_1");
    }

    #[test]
    fn test_generate_unique_field_name_equality_check() {
        let ident1 = syn::Ident::new("ohno_core", proc_macro2::Span::call_site());
        let ident2 = syn::Ident::new("other_field", proc_macro2::Span::call_site());
        let fields = vec![&ident1, &ident2];

        let name = generate_unique_field_name(&fields);

        // Should return "ohno_core_1" because "ohno_core" already exists
        assert_eq!(name.to_string(), "ohno_core_1");

        // Verify that the returned name is NOT in the existing fields
        assert!(!fields.iter().any(|ident| name == ident.to_string()));
    }

    #[test]
    fn test_generate_unique_field_name_multiple_collisions() {
        let ident1 = syn::Ident::new("ohno_core", proc_macro2::Span::call_site());
        let ident2 = syn::Ident::new("ohno_core_1", proc_macro2::Span::call_site());
        let ident3 = syn::Ident::new("ohno_core_2", proc_macro2::Span::call_site());
        let fields = vec![&ident1, &ident2, &ident3];

        let name = generate_unique_field_name(&fields);

        // Should return "ohno_core_3", not a compounding name like "ohno_core_1_2_3"
        assert_eq!(name.to_string(), "ohno_core_3");
    }
}
