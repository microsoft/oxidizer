<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Routerama Build Logo" width="96">

# Routerama Build

[![crate.io](https://img.shields.io/crates/v/routerama_build.svg)](https://crates.io/crates/routerama_build)
[![docs.rs](https://docs.rs/routerama_build/badge.svg)](https://docs.rs/routerama_build)
[![MSRV](https://img.shields.io/crates/msrv/routerama_build)](https://crates.io/crates/routerama_build)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Static resolver code generation for [`routerama`][__link0].

[`Route`][__link1] stores validated path templates and their generated variant names.
[`Generator`][__link2]
collects routes and emits a resolver as a
[`proc_macro2::TokenStream`][__link3].
This API is intended for build scripts and
procedural-macro implementations; applications normally use
`routerama::resolver` instead.

Disable the default `codegen` feature when only the hidden, framework-neutral
routing trie is required at run time.

## Examples

```rust
use http_path_template::{Grammar, PathTemplate};
use routerama_build::{Generator, Route};

let mut generator = Generator::new("Route", true);
generator.add(Route::new(
    "GetBook",
    "GET",
    PathTemplate::parse("/books/{book}", Grammar::default())?,
));

let generated = generator.generate().to_string();
assert!(generated.contains("GetBook"));
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama_build">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbhew8pv6r7HAblYEJpTRkXpAbADOjUkxD6robydXdnodrq0xhZIGCb3JvdXRlcmFtYV9idWlsZGUwLjEuMA
 [__link0]: https://docs.rs/routerama
 [__link1]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=Route
 [__link2]: https://docs.rs/routerama_build/latest/routerama_build/?search=Generator
 [__link3]: https://docs.rs/proc-macro2/latest/proc_macro2/struct.TokenStream.html
