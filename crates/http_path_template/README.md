<div align="center">
 <img src="./logo.png" alt="Http Path Template Logo" width="96">

# Http Path Template

[![crate.io](https://img.shields.io/crates/v/http_path_template.svg)](https://crates.io/crates/http_path_template)
[![docs.rs](https://docs.rs/http_path_template/badge.svg)](https://docs.rs/http_path_template)
[![MSRV](https://img.shields.io/crates/msrv/http_path_template)](https://crates.io/crates/http_path_template)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A parser for the [`google.api.http`][__link0] path-template grammar.

A path template is the pattern that appears in a `google.api.http`
annotation, for example `shelves/{shelf}/books/{book=**}:archive`. This crate
turns such a string into a validated, structured [`PathTemplate`][__link1] — an
abstract syntax tree of [`Segment`][__link2]s (literals, `*`, `**`, and
`{field.path=sub-template}` [`Variable`][__link3] bindings) plus an optional custom
`:verb`.

The grammar mirrors the reference [`google.api.HttpRule`][__link4] path syntax:

* a **literal** segment (`shelves`) must match verbatim;
* **`*`** ([`Segment::Single`][__link5]) matches exactly one non-empty segment;
* **`**`** ([`Segment::Rest`][__link6]) matches the remaining segments and may only
  appear as the final element;
* **`{field.path=sub-template}`** ([`Segment::Variable`][__link7]) captures the portion
  of the path matched by its sub-template into a dotted message field; the
  shorthand `{field}` is `{field=*}` and nested variables are rejected;
* a trailing **`:verb`** declares a custom method verb.

This crate is purely a *parser*: it validates a template and exposes its
structure ([`PathTemplate::segments`][__link8] / [`PathTemplate::verb`][__link9]). It performs
no request matching and pulls in no dependencies. Build-time code generators
(such as `rest_over_grpc_build`) consume the parsed structure to emit a static
router.

## Examples

```rust
use http_path_template::{PathTemplate, Segment};

let template = PathTemplate::parse("/shelves/{shelf}/books/{book=**}:archive")?;
assert_eq!(template.verb(), Some("archive"));

let Segment::Variable(book) = &template.segments()[3] else {
    panic!("expected variable")
};
assert_eq!(book.field_path(), &[String::from("book")]);
assert_eq!(book.segments(), &[Segment::Rest]);
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_path_template">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbG7NXYJQe0W0bm6x1pnkTCmwbMqeiJQ6UkhsbIOYGUSc0EadhZIGCcmh0dHBfcGF0aF90ZW1wbGF0ZWUwLjEuMA
 [__link0]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link1]: https://docs.rs/http_path_template/0.1.0/http_path_template/struct.PathTemplate.html
 [__link2]: https://docs.rs/http_path_template/0.1.0/http_path_template/enum.Segment.html
 [__link3]: https://docs.rs/http_path_template/0.1.0/http_path_template/struct.Variable.html
 [__link4]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link5]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Single
 [__link6]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Rest
 [__link7]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Variable
 [__link8]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate::segments
 [__link9]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate::verb
