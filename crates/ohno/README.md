<div align="center">
 <img src="./logo.png" alt="Ohno Logo" width="96">

# Ohno

[![crate.io](https://img.shields.io/crates/v/ohno.svg)](https://crates.io/crates/ohno)
[![docs.rs](https://docs.rs/ohno/badge.svg)](https://docs.rs/ohno)
[![MSRV](https://img.shields.io/crates/msrv/ohno)](https://crates.io/crates/ohno)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

High-quality error handling for Rust.

Ohno combines error wrapping, enrichment messages stacking, backtrace capture, and procedural macros
into one ergonomic crate for comprehensive error handling.

## Key Features

* [**`#[derive(Error)]`**](#derive-macro): Derive macro for automatic `std::error::Error`, [`Display`][__link0], [`Debug`][__link1] implementations
* [**`#[error]`**](#ohnoerror): Attribute macro for creating error types
* [**`#[enrich_err("...")]`**](#error-enrichment): Attribute macro for automatic error enrichment with file and line information.
* [**`ErrorExt`**][__link2]: Trait that provides additional methods for ohno error types, it’s implemented automatically for all ohno error types
* [**`OhnoCore`**][__link3]: Core error type that wraps source errors, captures backtraces, and holds enrichment entries
* [**`AppError`**][__link4]: Application-level error type for general application errors

## Quick Start

```rust
use std::path::{Path, PathBuf};

#[ohno::error]
pub struct ConfigError(PathBuf);

#[ohno::enrich_err("failed to open file {}", path.as_ref().display())]
fn open_file(path: impl AsRef<Path>) -> Result<String, ConfigError> {
    std::fs::read_to_string(path.as_ref())
        .map_err(|e| ConfigError::caused_by(path.as_ref().to_path_buf(), e))
}
```

## Derive Macro

Derive macro for automatically implementing error traits.

When applied to a struct or enum containing an [`OhnoCore`][__link5] field,
this macro automatically implements [`std::error::Error`][__link6], [`std::fmt::Display`][__link7], [`std::fmt::Debug`][__link8], and [`From`][__link9] conversions.

 > 
 > **Note**: `From<std::convert::Infallible>` is implemented by default and calls via [`unreachable!`][__link10] macro.

```rust
use ohno::{OhnoCore, Error};

#[derive(Error)]
pub struct MyError {
    inner_error: OhnoCore,
}
```

## `ohno::error`

The `#[ohno::error]` attribute macro is a convenience wrapper that automatically adds a `OhnoCore`
field to your struct and applies `#[derive(Error)]`. This is the simplest way to create error types
without manually managing the error infrastructure.

```rust
// Simple error without extra fields
#[ohno::error]
pub struct ParseError;

// Error with multiple fields
#[ohno::error]
pub struct NetworkError {
    host: String,
    port: u16,
}
```

## Display Error Override

The `#[display("...")]` attribute allows you to customize the main error message
while preserving the underlying error as a cause in the error chain.

```rust
use std::path::PathBuf;

#[ohno::error]
#[display("Failed to read config with path: {path}")]
pub struct ConfigError {
    pub path: String,
}

// Usage
let error = ConfigError::caused_by("/etc/config.toml", "file not found");

// Output: "Failed to read config with path: /etc/config.toml\nCaused by:\n\tfile not found"
```

The template string supports field interpolation using `{field_name}` syntax. The underlying
error (if any) is automatically shown as “Caused by:” in the error chain. If the inner error
has no source, only the custom message is displayed.

## Automatic Constructors

By default, `#[derive(Error)]` automatically generates `new()` and `caused_by()` constructor methods:

```rust
#[ohno::error]
struct ConfigError {
    path: String,
}

// The derive macro automatically generates:
// - ConfigError::new(path: String) -> Self
// - ConfigError::caused_by(path: String, error: impl Into<Box<dyn Error...>>) -> Self

let error = ConfigError::new("/etc/config.toml");
let error_with_cause = ConfigError::caused_by("/etc/config.toml", "File not found");
```

**Disabling Automatic Constructors:**

Use `#[no_constructors]` to disable automatic generation when you need custom constructors:

```rust
use ohno::{Error, OhnoCore};

#[derive(Error)]
#[no_constructors]
struct CustomError {
    inner_error: OhnoCore,
}

impl CustomError {
    pub fn new(custom_logic: bool) -> Self {
        // Your custom constructor logic here
        Self { inner_error: OhnoCore::default() }
    }
}
```

## Automatic From Implementations

The `#[from(Type1, Type2, ...)]` attribute automatically generates `From<Type>` implementations
for the specified types. Other fields in the struct are defaulted using `Default::default()`.

```rust
#[ohno::error]
#[derive(Default)]
#[from(std::io::Error, std::fmt::Error)]
struct MyError {
    optional_field: Option<String>,
    code: i32,
}

// This generates:
// impl From<std::io::Error> for MyError { ... }
// impl From<std::fmt::Error> for MyError { ... }

let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
let my_err: MyError = io_err.into(); // Works automatically
// optional_field = None, code = 0 (defaulted)
```

**Note:** Error’s fields must implement `Default` when using `#[from]` to ensure they can be properly initialized.

## Error Enrichment

The [`#[enrich_err("message")]`][__link11] attribute macro adds error enrichment with file and line info to function errors.

Functions annotated with [`#[enrich_err("message")]`][__link12] automatically wrap any returned `Result`. If
the function returns an error, the macro injects a message, including file and line information, into the error chain.

**Requirements:**

* The function must return a type that implements the `map_err` method (such as `Result` or `Poll`)
* The error type must implement the [`Enrichable`][__link13] trait (automatically implemented for all ohno error types)

**Supported syntax patterns:**

1. **Simple string literals:**

```rust
#[enrich_err("failed to process request")]
fn process() -> Result<(), MyError> { /* ... */ }
```

2. **Parameter interpolation:**

```rust
#[enrich_err("failed to read file: {path}")]
fn read_file(path: &str) -> Result<String, MyError> { /* ... */ }
```

3. **Complex expressions with method calls:**

```rust
use std::path::Path;

#[enrich_err("failed to read file: {}", path.display())]
fn read_file(path: &Path) -> Result<String, MyError> { /* ... */ }
```

4. **Multiple expressions and calculations:**

```rust
#[enrich_err("processed {} items with total size {} bytes", items.len(), total_size)]
fn process_items(items: &[String], total_size: usize) -> Result<(), MyError> { /* ... */ }
```

5. **Mixed parameter interpolation and format expressions:**

```rust
#[enrich_err("user {user} failed operation with {} items", items.len())]
fn user_operation(user: &str, items: &[String]) -> Result<(), MyError> { /* ... */ }
```

All patterns include file and line information automatically:

```rust
#[ohno::error]
struct MyError;

#[ohno::enrich_err("failed to open file")]
fn open_file(path: &str) -> Result<String, MyError> {
    std::fs::read_to_string(path)
        .map_err(MyError::caused_by)
}
// Error output will include: "failed to open file (at src/main.rs:42)"
```

## AppError

For applications that need a simple, catch-all error type, use [`app::AppError`][__link14]. It
automatically captures backtraces and can wrap any error type.

```rust
use ohno::app::{AppError, Result};

fn process() -> Result<()> {
    std::fs::read_to_string("file.txt")?; // Automatically converts errors
    Ok(())
}
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/ohno">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG8VzlgcwZoaHG1lpW_HjUJBtG0CPpcKgZaBLGyhb9wtav2vDYWSCgmRvaG5vZTAuMi4wgmtvaG5vX21hY3Jvc2UwLjIuMA
 [__link0]: https://doc.rust-lang.org/stable/std/?search=fmt::Display
 [__link1]: https://doc.rust-lang.org/stable/std/?search=fmt::Debug
 [__link10]: https://doc.rust-lang.org/stable/std/macro.unreachable.html
 [__link11]: https://docs.rs/ohno_macros/0.2.0/ohno_macros/?search=enrich_err
 [__link12]: https://docs.rs/ohno_macros/0.2.0/ohno_macros/?search=enrich_err
 [__link13]: https://docs.rs/ohno/0.2.0/ohno/?search=Enrichable
 [__link14]: https://docs.rs/ohno/0.2.0/ohno/app/index.html
 [__link2]: https://docs.rs/ohno/0.2.0/ohno/?search=ErrorExt
 [__link3]: https://docs.rs/ohno/0.2.0/ohno/?search=OhnoCore
 [__link4]: https://docs.rs/ohno/0.2.0/ohno/app/index.html
 [__link5]: https://docs.rs/ohno/0.2.0/ohno/?search=OhnoCore
 [__link6]: https://doc.rust-lang.org/stable/std/?search=error::Error
 [__link7]: https://doc.rust-lang.org/stable/std/?search=fmt::Display
 [__link8]: https://doc.rust-lang.org/stable/std/?search=fmt::Debug
 [__link9]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
