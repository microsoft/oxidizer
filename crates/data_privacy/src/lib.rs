// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Mechanisms to classify, manipulate, and redact sensitive data.
//!
//! Commercial software often needs to handle sensitive data, such as personally identifiable information (PII).
//! A user's name, IP address, email address, and other similar information require special treatment. For
//! example, it's usually not legally acceptable to emit a user's email address in a system's logs.
//! Following these rules can be challenging and error-prone, especially when the data is
//! transferred between different components of a large complex system. This crate provides
//! mechanisms to reduce the risk of unintentionally exposing sensitive data.
//!
//! This crate's approach uses wrapping to isolate sensitive data and avoid accidental exposure.
//! Mechanisms are provided to automatically process sensitive data to make it safe to use in telemetry.
//!
//! # Concepts
//!
//! Before continuing, it's important to understand a few concepts:
//!
//! * **Data Classification**: The process of assigning sensitive data individual data classes.
//!   Different data classes may have different rules for handling them. For example, some sensitive
//!   data can be put into logs, but only for a limited time, while other data can never be logged.
//!
//! * **Data Taxonomy**: A group of related data classes that together represent a consistent set
//!   of rules for handling sensitive data. Different companies or governments usually have their
//!   own taxonomies representing the different types of data they manipulate, each with specific
//!   policies.
//!
//! * **Redaction**: The process of removing or obscuring sensitive information from data.
//!   Redaction is often done by using consistent hashing, replacing the sensitive data with a hash
//!   value that is not reversible. This allows the data to be used for analysis or processing
//!   without exposing the sensitive information.
//!
//! It's important to note that redaction is different from deletion. Redaction typically replaces sensitive data
//! with something else, while deletion removes the data entirely. Redaction allows for correlation since a given piece
//! of sensitive data will always produce the same redacted value. This makes it possible to look at many different
//! log records and correlate them to a specific user or entity without exposing the sensitive data itself. It's possible
//! to tell over time that an operation is attributed to the same piece of state without knowing what the state is.
//!
//! # Traits
//!
//! This crate is built around two primary traits:
//!
//! * The [`Classified`] trait is used to mark types that hold sensitive data.
//!
//! * The [`Redactor`] trait defines the interface for applying redaction. Both
//!   [`RedactionEngine`] (the high-level engine that routes data classes to
//!   strategies) and individual redaction strategies (e.g. hash-based or
//!   replacement-based redactors) implement this trait.
//!
//! This crate also exposes additional traits which are usually, but not necessarily, implemented by types that implement the
//! [`Classified`] trait:
//!
//! - The [`RedactedDebug`] trait defines how to produce redacted debug output for classified data.
//!
//! - The [`RedactedDisplay`] trait defines how to produce redacted display output for classified data.
//!
//! - The [`RedactedToString`] trait defines how to produce a redacted string representation of classified data.
//!
//! # Taxonomies and Data Classes
//!
//! A taxonomy is defined using the [`taxonomy`] attribute macro. The macro is applied to an enum
//! declaration. Each variant of the enum represents a data class within the taxonomy.
//!
//! [`DataClass`] is a struct that represents a single data class within a taxonomy. The struct
//! contains the name of the taxonomy and the name of the data class. You can get a `DataClass` instance for a given data class
//! by calling the associated `data_class` method on the taxonomy enum.
//!
//! ```rust
//! use data_privacy::taxonomy;
//!
//! // A simple taxonomy definition for the Contoso organization.
//! #[taxonomy(contoso)]
//! enum ContosoTaxonomy {
//!     CustomerContent,
//!     CustomerIdentifier,
//!     OrganizationIdentifier,
//! }
//!
//! let dc = ContosoTaxonomy::CustomerIdentifier.data_class();
//! assert_eq!(dc.taxonomy(), "contoso");
//! assert_eq!(dc.name(), "customer_identifier");
//! ```
//!
//! # Classified Containers
//!
//! Types that implement the [`Classified`] trait are said to be _classified containers_. They encapsulate
//! an instance of another type. Although containers can be created by hand, they are most commonly created
//! using the [`classified`] attribute. See the documentation for the attribute to learn how you define your own
//! classified type.
//!
//! Applications use the classified container types around application
//! data types to indicate instances of those types hold sensitive data.
//!
//! # Theory of Operation
//!
//! How this all works:
//!
//! * An application defines its own taxonomy using the [`taxonomy`] macro.
//!
//! * An application defines classified container types using the [`classified`] attribute for each piece of sensitive data it needs to manipulate.
//!
//! * The application uses the classified container types to wrap sensitive data throughout the application. This ensures the
//!   sensitive data is not accidentally exposed through telemetry or other means.
//!
//! * On startup, the application initializes a [`RedactionEngine`] via [`RedactionEngine::builder`]. The engine is configured
//!   with redactors for each data class in the taxonomy. The redactors define how to handle sensitive data for that class.
//!   For example, for a given data class, a redactor may substitute the original data for a hash value, or it may replace it with asterisks.
//!
//! * When it's time to log or otherwise process the sensitive data, the application uses the redaction engine to redact the data.
//!
//! # Examples
//!
//! This example shows how to define a simple taxonomy and a few classified container types, and how to manipulate these
//! container types.
//!
//! ```rust
//! use data_privacy::{classified, RedactionEngine, taxonomy};
//! use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
//!
//! // A simple taxonomy definition for the Contoso organization.
//! #[taxonomy(contoso)]
//! enum ContosoTaxonomy {
//!     CustomerContent,
//!     CustomerIdentifier,
//!     OrganizationIdentifier,
//! }
//!
//! // A classified container for customer names.
//! #[classified(ContosoTaxonomy::CustomerIdentifier)]
//! struct Name(String);
//!
//! // A classified container for customer addresses.
//! #[classified(ContosoTaxonomy::CustomerIdentifier)]
//! struct Address(String);
//!
//! // A classified container for customer content.
//! #[classified(ContosoTaxonomy::CustomerContent)]
//! struct Memo(String);
//!
//! // A customer record which contains a bunch of classified data.
//! #[derive(Debug)]
//! struct Customer {
//!    name: Name,
//!    address: Address,
//!    memo : Memo,
//! }
//!
//! let c = Customer {
//!     name: Name("John Doe".to_string()),
//!     address: Address("123 Main St, Anytown, USA".to_string()),
//!     memo: Memo("Leave packages on the front porch.".to_string()),
//! };
//!
//! // Displaying the customer record will not leak sensitive data because the classified containers protect the data
//! println!("Customer record: {:?}", c);
//!
//! // You can get redacted representations of classified data using a [`RedactionEngine`](redaction_engine::RedactionEngine).
//!
//! // Initialize some redactors
//! let asterisk_redactor = SimpleRedactor::new();
//! let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
//!
//! // Create the redaction engine. This is typically done once when the application starts.
//! let engine = RedactionEngine::builder()
//!     .add_class_redactor(ContosoTaxonomy::CustomerIdentifier.data_class(), asterisk_redactor)
//!     .set_fallback_redactor(erasing_redactor)
//!     .build();
//!
//! let mut output_buffer = String::new();
//! _ = engine.redacted_display(&c.name, &mut output_buffer);
//!
//! // check that the data in the output buffer has indeed been redacted as expected.
//! assert_eq!(output_buffer, "********");
//! ```
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy/favicon.ico")]

