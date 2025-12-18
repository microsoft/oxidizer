<div align="center">
 <img src="./logo.png" alt="Data Privacy Logo" width="96">

# Data Privacy

[![crate.io](https://img.shields.io/crates/v/data_privacy.svg)](https://crates.io/crates/data_privacy)
[![docs.rs](https://docs.rs/data_privacy/badge.svg)](https://docs.rs/data_privacy)
[![MSRV](https://img.shields.io/crates/msrv/data_privacy)](https://crates.io/crates/data_privacy)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Mechanisms to classify, manipulate, and redact sensitive data.

Commercial software often needs to handle sensitive data, such as personally identifiable information (PII).
A user’s name, IP address, email address, and other similar information require special treatment. For
example, it’s usually not legally acceptable to emit a user’s email address in a system’s logs.
Following these rules can be challenging and error-prone, especially when the data is
transferred between different components of a large complex system. This crate provides
mechanisms to reduce the risk of unintentionally exposing sensitive data.

This crate’s approach uses wrapping to isolate sensitive data and avoid accidental exposure.
Mechanisms are provided to automatically process sensitive data to make it safe to use in telemetry.

## Concepts

Before continuing, it’s important to understand a few concepts:

* **Data Classification**: The process of assigning sensitive data individual data classes.
  Different data classes may have different rules for handling them. For example, some sensitive
  data can be put into logs, but only for a limited time, while other data can never be logged.

* **Data Taxonomy**: A group of related data classes that together represent a consistent set
  of rules for handling sensitive data. Different companies or governments usually have their
  own taxonomies representing the different types of data they manipulate, each with specific
  policies.

* **Redaction**: The process of removing or obscuring sensitive information from data.
  Redaction is often done by using consistent hashing, replacing the sensitive data with a hash
  value that is not reversible. This allows the data to be used for analysis or processing
  without exposing the sensitive information.

It’s important to note that redaction is different from deletion. Redaction typically replaces sensitive data
with something else, while deletion removes the data entirely. Redaction allows for correlation since a given piece
of sensitive data will always produce the same redacted value. This makes it possible to look at many different
log records and correlate them to a specific user or entity without exposing the sensitive data itself. It’s possible
to tell over time that an operation is attributed to the same piece of state without knowing what the state is.

## Traits

This crate is built around two primary traits:

* The [`Classified`][__link0] trait is used to mark types that hold sensitive data. The trait exposes
  explicit mechanisms to access the data in a safe and auditable way.

* The [`Redactor`][__link1] trait defines the logic needed by an individual redactor. This crate provides a
  few implementations of this trait, such as [`SimpleRedactor`][__link2], but others can
  be implemented and used by applications as well.

This crate also exposes additional traits which are usually, but not necessarily, implemented by types that implement the
[`Classified`][__link3] trait:

* The [`RedactedDebug`][__link4] trait defines how to produce redacted debug output for classified data.

* The [`RedactedDisplay`][__link5] trait defines how to produce redacted display output for classified data.

* The [`RedactedToString`][__link6] trait defines how to produce a redacted string representation of classified data.

## Taxonomies and Data Classes

A taxonomy is defined using the [`taxonomy`][__link7] attribute macro. The macro is applied to an enum
declaration. Each variant of the enum represents a data class within the taxonomy.

[`DataClass`][__link8] is a struct that represents a single data class within a taxonomy. The struct
contains the name of the taxonomy and the name of the data class. You can get a `DataClass` instance for a given data class
by calling the associated `data_class` method on the taxonomy enum.

```rust
use data_privacy::taxonomy;

// A simple taxonomy definition for the Contoso organization.
#[taxonomy(contoso)]
enum ContosoTaxonomy {
    CustomerContent,
    CustomerIdentifier,
    OrganizationIdentifier,
}

let dc = ContosoTaxonomy::CustomerIdentifier.data_class();
assert_eq!(dc.taxonomy(), "contoso");
assert_eq!(dc.name(), "customer_identifier");
```

## Classified Containers

Types that implement the [`Classified`][__link9] trait are said to be classified containers. They encapsulate
an instance of another type. Although containers can be created by hand, they are most commonly created
using the [`classified`][__link10] attribute. See the documentation for the attribute to learn how you define your own
classified type.

Applications use the classified container types around application
data types to indicate instances of those types hold sensitive data.

## Theory of Operation

How this all works:

* An application defines its own taxonomy using the [`taxonomy`][__link11] macro.

* An application defines classified container types using the [`classified`][__link12] attribute for each piece of sensitive data it needs to manipulate.

* The application uses the classified container types to wrap sensitive data throughout the application. This ensures the
  sensitive data is not accidentally exposed through telemetry or other means.

* On startup, the application initializes a [`RedactionEngine`][__link13] via [`RedactionEngine::builder()`][__link14]. The engine is configured
  with redactors for each data class in the taxonomy. The redactors define how to handle sensitive data for that class.
  For example, for a given data class, a redactor may substitute the original data for a hash value, or it may replace it with asterisks.

* When it’s time to log or otherwise process the sensitive data, the application uses the redaction engine to redact the data.

## Examples

This example shows how to define a simple taxonomy and a few classified container types, and how to manipulate these
container types.

```rust
use data_privacy::{classified, RedactionEngine, taxonomy};
use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};

// A simple taxonomy definition for the Contoso organization.
#[taxonomy(contoso)]
enum ContosoTaxonomy {
    CustomerContent,
    CustomerIdentifier,
    OrganizationIdentifier,
}

// A classified container for customer names.
#[classified(ContosoTaxonomy::CustomerIdentifier)]
struct Name(String);

// A classified container for customer addresses.
#[classified(ContosoTaxonomy::CustomerIdentifier)]
struct Address(String);

// A classified container for customer content.
#[classified(ContosoTaxonomy::CustomerContent)]
struct Memo(String);

// A customer record which contains a bunch of classified data.
#[derive(Debug)]
struct Customer {
   name: Name,
   address: Address,
   memo : Memo,
}

let c = Customer {
    name: Name("John Doe".to_string()),
    address: Address("123 Main St, Anytown, USA".to_string()),
    memo: Memo("Leave packages on the front porch.".to_string()),
};

// Displaying the customer record will not leak sensitive data because the classified containers protect the data
println!("Customer record: {:?}", c);

// You can get redacted representations of classified data using a [`RedactionEngine`](redaction_engine::RedactionEngine).

// Initialize some redactors
let asterisk_redactor = SimpleRedactor::new();
let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

// Create the redaction engine. This is typically done once when the application starts.
let engine = RedactionEngine::builder()
    .add_class_redactor(ContosoTaxonomy::CustomerIdentifier.data_class(), asterisk_redactor)
    .set_fallback_redactor(erasing_redactor)
    .build();

let mut output_buffer = String::new();
_ = engine.redacted_display(&c.name, &mut output_buffer);

// check that the data in the output buffer has indeed been redacted as expected.
assert_eq!(output_buffer, "********");
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/data_privacy">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG60g5Zo_p4DxG7thIbirEmXiG-Rmydk-fpIuG-OodFo7tu4EYWSBgmxkYXRhX3ByaXZhY3lmMC4xMC4w
 [__link0]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=Classified
 [__link1]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=Redactor
 [__link10]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=classified
 [__link11]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=taxonomy
 [__link12]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=classified
 [__link13]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=RedactionEngine
 [__link14]: `RedactionEngine::builder()`
 [__link2]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=simple_redactor::SimpleRedactor
 [__link3]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=Classified
 [__link4]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=RedactedDebug
 [__link5]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=RedactedDisplay
 [__link6]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=RedactedToString
 [__link7]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=taxonomy
 [__link8]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=DataClass
 [__link9]: https://docs.rs/data_privacy/0.10.0/data_privacy/?search=Classified
