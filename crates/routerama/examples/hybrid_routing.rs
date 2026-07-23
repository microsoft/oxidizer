// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static and runtime-configured routes in one typed resolver.

#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::resolve::{HttpMethod, ResolveError, resolver};

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum Api<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },

    #[route(GET, "/health")]
    Health,

    #[route(dynamic)]
    Plugin { name: String },
    #[route(dynamic)]
    PluginAction { name: String, action: String },
    #[route(dynamic)]
    ExtensionBook { book: String },
}

/// The action a resolved request maps to, tagged with which side served it.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// A built-in route served by the static trie.
    Static(&'static str),
    /// A run-time route served by the dynamic registrations.
    Dynamic(String),
    /// A matched route whose capture could not be coerced.
    BadRequest,
    /// No route in either set matched.
    NotFound,
}

fn main() {
    let resolver = Api::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .add_plugin_action(HttpMethod::POST, "/plugins/{name}/{action}")
        .add_extension_book(HttpMethod::GET, "/books/{book}")
        .add_extension_book(HttpMethod::GET, "/extensions/books/{book}")
        .build()
        .expect("all plugin routes are registered with matching captures");

    let dispatch = |method: &str, path: &str| -> Action {
        match resolver.resolve(method, path) {
            Ok(Api::ListBooks) => Action::Static("ListBooks"),
            Ok(Api::GetBook { book }) => {
                assert_eq!(book, "rust");
                Action::Static("GetBook")
            }
            Ok(Api::Health) => Action::Static("Health"),
            Ok(Api::Plugin { name }) => Action::Dynamic(format!("Plugin({name})")),
            Ok(Api::PluginAction { name, action }) => Action::Dynamic(format!("PluginAction({name}:{action})")),
            Ok(Api::ExtensionBook { book }) => Action::Dynamic(format!("ExtensionBook({book})")),
            Err(ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_)) => {
                Action::BadRequest
            }
            Err(ResolveError::NotFound(_)) => Action::NotFound,
            Err(error) => unreachable!("unknown resolution error: {error}"),
        }
    };

    assert_eq!(dispatch("GET", "/books/rust"), Action::Static("GetBook"));
    assert_eq!(dispatch("GET", "/health"), Action::Static("Health"));

    assert_ne!(dispatch("GET", "/books/rust"), Action::Dynamic("ExtensionBook(rust)".to_owned()));

    assert_eq!(dispatch("GET", "/plugins/auth"), Action::Dynamic("Plugin(auth)".to_owned()));
    assert_eq!(
        dispatch("POST", "/plugins/auth/enable"),
        Action::Dynamic("PluginAction(auth:enable)".to_owned())
    );
    assert_eq!(
        dispatch("GET", "/extensions/books/rust"),
        Action::Dynamic("ExtensionBook(rust)".to_owned())
    );

    assert_eq!(dispatch("GET", "/nope"), Action::NotFound);

    println!("hybrid_routing: all assertions passed");
}
