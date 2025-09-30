<div align="center">
 <img src="./logo.png" alt="Data Privacy Logo" width="128">

# Data Privacy

[![crate.io](https://img.shields.io/crates/v/data_privacy.svg)](https://crates.io/crates/data_privacy)
[![docs.rs](https://docs.rs/data_privacy/badge.svg)](https://docs.rs/data_privacy)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)
* [Concepts](#concepts)
* [Traits](#traits)
* [Data Classes](#data-classes)
* [Classified Containers](#classified-containers)
* [Theory of Operation](#theory-of-operation)
* [Examples](#examples)

## Summary

<!-- cargo-rdme start -->

Mechanisms to classify, manipulate, and redact sensitive data.

Commercial software often needs to handle sensitive data, such as personally identifiable information (PII).
A user's name, IP address, email address, and other similar information require special treatment. For
example, it's usually not legally acceptable to emit a user's email address in a system's logs.
Following these rules can be challenging and error-prone, especially when the data is
transferred between different components of a large complex system. This crate provides
mechanisms to reduce the risk of unintentionally exposing sensitive data.

This crate's approach uses wrapping to isolate sensitive data and avoid accidental exposure.
Mechanisms are provided to automatically process sensitive data to make it safe to use in telemetry.

## Concepts

Before continuing, it's important to understand a few concepts:

* **Data Classification**: The process of tagging sensitive data with individual data classes.
  Different data classes may have different rules for handling them. For example, some sensitive
  data can be put into logs, but only for a limited time, while other data can never be logged.

* **Data Taxonomy**: A group of related data classes that together represent a consistent set
  of rules for handling sensitive data. Different companies or governments usually have their
  own taxonomies.

* **Redaction**: The process of removing or obscuring sensitive information from data.
  Redaction is often done by using consistent hashing, replacing the sensitive data with a hash
  value that is not reversible. This allows the data to be used for analysis or processing
  without exposing the sensitive information.

It's important to note that redaction is different from deletion. Redaction typically replaces sensitive data
with something else, while deletion removes the data entirely. Redaction allows for correlation since a given piece
of sensitive data will always produce the same redacted value. This makes it possible to look at many different
log records and correlate them to a specific user or entity without exposing the sensitive data itself. It's possible
to tell over time that an operation is attributed to a the same piece of state without knowing what the state is.

## Traits

This crate is built around two traits:

* The [`Classified`](https://docs.rs/data_privacy/latest/data_privacy/classified/trait.Classified.html) trait is used to mark types that hold sensitive data. The trait exposes
  explicit mechanisms to access the data in a safe and auditable way.

* The [`Redactor`](https://docs.rs/data_privacy/latest/data_privacy/redactor/trait.Redactor.html) trait defines the logic needed by an individual redactor. This crate provides a
  few implementations of this trait, such as [`SimpleRedactor`](https://docs.rs/data_privacy/latest/data_privacy/simple_redactor/struct.SimpleRedactor.html), but others can
  be implemented and used by applications as well.

## Data Classes

A [`DataClass`](https://docs.rs/data_privacy/latest/data_privacy/data_class/struct.DataClass.html) is a struct that represents a single data class within a taxonomy. The struct
contains the name of the taxonomy and the name of the data class.

## Classified Containers

Types that implement the [`Classified`] trait are said to be classified containers. They encapsulate
an instance of another type. Although containers can be created by hand, they are most commonly created
using the `taxonomy` attribute. See the documentation for the attribute to learn how you define your own
taxonomy and all its data classes.

Classified containers implement the `Debug` trait if the data they hold implements the trait. However,
the data produced by the `Debug` trait is redacted, so it does not accidentally expose the sensitive data.

Applications use the classified container types around application
data types to indicate instances of those types hold sensitive data. Although applications typically
define their own taxonomies of data classes, this crate defines three well-known data classes:

* `Sensitive<T>` which can be used for taxonomy-agnostic classification in libraries.
* `UnknownSensitivity<T>` which holds data without a known classification.
* `Insensitive<T>` which holds data that explicitly has no classification.

## Theory of Operation

How this all works:

* An application defines its own taxonomy using the `taxonomy` macro, which generates classified container types.

* The application uses the classified container types to wrap sensitive data throughout the application. This ensures the
  sensitive data is not accidentally exposed through telemetry or other means.

* On startup, the application initializes a [`RedactionEngine`](https://docs.rs/data_privacy/latest/data_privacy/redaction_engine/struct.RedactionEngine.html) using the [`RedactionEngineBuilder`](https://docs.rs/data_privacy/latest/data_privacy/redaction_engine_builder/struct.RedactionEngineBuilder.html)
  type. The engine is configured with
  redactors for each data class in the taxonomy. The redactors define how to handle sensitive data for that class. For example, for
  a given data class, a redactor may substitute the original data for a hash value, or it may replace it with asterisks.

* When it's time to log or otherwise process the sensitive data, the application uses the redaction engine to redact the data.

## Examples

This example shows how to use the `Sensitive` type to classify sensitive data.

```rust
use data_privacy::common_taxonomy::Sensitive;

struct Person {
    name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
    age: u32,
}

fn try_out() {
    let person = Person {
        name: "John Doe".to_string().into(),
        age: 30,
    };

    // doesn't compile since `Sensitive` doesn't implement `Display`
    // println!("Name: {}", person.name);

    // outputs: Name: <common/sensitive:REDACTED>"
    println!("Name: {:?}", person.name);

    // extract the data from the `Sensitive` type and outputs: Name: John Doe
    let name = person.name.declassify();
    println!("Name: {name}");
}
```

This example shows how to initialize and use a redaction engine.

```rust
use std::fmt::Write;

use data_privacy::common_taxonomy::{CommonTaxonomy, Sensitive};
use data_privacy::{RedactionEngineBuilder, Redactor, SimpleRedactor, SimpleRedactorMode};

struct Person {
    name: Sensitive<String>, // a bit of sensitive data we should not leak in logs
    age: u32,
}

fn try_out() {
    let person = Person {
        name: "John Doe".to_string().into(),
        age: 30,
    };

    let asterisk_redactor = SimpleRedactor::new();
    let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);

    // Create the redaction engine. This is typically done once when the application starts.
    let engine = RedactionEngineBuilder::new()
        .add_class_redactor(&CommonTaxonomy::Sensitive.data_class(), asterisk_redactor)
        .set_fallback_redactor(erasing_redactor)
        .build();

    let mut output_buffer = String::new();

    // Redact the sensitive data in the person's name using the redaction engine.
    engine.display_redacted(&person.name, |s| output_buffer.write_str(s).unwrap());

    // check that the data in the output buffer has indeed been redacted as expected.
    assert_eq!(output_buffer, "********");
}
```

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
