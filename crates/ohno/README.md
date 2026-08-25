<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Ohno Logo" width="96">

# Ohno

[![crate.io](https://img.shields.io/crates/v/ohno.svg)](https://crates.io/crates/ohno)
[![docs.rs](https://docs.rs/ohno/badge.svg)](https://docs.rs/ohno)
[![MSRV](https://img.shields.io/crates/msrv/ohno)](https://crates.io/crates/ohno)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

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

When applied to a struct containing an [`OhnoCore`][__link5] field, this macro automatically implements [`std::error::Error`][__link6], [`std::fmt::Display`][__link7], [`std::fmt::Debug`][__link8], and [`From`][__link9] conversions.

 > 
 > **Note**: `From<std::convert::Infallible>` is implemented by default and calls via [`unreachable!`][__link10] macro.

```rust
use ohno::{Error, OhnoCore};

#[derive(Error)]
pub struct MyError {
    inner_error: OhnoCore,
}
```

## `ohno::error`

The `#[ohno::error]` attribute macro is a convenience wrapper that automatically adds a `OhnoCore`
field to the struct and applies `#[derive(Error)]`. This is the simplest way to create error types
without manually managing the error infrastructure.

The attribute always adds that field and always generates the error representation from it, so
no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use
`#[derive(Error)]` directly to place the core by hand.

A field of type `OhnoCore` may still be declared, and is then treated as data rather than as the
error: it is passed to the generated constructors like any other field, appears in the generated
`Debug`, and can be referenced from a `#[display(...)]` template — but it is never read for
`source()`, the backtrace, or enrichment, which all come from the injected field.

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

## How error text is rendered

Without a `#[display("...")]` attribute, the text an error renders depends on whether it has a
cause — an error or a string handed to `caused_by`.

**With a cause**, the cause’s message is printed as it stands: no type name, and no
`caused by:` line, so the wrapper leaves no trace in the message line. Enrichment and a
backtrace, described below, are still written after it.
A cause that is an error also stays in the [`source()`][__link11] chain, so a
caller that walks the chain still finds it.

```rust
use std::io;

#[ohno::error]
pub struct ConfigError;

fn read_config() -> Result<String, ConfigError> {
    Err(ConfigError::caused_by(io::Error::new(
        io::ErrorKind::NotFound,
        "no such file: /etc/app.toml",
    )))
}

let error = read_config().unwrap_err();

println!("{error}");
// Output: no such file: /etc/app.toml
```

A cause given as a string renders the same way, but it does not join the chain: it is a message
rather than an error, so `source()` returns `None` for it.

Wrapping therefore adds nothing to the text. A wrapper that should say what it was attempting —
“failed to load the configuration”, say — has to be given a template; see
[Overriding error text](#overriding-error-text).

**Without a cause**, there is no message to pass through, so the type’s own name is printed:

```rust
#[ohno::error]
pub struct ConfigError;

let error = ConfigError::new();

println!("{error}");
// Output: ConfigError
```

That is a symbol, not an explanation, so an error that renders as its own bare name is a sign
that it needs either a cause or a template.

### Enrichment and backtraces

The message is only the first part of what `Display` writes — with a template and a cause it is
already two lines. Each enrichment entry follows it on its own line, marked with `>` and tagged
with the place it was added:

```rust
use std::io;

#[ohno::error]
pub struct ConfigError;

#[ohno::enrich_err("failed to load the service configuration")]
fn read_config() -> Result<String, ConfigError> {
    Err(ConfigError::caused_by(io::Error::new(
        io::ErrorKind::NotFound,
        "no such file: /etc/app.toml",
    )))
}

let error = read_config().unwrap_err();

println!("{error}");
// Output: no such file: /etc/app.toml
//         > failed to load the service configuration (at src/config.rs:6)
```

A captured backtrace comes last for that level, after that level’s enrichment:

```text
no such file: /etc/app.toml
> failed to load the service configuration (at src/config.rs:6)

Backtrace:
   0: std::backtrace::Backtrace::capture
   1: ohno::backtrace::Backtrace::capture
   2: ohno::core::OhnoCore::from_source
   3: my_app::config::ConfigError::caused_by
   4: my_app::config::read_config
   ...
```

Whether a backtrace is captured at all is the standard library’s decision — see its
[environment variables][__link12].
Use [`ErrorExt::message()`][__link13] to read the message without this level’s own
enrichment or backtrace.

Every error owns its [`OhnoCore`][__link14], and every core renders its own backtrace, so a chain of
wrappers that all use the default rendering prints the message once and one backtrace block per
level. The levels are written in turn, innermost first — a wrapper’s own enrichment and
backtrace follow the complete rendering of the level it wraps, so an outer enrichment entry
appears *after* the inner level’s `Backtrace:` block, not alongside the other enrichment:

```text
no such file: /etc/app.toml

Backtrace:
   ... frames from where ConfigError wrapped the io::Error ...

Backtrace:
   ... frames from where StartupError wrapped the ConfigError ...
```

This follows from each type holding its own core, and is worth knowing before a type is wrapped
several layers deep.

## Overriding error text

The `#[display("...")]` attribute replaces the rendered message with a template of its own,
while still printing the cause after it. A cause that is an error also stays in the
[`source()`][__link15] chain; a cause given as a string is printed the same way
but does not join the chain, exactly as under the default rendering.

```rust
use std::path::PathBuf;

#[ohno::error]
#[display("Failed to read config with path: {path}")]
pub struct ConfigError {
    pub path: String,
}

// Usage
let error = ConfigError::caused_by("/etc/config.toml", "file not found");

// Output: "Failed to read config with path: /etc/config.toml\ncaused by: file not found"
```

The template string supports field interpolation using `{field_name}` syntax. Unlike the
default rendering, the cause is never printed on its own: the custom message always leads, and
the cause (if any) follows on the next line, after a `caused by:` label. If the error has no
cause, only the custom message is displayed — the type name is never used once a template is
given.

Fields of a tuple struct are interpolated by index, using `{0}`, `{1}`, and so on.

### Format Arguments

Anything that is not a plain field reference is passed as a positional argument, with
`format!`’s placeholder and argument-counting semantics:

```rust
use std::path::PathBuf;

#[ohno::error]
#[display("failed to read config: {}", path.display())]
pub struct ConfigError {
    pub path: PathBuf,
}
```

Positional arguments are implicitly scoped to `self`, so a field is referenced by its bare
name. Neither the `self.` prefix nor the leading-dot form is accepted:

|Argument|Accepted|
|--------|--------|
|`path.display()`|yes|
|`self.path.display()`|no, the `self.` prefix is implicit|
|`.path.display()`|no, not a valid expression|

## Automatic Constructors

By default, `#[derive(Error)]` automatically generates `new()` and `caused_by()` constructor methods:

```rust
#[ohno::error]
struct ConfigError {
    path: String,
}

// The derive macro automatically generates:
//
// impl ConfigError {
//     pub(crate) fn new(path: impl Into<String>) -> Self { ... }
//     pub(crate) fn caused_by(path: impl Into<String>, error: impl Into<Box<dyn Error...>>) -> Self { ... }
// }

let error = ConfigError::new("/etc/config.toml");
let error_with_cause = ConfigError::caused_by("/etc/config.toml", "File not found");
```

**The generated constructors are `pub(crate)`, regardless of the visibility of the error type
itself.** They are an implementation convenience for the crate that defines the error, not part
of its public API, so a `pub struct` error exported from a library cannot be constructed with
`new()` or `caused_by()` by a downstream crate. This is deliberate: it keeps the set of ways an
error can be built under the control of the crate that owns it, so adding a field is not a
breaking change for callers.

**Disabling Automatic Constructors:**

`#[no_constructors]` disables the generated constructors, leaving the names `new` and
`caused_by` free for hand-written versions. It works only with `#[derive(Error)]`, which
requires the `OhnoCore` field to be declared explicitly — and that field is the one the
hand-written constructor has to initialize:

```rust
use ohno::{Error, OhnoCore};

#[derive(Error)]
#[no_constructors]
struct CustomError {
    inner_error: OhnoCore,
}

impl CustomError {
    pub fn new(custom_logic: bool) -> Self {
        // Custom constructor logic here
        Self {
            inner_error: OhnoCore::default(),
        }
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

The [`#[enrich_err("message")]`][__link16] attribute macro adds error enrichment with file and line info to function errors.

Functions annotated with [`#[enrich_err("message")]`][__link17] automatically wrap any returned `Result`. If
the function returns an error, the macro injects a message, including file and line information, into the error chain.

**Requirements:**

* The function must return a type that implements the `map_err` method (such as `Result` or `Poll`)
* The error type must implement the [`Enrichable`][__link18] trait (automatically implemented for all ohno error types)

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
    std::fs::read_to_string(path).map_err(MyError::caused_by)
}
// Error output will include: "failed to open file (at src/main.rs:42)"
```

## AppError

For applications that need a simple, catch-all error type, use [`AppError`][__link19]. It
automatically captures backtraces and can wrap any error type.

To avoid accidental usage in libraries, [`AppError`][__link20] is only available when the `app-err`
feature is enabled.

Example usage:

```rust
use ohno::AppError;

fn process() -> Result<(), AppError> {
    std::fs::read_to_string("file.txt")?; // Automatically converts errors
    Ok(())
}
```

## Error Labeling

[`ErrorLabel`][__link21] is a low-cardinality string label for errors, intended for use as a metric
tag or structured log field. Labels must be chosen from a small, bounded set known at
development time to avoid high-cardinality metric series.

```rust
use ohno::ErrorLabel;

let label: ErrorLabel = ErrorLabel::from_static("timeout");
assert_eq!(label, "timeout");

let label = ErrorLabel::from_parts(["http", "client", "timeout"]);
assert_eq!(label, "http.client.timeout");
```

Use [`ErrorLabel::from_error_chain`][__link22] to walk an error’s [`source`][__link23]
chain and build a dotted label from recognized errors:

```rust
use ohno::ErrorLabel;

let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
let label = ErrorLabel::from_error_chain(&io_err, |e| {
    e.downcast_ref::<std::io::Error>()
        .map(|io| ErrorLabel::from(io.kind()))
});
assert_eq!(label, "connection_refused");
```

Types that carry an [`ErrorLabel`][__link24] can implement the [`Labeled`][__link25] trait to expose it
uniformly via [`Labeled::label`][__link26].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/ohno">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbPt0s3Sb8yJUbtV_MElvrqIMbHWX1B21g8MIbor0e9qvU6hVhZIKCZG9obm9lMC40LjCCa29obm9fbWFjcm9zZTAuNC4w
 [__link0]: https://doc.rust-lang.org/stable/std/?search=fmt::Display
 [__link1]: https://doc.rust-lang.org/stable/std/?search=fmt::Debug
 [__link10]: https://doc.rust-lang.org/stable/std/macro.unreachable.html
 [__link11]: https://doc.rust-lang.org/stable/std/?search=error::Error::source
 [__link12]: https://doc.rust-lang.org/std/backtrace/index.html#environment-variables
 [__link13]: https://docs.rs/ohno/0.4.0/ohno/?search=ErrorExt::message
 [__link14]: https://docs.rs/ohno/0.4.0/ohno/?search=OhnoCore
 [__link15]: https://doc.rust-lang.org/stable/std/?search=error::Error::source
 [__link16]: https://docs.rs/ohno_macros/0.4.0/ohno_macros/?search=enrich_err
 [__link17]: https://docs.rs/ohno_macros/0.4.0/ohno_macros/?search=enrich_err
 [__link18]: https://docs.rs/ohno/0.4.0/ohno/?search=Enrichable
 [__link19]: https://docs.rs/ohno/0.4.0/ohno/?search=AppError
 [__link2]: https://docs.rs/ohno/0.4.0/ohno/?search=ErrorExt
 [__link20]: https://docs.rs/ohno/0.4.0/ohno/?search=AppError
 [__link21]: https://docs.rs/ohno/0.4.0/ohno/?search=ErrorLabel
 [__link22]: https://docs.rs/ohno/0.4.0/ohno/?search=ErrorLabel::from_error_chain
 [__link23]: https://doc.rust-lang.org/stable/std/?search=error::Error::source
 [__link24]: https://docs.rs/ohno/0.4.0/ohno/?search=ErrorLabel
 [__link25]: https://docs.rs/ohno/0.4.0/ohno/?search=Labeled
 [__link26]: https://docs.rs/ohno/0.4.0/ohno/?search=Labeled::label
 [__link3]: https://docs.rs/ohno/0.4.0/ohno/?search=OhnoCore
 [__link4]: https://docs.rs/ohno/0.4.0/ohno/?search=AppError
 [__link5]: https://docs.rs/ohno/0.4.0/ohno/?search=OhnoCore
 [__link6]: https://doc.rust-lang.org/stable/std/?search=error::Error
 [__link7]: https://doc.rust-lang.org/stable/std/?search=fmt::Display
 [__link8]: https://doc.rust-lang.org/stable/std/?search=fmt::Debug
 [__link9]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