// Needed for the `taxonomy` macro to be able to use `data_privacy` instead of `crate` in examples.
extern crate self as data_privacy;

// Re-export types and traits from data_privacy_core.
#[doc(inline)]
pub use data_privacy_core::{Classified, DataClass, IntoDataClass, RedactedDebug, RedactedDisplay, RedactedToString, Redactor};
/// Derives an implementation of the [`RedactedDebug`](trait@RedactedDebug) trait for a struct.
///
/// The generated implementation mirrors the layout produced by the standard library's
/// [`#[derive(Debug)]`](core::fmt::Debug): it prints the struct name, the field names, and the
/// surrounding delimiters. The crucial difference is that each field is routed through the supplied
/// [`Redactor`] rather than being printed verbatim, so sensitive data is redacted before it reaches
/// the output.
///
/// Every field that is not marked `#[unredacted]` must itself implement
/// [`RedactedDebug`](trait@RedactedDebug). Classified containers generated by the
/// [`classified`](macro@classified) attribute implement it automatically, so they can be nested
/// directly inside a type that derives `RedactedDebug`.
///
/// # The `#[unredacted]` attribute
///
/// Mark a field with `#[unredacted]` to opt it out of redaction. Such a field is formatted with its
/// regular [`Debug`](core::fmt::Debug) implementation, exactly as `#[derive(Debug)]` would do. Use
/// this only for fields that are known to never carry sensitive data.
///
/// # Generated output
///
/// The shape of the output depends on the kind of struct:
///
/// - Named structs render as `Name { field: value, ... }`.
/// - Tuple structs render as `Name(value, ...)`.
/// - Unit structs render as `Name`.
///
/// Deriving on an enum or union is not supported and produces a compile-time error.
///
/// # Example
///
/// ```rust
/// use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
/// use data_privacy::{RedactedDebug, RedactionEngine, classified, taxonomy};
///
/// #[taxonomy(myco)]
/// enum MyTaxonomy {
///     Pii,
/// }
///
/// // A classified container. Its `RedactedDebug` impl is generated by `#[classified]`.
/// #[classified(MyTaxonomy::Pii)]
/// struct Email(String);
///
/// #[derive(RedactedDebug)]
/// struct User {
///     email: Email, // classified: routed through the redactor
///     #[unredacted]
///     department: &'static str, // shown verbatim, using `Debug` formatting (note the quotes)
/// }
///
/// let user = User {
///     email: Email("alice@example.com".to_string()),
///     department: "engineering",
/// };
///
/// // Configure an engine that replaces every PII value with a fixed marker.
/// let engine = RedactionEngine::builder()
///     .add_class_redactor(
///         MyTaxonomy::Pii.data_class(),
///         SimpleRedactor::with_mode(SimpleRedactorMode::Insert("<redacted>".into())),
///     )
///     .build();
///
/// let mut out = String::new();
/// engine.redacted_debug(&user, &mut out).unwrap();
///
/// // The classified `email` is redacted, while the `#[unredacted]` `department` keeps its
/// // standard `Debug` representation (including the surrounding quotes).
/// assert_eq!(
///     out,
///     r#"User { email: <redacted>, department: "engineering" }"#
/// );
/// ```
#[doc(inline)]
pub use data_privacy_macros::RedactedDebug;
/// Derives an implementation of the [`RedactedDisplay`](trait@RedactedDisplay) trait for a struct.
///
/// This derive works exactly like [`RedactedDebug`](derive@RedactedDebug), but it produces
/// [`Display`](core::fmt::Display)-style output: non-`#[unredacted]` fields must implement
/// [`RedactedDisplay`](trait@RedactedDisplay) and are routed through the supplied [`Redactor`],
/// while `#[unredacted]` fields are formatted with their regular
/// [`Display`](core::fmt::Display) implementation.
///
/// Note that the generated implementation still prints the struct name and field names; the
/// `Display`/`Debug` distinction only changes how the individual field values are formatted (for
/// example, an `#[unredacted]` string is printed without surrounding quotes here, unlike with
/// [`RedactedDebug`](derive@RedactedDebug)).
///
/// # The `#[unredacted]` attribute
///
/// Mark a field with `#[unredacted]` to opt it out of redaction. Such a field is formatted with its
/// regular [`Display`](core::fmt::Display) implementation. Use this only for fields that are known
/// to never carry sensitive data.
///
/// # Generated output
///
/// The shape of the output depends on the kind of struct:
///
/// - Named structs render as `Name { field: value, ... }`.
/// - Tuple structs render as `Name(value, ...)`.
/// - Unit structs render as `Name`.
///
/// Deriving on an enum or union is not supported and produces a compile-time error.
///
/// # Example
///
/// ```rust
/// use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
/// use data_privacy::{RedactedDisplay, RedactionEngine, classified, taxonomy};
///
/// #[taxonomy(myco)]
/// enum MyTaxonomy {
///     Pii,
/// }
///
/// // A classified container. Its `RedactedDisplay` impl is generated by `#[classified]`.
/// #[classified(MyTaxonomy::Pii)]
/// struct Email(String);
///
/// #[derive(RedactedDisplay)]
/// struct User {
///     email: Email, // classified: routed through the redactor
///     #[unredacted]
///     department: &'static str, // shown verbatim, using `Display` formatting (no quotes)
/// }
///
/// let user = User {
///     email: Email("alice@example.com".to_string()),
///     department: "engineering",
/// };
///
/// // Configure an engine that replaces every PII value with a fixed marker.
/// let engine = RedactionEngine::builder()
///     .add_class_redactor(
///         MyTaxonomy::Pii.data_class(),
///         SimpleRedactor::with_mode(SimpleRedactorMode::Insert("<redacted>".into())),
///     )
///     .build();
///
/// let mut out = String::new();
/// engine.redacted_display(&user, &mut out).unwrap();
///
/// // The classified `email` is redacted, while the `#[unredacted]` `department` keeps its
/// // standard `Display` representation (no surrounding quotes).
/// assert_eq!(out, "User { email: <redacted>, department: engineering }");
/// ```
#[doc(inline)]
pub use data_privacy_macros::RedactedDisplay;
// Re-export attribute macros.
#[doc(inline)]
pub use data_privacy_macros::{classified, taxonomy};

mod redaction_engine;
mod redaction_engine_builder;
mod redaction_engine_inner;
mod redactors;
mod sensitive;

pub use redaction_engine::RedactionEngine;
pub use redaction_engine_builder::RedactionEngineBuilder;
#[cfg(feature = "rapidhash")]
pub use redactors::rapidhash_redactor;
pub use redactors::simple_redactor;
#[cfg(feature = "xxh3")]
pub use redactors::xxh3_redactor;
pub use sensitive::Sensitive;
