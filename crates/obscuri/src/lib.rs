// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/favicon.ico"
)]

//! Standards-compliant URI handling with templating, safety validation, and data classification.
//!
//! This crate provides comprehensive URI manipulation capabilities designed for HTTP clients
//! and servers that need type-safe, efficient, and data classification-aware URI handling. It builds
//! on top of the standard `http` crate while adding additional safety guarantees, templating
//! capabilities, and data classification features.
//!
//! # Core Types
//!
//! The crate centers around several key abstractions:
//!
//! - [`Uri`] - Flexible URI type with endpoint and path/query components
//! - [`BaseUri`] - Lightweight type representing scheme and authority (no path/query)
//! - [`TemplatedPathAndQuery`] - RFC 6570 Level 3 compliant URI templating
//! - [`UriSafe`] and [`UriSafeString`] - Trait and its implementation for String, marking type as safe for URI components
//!   by not containing any reserved characters
//!
//! # Basic Usage
//! ## Simple URI Construction
//!
//! ```rust
//! use obscuri::uri::{PathAndQuery, TargetPathAndQuery};
//! use obscuri::{BaseUri, Uri};
//!
//! // Create an endpoint (scheme + authority only)
//! let base_uri = BaseUri::from_uri_static("https://api.example.com");
//!
//! // Create a path (can be static for zero-allocation)
//! let path: TargetPathAndQuery = TargetPathAndQuery::from_static("/api/v1/users");
//!
//! // Combine into complete URI
//! let uri = Uri::default().base_uri(base_uri).path_and_query(path);
//! assert_eq!(
//!     uri.to_string().declassify_ref(),
//!     "https://api.example.com/api/v1/users"
//! );
//! ```
//!
//! ## Templated URIs
//!
//! For dynamic URIs with variable components, use the templating system:
//!
//! ```rust
//! use obscuri::{BaseUri, TemplatedPathAndQuery, Uri, UriSafeString, templated};
//! use uuid::Uuid;
//!
//! #[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
//! #[derive(Clone)]
//! struct UserPostPath {
//!     user_id: Uuid,
//!     post_id: UriSafeString,
//! }
//!
//! let path = UserPostPath {
//!     user_id: Uuid::new_v4(),
//!     post_id: UriSafeString::new(&"my-post").unwrap(),
//! };
//!
//! let uri = Uri::default()
//!     .base_uri(BaseUri::from_uri_static("https://api.example.com"))
//!     .path_and_query(path);
//! ```
//!
//! # URI Safety Guarantees
//!
//! The crate provides [`UriSafe`] trait implementations for types that are guaranteed
//! to contain only URI-safe characters. This prevents common URI injection vulnerabilities:
//!
//! ```rust
//! use obscuri::UriSafeString;
//!
//! // This will succeed - contains only safe characters
//! let safe = UriSafeString::new(&"hello-world_123").unwrap();
//!
//! // This will fail - contains URI-reserved characters
//! let unsafe_string = UriSafeString::new(&"hello world?foo=bar");
//! assert!(unsafe_string.is_err());
//! ```
//!
//! Built-in safe types include numeric types (`u32`, `u64`, etc.), [`Uuid`](uuid::Uuid),
//! IP addresses, and validated [`UriSafeString`] instances.
//!
//! # Telemetry Labels
//!
//! For complex templates, use the `label` attribute to provide a concise identifier
//! for telemetry. When present, the label takes precedence over the template string.
//!
//! ```rust
//! use obscuri::{UriSafeString, templated};
//!
//! #[templated(
//!     template = "/{org}/users/{user_id}/reports/{report_type}",
//!     label = "user_report",
//!     unredacted
//! )]
//! struct ReportPath {
//!     org: UriSafeString,
//!     user_id: UriSafeString,
//!     report_type: UriSafeString,
//! }
//! ```
//!
//! # Data Classification
//!
//! The crate integrates with `data_privacy` to track data sensitivity levels
//! in URIs. This is particularly important for compliance and data security:
//!
//! ```rust
//! use data_privacy::Sensitive;
//! use obscuri::{UriSafeString, templated};
//!
//! #[templated(template = "/{org_id}/user/{user_id}/")]
//! #[derive(Clone)]
//! struct UserPath {
//!     #[unredacted]
//!     org_id: UriSafeString,
//!     user_id: Sensitive<UriSafeString>,
//! }
//! ```
//!
//! # RFC 6570 Template Compliance
//!
//! The templating system implements [RFC 6570](https://datatracker.ietf.org/doc/html/rfc6570)
//! Level 3 URI Template specification. Supported expansions include:
//!
//! - Simple string expansion: `{var}`
//! - Reserved string expansion: `{+var}`
//! - Fragment expansion: `{#var}`
//! - Path segments: `{/var}`
//! - Query parameters: `{?var}`
//! - Query continuation: `{&var}`
//!
//! Template variables must implement [`UriSafe`] (except for fragment and reserved expansions)
//! to ensure the resulting URI is valid.
//!
//! # Integration with HTTP Ecosystem
//!
//! This crate seamlessly integrates with the broader Rust HTTP ecosystem by re-exporting
//! and building upon the standard [`http`](https://docs.rs/http/latest/http/) crate types. The resulting [`Uri`] can be converted
//! to an [`http::Uri`] for use with HTTP clients
//! and servers based on [`hyper`](https://docs.rs/hyper/latest/hyper/) like [`reqwest`](https://docs.rs/reqwest/latest/reqwest/).

mod base_uri;
mod error;
mod macros;
mod templated;
pub mod uri;
mod uri_fragment;
mod uri_safe;

pub use base_uri::{BasePath, BaseUri, Origin};
pub use error::ValidationError;
pub use macros::{UriFragment, UriUnsafeFragment, templated};
pub use templated::TemplatedPathAndQuery;
#[doc(inline)]
pub use uri::{DATA_CLASS_UNKNOWN_URI, Uri};
pub use uri_fragment::{UriFragment, UriUnsafeFragment};
pub use uri_safe::{UriSafe, UriSafeError, UriSafeString};
