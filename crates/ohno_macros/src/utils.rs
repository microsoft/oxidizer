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

/// Generate a unique field name for `OhnoCore` that doesn't conflict with existing named fields
#[cfg_attr(test, mutants::skip)] // mutation testing leads to an infinite loop here...
pub fn generate_unique_field_name(existing_fields: &[&syn::Ident]) -> syn::Ident {
    let mut candidate = "ohno_core".to_string();
    let mut counter = 0;
    while existing_fields.iter().any(|ident| ident == &AsRef::<str>::as_ref(&candidate)) {
        counter += 1;
        candidate = format!("{candidate}_{counter}");
    }

    syn::Ident::new(&candidate, proc_macro2::Span::call_site())
}

/// Assert that two token streams are semantically identical by parsing them
/// into `syn::File`, pretty-printing with `prettyplease` and comparing the
/// resulting strings.
///
/// Usage:
/// ```ignore
/// assert_token_streams_equal!(expected_tokens, actual_tokens);
/// ```
#[cfg(test)]
macro_rules! assert_token_streams_equal {
    ($actual:expr, $expected:expr $(,)?) => {{
        let expected_ts: proc_macro2::TokenStream = $expected;
        let actual_ts: proc_macro2::TokenStream = $actual;

        let ast_actual: syn::File = syn::parse2(actual_ts).expect("actual tokenstream is not valid Rust");
        let ast_expected: syn::File = syn::parse2(expected_ts).expect("expected tokenstream is not valid Rust");

        let actual_string = prettyplease::unparse(&ast_actual);
        let expected_string = prettyplease::unparse(&ast_expected);

        pretty_assertions::assert_eq!(actual_string, expected_string);
    }};
}

#[cfg(test)]
pub(crate) use assert_token_streams_equal;

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
}
