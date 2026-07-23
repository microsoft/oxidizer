// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macro hygiene: generated code must not capture user items that shadow
//! prelude names.
//!
//! Every bound the generators emit names its item through a fully qualified
//! path, so a user item shadowing a prelude name in the surrounding module
//! cannot change the meaning of the expansion. This file compiles both macros
//! next to a user `Sized` item; a regression makes the crate fail to build
//! rather than fail an assertion.

mod resolver_with_shadowed_sized {
    use routerama::resolve::resolver;

    /// Shadows `core::marker::Sized` for every item in this module.
    struct Sized;

    #[resolver]
    #[derive(Debug, PartialEq, Eq)]
    enum Api<'p> {
        #[route(GET, "/books")]
        ListBooks,
        #[route(GET, "/books/{book}")]
        GetBook { book: &'p str },
    }

    #[test]
    fn a_shadowed_sized_does_not_break_the_generated_resolver() {
        let shadow = Sized;
        assert_eq!(size_of_val(&shadow), 0, "the shadowing item is the user's, not the prelude trait");

        let api = Api::resolver();
        assert_eq!(api.resolve("GET", "/books"), Ok(Api::ListBooks));
        assert_eq!(api.resolve("GET", "/books/rust"), Ok(Api::GetBook { book: "rust" }));
    }
}

mod router_with_shadowed_sized {
    use routerama::route::{StatusCode, router};

    /// Shadows `core::marker::Sized` for every item in this module.
    struct Sized;

    struct Api;

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
    )]
    #[router]
    impl Api {
        #[route(GET, "/books")]
        async fn list(&self) -> StatusCode {
            StatusCode::NO_CONTENT
        }
    }

    #[test]
    fn a_shadowed_sized_does_not_break_the_generated_router() {
        let shadow = Sized;
        assert_eq!(size_of_val(&shadow), 0, "the shadowing item is the user's, not the prelude trait");
        assert_eq!(size_of_val(&Api), 0, "the router expands next to the shadowing item");
    }
}
