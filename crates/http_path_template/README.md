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
annotation, for example `/shelves/{shelf}/books/{book=**}:archive`. This crate
turns such a string into a validated, structured [`PathTemplate`][__link1] — an
abstract syntax tree of [`Segment`][__link2]s (literals, `*`, `**`, and
`{field.path=sub-template}` [`Variable`][__link3] bindings) plus an optional custom
`:verb`.

Parsing is zero-copy: the returned [`PathTemplate`][__link4] borrows from the input
string (every literal, field name, and verb is a slice into it), so a parse
copies no text and allocates only the top-level segment list.

A template must begin with `/` and is a `/`-separated sequence of segments —
each `/` delimits one segment. Literal segments, variable field names, and the
custom `:verb` are preserved and compared **verbatim**, so the grammar is
case-sensitive; the parser performs no case folding.

The grammar mirrors the reference [`google.api.HttpRule`][__link5] path syntax:

* a **literal** segment (`shelves`) must match verbatim;
* **`*`** ([`Segment::Single`][__link6]) matches exactly one non-empty segment;
* **`**`** ([`Segment::Rest`][__link7]) matches the remaining segments and may only
  appear as the final element;
* **`{field.path=sub-template}`** ([`Segment::Variable`][__link8]) captures the portion
  of the path matched by its sub-template into a dotted message field; the
  shorthand `{field}` is `{field=*}` and nested variables are rejected;
* a trailing **`:verb`** declares a custom method verb.

## Extended grammar

[`PathTemplate::parse`][__link9] takes a [`Grammar`][__link10] argument. The default grammar is
the strict `google.api.http` syntax above; passing a [`Grammar`][__link11] with
[`Grammar::with_segment_affixes`][__link12] enabled additionally allows **intra-segment
prefix/suffix parameters**: a single segment may wrap one `{field.path}`
variable in literal text, for example `/files/{name}.json`, `/v{version}/x`,
or `/img-{id}.png`. Such a segment parses to a [`Segment::Affix`][__link13]. The strict
grammar rejects this syntax.

## Examples

Parsing `/shelves/{shelf}/books/{book=**}:archive` yields four top-level
[`Segment`][__link14]s plus the custom verb `archive`:

* `shelves` — a [`Segment::Literal`][__link15];
* `{shelf}` — a [`Segment::Variable`][__link16] binding field `shelf` to a single
  segment (`*`, i.e. [`Segment::Single`][__link17]);
* `books` — a [`Segment::Literal`][__link18];
* `{book=**}` — a [`Segment::Variable`][__link19] binding field `book` to the remaining
  segments (`**`, i.e. [`Segment::Rest`][__link20]).

```rust
use http_path_template::{Grammar, PathTemplate, Segment};

let template = PathTemplate::parse(
    "/shelves/{shelf}/books/{book=**}:archive",
    Grammar::default(),
)?;

assert_eq!(template.segments().len(), 4);
assert_eq!(template.verb(), Some("archive"));

assert_eq!(template.segments()[0], Segment::Literal("shelves"));
assert_eq!(template.segments()[2], Segment::Literal("books"));

let Segment::Variable(shelf) = template.segments()[1] else {
    panic!("expected variable")
};
assert_eq!(shelf.field_path(), "shelf");
assert!(shelf.segments().eq([Segment::Single]));

let Segment::Variable(book) = template.segments()[3] else {
    panic!("expected variable")
};
assert_eq!(book.field_path(), "book");
assert!(book.segments().eq([Segment::Rest]));
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_path_template">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbdehWW1wo3-MbQZ4f-tMBwgsbFQlyCp62fwcbNkxL2v1MviZhZIGCcmh0dHBfcGF0aF90ZW1wbGF0ZWUwLjEuMA
 [__link0]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link1]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate
 [__link10]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Grammar
 [__link11]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Grammar
 [__link12]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Grammar::with_segment_affixes
 [__link13]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Affix
 [__link14]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment
 [__link15]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Literal
 [__link16]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Variable
 [__link17]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Single
 [__link18]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Literal
 [__link19]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Variable
 [__link2]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment
 [__link20]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Rest
 [__link3]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Variable
 [__link4]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate
 [__link5]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link6]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Single
 [__link7]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Rest
 [__link8]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=Segment::Variable
 [__link9]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate::parse
