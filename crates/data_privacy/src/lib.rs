// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(rustdoc::redundant_explicit_links, reason = "Needed to support cargo-rdme link mapping.")]

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
//! * **Data Classification**: The process of tagging sensitive data with individual data classes.
//!   Different data classes may have different rules for handling them. For example, some sensitive
//!   data can be put into logs, but only for a limited time, while other data can never be logged.
//!
//! * **Data Taxonomy**: A group of related data classes that together represent a consistent set
//!   of rules for handling sensitive data. Different companies or governments usually have their
//!   own taxonomies.
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
//! to tell over time that an operation is attributed to a the same piece of state without knowing what the state is.
//!
//! # Traits
//!
//! This crate is built around two traits:
//!
//! * The [`Classified`](crate::classified::Classified) trait is used to mark types that hold sensitive data. The trait exposes
//!   explicit mechanisms to access the data in a safe and auditable way.
//!
//! * The [`Redactor`](crate::redactor::Redactor) trait defines the logic needed by an individual redactor. This crate provides a
//!   few implementations of this trait, such as [`SimpleRedactor`](crate::simple_redactor::SimpleRedactor), but others can
//!   be implemented and used by applications as well.
//!
//! # Data Classes
//!
//! A [`DataClass`](crate::data_class::DataClass) is a struct that represents a single data class within a taxonomy. The struct
//! contains the name of the taxonomy and the name of the data class.
//!
//! # Classified Containers
//!
//! Types that implement the [`Classified`] trait are said to be classified containers. They encapsulate
//! an instance of another type. Although containers can be created by hand, they are most commonly created
//! using the [`taxonomy`](crate::taxonomy) attribute. See the documentation for the attribute to learn how you define your own
//! taxonomy and all its data classes.
//!
//! Classified containers implement the `Debug` trait if the data they hold implements the trait. However,
//! the data produced by the `Debug` trait is redacted, so it does not accidentally expose the sensitive data.
//!
//! Applications use the classified container types around application
//! data types to indicate instances of those types hold sensitive data. Although applications typically
//! define their own taxonomies of data classes, this crate defines three well-known data classes:
//!
//! * [`Sensitive<T>`](crate::common_taxonomy::Sensitive) which can be used for taxonomy-agnostic classification in libraries.
//! * [`UnknownSensitivity<T>`](crate::common_taxonomy::UnknownSensitivity) which holds data without a known classification.
//! * [`Insensitive<T>`](crate::common_taxonomy::Insensitive) which holds data that explicitly has no classification.
//!
//! # Theory of Operation
//!
//! How this all works:
//!
//! * An application defines its own taxonomy using the [`taxonomy`](crate::taxonomy) macro, which generates classified container types.
//!
//! * The application uses the classified container types to wrap sensitive data throughout the application. This ensures the
//!   sensitive data is not accidentally exposed through telemetry or other means.
//!
//! * On startup, the application initializes a [`RedactionEngine`](crate::redaction_engine::RedactionEngine) using the [`RedactionEngineBuilder`](crate::redaction_engine_builder::RedactionEngineBuilder)
//!   type. The engine is configured with
//!   redactors for each data class in the taxonomy. The redactors define how to handle sensitive data for that class. For example, for
//!   a given data class, a redactor may substitute the original data for a hash value, or it may replace it with asterisks.
//!
//! * When it's time to log or otherwise process the sensitive data, the application uses the redaction engine to redact the data.
//!
//! # Examples
//!
//! This example shows how to use the `Sensitive` type to classify sensitive data.
//!
//! ```rust
//! use data_privacy::common_taxonomy::Sensitive;
//!
//! struct Person {
//!     name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
//!     age: u32,
//! }
//!
//! fn try_out() {
//!     let person = Person {
//!         name: "John Doe".to_string().into(),
//!         age: 30,
//!     };
//!
//!     // doesn't compile since `Sensitive` doesn't implement `Display`
//!     // println!("Name: {}", person.name);
//!
//!     // outputs: Name: <common/sensitive:REDACTED>"
//!     println!("Name: {:?}", person.name);
//!
//!     // extract the data from the `Sensitive` type and outputs: Name: John Doe
//!     let name = person.name.declassify();
//!     println!("Name: {name}");
//! }
//! #
//! # fn main() {
//! #     try_out();
//! # }
//! ```
//!
//! This example shows how to initialize and use a redaction engine.
//!
//! ```rust
//! use std::fmt::Write;
//!
//! use data_privacy::common_taxonomy::{CommonTaxonomy, Sensitive};
//! use data_privacy::{RedactionEngineBuilder, Redactor, SimpleRedactor, SimpleRedactorMode};
//!
//! struct Person {
//!     name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
//!     age: u32,
//! }
//!
//! fn try_out() {
//!     let person = Person {
//!         name: "John Doe".to_string().into(),
//!         age: 30,
//!     };
//!
//!     let asterisk_redactor = SimpleRedactor::new();
//!     let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
//!
//!     // Create the redaction engine. This is typically done once when the application starts.
//!     let engine = RedactionEngineBuilder::new()
//!         .add_class_redactor(&CommonTaxonomy::Sensitive.data_class(), asterisk_redactor)
//!         .set_fallback_redactor(erasing_redactor)
//!         .build();
//!
//!     let mut output_buffer = String::new();
//!
//!     // Redact the sensitive data in the person's name using the redaction engine.
//!     engine.display_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());
//!
//!     // check that the data in the output buffer has indeed been redacted as expected.
//!     assert_eq!(output_buffer, "********");
//! }
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy/favicon.ico")]

mod classified;
mod classified_wrapper;
pub mod common_taxonomy;
mod data_class;
mod redaction_engine;
mod redaction_engine_builder;
mod redactor;
mod redactors;
mod simple_redactor;

#[cfg(feature = "xxh3")]
mod xxh3_redactor;

// Needed for the `taxonomy` macro to be able to use `data_privacy` instead of `crate` in examples
// Workaround for https://github.com/bkchr/proc-macro-crate/issues/14
extern crate self as data_privacy;

pub use classified::Classified;
pub use classified_wrapper::ClassifiedWrapper;
pub use data_class::DataClass;
/// Generates implementation logic and types to expose a data taxonomy.
///
/// This macro is applied to an enum declaration. Each variant of the enum
/// represents a data class within the taxonomy.
///
/// You provide a taxonomy name as first argument, followed by an optional `serde = false` or `serde = true`
/// argument to control whether serde support is included in the generated taxonomy code.
/// The default value for `serde` is `true`, meaning that serde support is included by default.
///
/// This attribute produces an implementation block for the enum which includes one method for
/// each variant of the enum. These methods each return a [`DataClass`] instance representing that data class.
/// In addition, classified data container types are generated for each data class.
///
/// ## Example
///
/// ```ignore
/// use data_privacy::taxonomy;
///
/// #[taxonomy(contoso, serde = false)]
/// enum ContosoTaxonomy {
///     CustomerContent,
///     CustomerIdentifier,
///     OrganizationIdentifier,
/// }
/// ```
pub use data_privacy_macros::taxonomy;
pub use redaction_engine::RedactionEngine;
pub use redaction_engine_builder::RedactionEngineBuilder;
pub use redactor::Redactor;
pub use simple_redactor::{SimpleRedactor, SimpleRedactorMode};

#[cfg(feature = "xxh3")]
pub use crate::xxh3_redactor::xxH3Redactor;
