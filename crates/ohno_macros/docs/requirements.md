# Requirements

What `ohno_macros` has to deliver. Written before the rewrite, from the previous
implementation, the crate docs, and the tests in `crates/ohno`. It covers both
packages: `ohno_macros`, the proc-macro crate the compiler sees, and
`ohno_macros_impl`, the ordinary library holding the logic behind it. See the
"Two crates" section of `design.md` for why the split exists.

The authority for behavior is `crates/ohno/tests/**` (integration tests) and
`crates/ohno/tests/ui/**` (compile-fail snapshots): they are what states, and
pins, what the macros owe. The crate's own rustdoc examples compile expansions
too, but they illustrate the surface rather than constrain it. The expansion
snapshots in `crates/ohno_macros_impl/tests/public_api.rs` pin the shape of the
tokens the three public expansion functions emit, which is a regression net
rather than a statement of what the crate owes.

## Public surface

Three entry points, re-exported by `ohno` as `ohno::Error`, `ohno::enrich_err`
and `ohno::error`.

| Entry point | Kind | Helper attributes |
| --- | --- | --- |
| `Error` | derive | `error`, `display`, `no_constructors`, `no_debug`, `from` |
| `enrich_err` | attribute | none |
| `error` | attribute | none |

The helper attribute list is part of the surface: everything in it appears in
the derive's rustdoc, and anything not in it is not inert on a field.

Generated code names the crate as `ohno`. Renaming the package in `Cargo.toml`
is not supported.

## R1 — `#[derive(Error)]`

### R1.1 Input

Structs only. Named, tuple, and (through `#[ohno::error]`) unit structs that
have already been rewritten into tuple structs.

- An enum is rejected.
- A unit struct under the derive alone is rejected: there is no room for a core.

### R1.2 Picking the error field

Exactly one field holds the `OhnoCore`. It is found in this order:

1. A field carrying the reserved `#[doc]` marker that `#[ohno::error]` writes.
2. A field carrying `#[error]`.
3. The single field whose type path ends in `OhnoCore`.

Step 3 reads the last path segment and resolves nothing, so a core reached
through a type alias or a renamed import is invisible to it and has to be
marked. A marked field's type is never checked — `rustc` resolves it.

Rejected:

- `#[error]` with any argument: `#[error(x)]`, `#[error()]`, `#[error = "x"]`.
- Two fields marked, or one field marked twice.
- `#[error]` on a struct that already carries the generated marker.
- No marker and no `OhnoCore` field; no marker and several of them.

Full statement of the rules, including why the marker is a doc comment with a
nonce: `error_error.md`.

### R1.3 Generated items

For every accepted input:

| Item | Source |
| --- | --- |
| `Display` | the core, plus the `#[display(...)]` message when present |
| `std::error::Error` | `source()` delegates to the core |
| `ohno::Enrichable` | `add_enrichment` delegates to the core |
| `ohno::ErrorExt` | `message()` and `backtrace()` delegate to the core |
| `From<std::convert::Infallible>` | always; the body is `unreachable!` |
| `Debug` | unless `#[no_debug]`; prints every field, including the core |
| constructors | unless `#[no_constructors]` |
| `From<T>` | one per type listed in `#[from(...)]` |

All of them carry the input's generics, type generics, and where clause.

### R1.4 Constructors

Two associated functions, both `pub(crate)`:

- `new(...)` — one parameter per non-core field, each `impl Into<FieldType>`;
  the core is `OhnoCore::default()`.
- `caused_by(..., error)` — the same parameters plus a trailing
  `impl Into<Box<dyn std::error::Error + Send + Sync>>`; the core is
  `OhnoCore::from(error)`.

For a tuple struct the parameters are named `param_0`, `param_1`, … by field
index, skipping the core, and the field order of the struct is preserved.

`pub(crate)` is deliberate and documented: an error type that needs a public
constructor declares one by hand. `#[no_constructors]` suppresses both.

### R1.5 `#[display(...)]`

`#[display("template")]` or `#[display("template", arg1, arg2)]`. The template
is a `format!` string.

| Placeholder | Meaning |
| --- | --- |
| `{name}` | the field called `name` |
| `{0}` | tuple field 0 |
| `{name:spec}` | the field, with `spec` as the format spec |
| `{}` | the next positional argument |
| `{{`, `}}` | a literal brace |

Positional arguments are implicitly scoped to `self`: a field is written by its
bare name, and the argument's leftmost term is the one that has to name a field
or call a method of `self`.
The scoped argument is emitted as `&(self.<arg>)` — the parentheses are
load-bearing, so that `count as u64` casts the field rather than a reference to
it.

Referenceable fields are every field the user wrote; the field injected by
`#[ohno::error]` is excluded, a core the user declared is not. Raw identifiers
keep their `r#`.

Rejected, by the macro rather than by the expansion:

- A placeholder naming something that is not a field.
- `{}` with no argument left, or an argument no `{}` consumes.
- An argument written with a `self.` prefix.
- An argument rooted in anything other than a field or method of `self`.
- An unbalanced `{` or `}`.

Diagnostics point at what the user wrote, and any diagnostic spanning more than
one token is built with `syn::Error::new_spanned` so it renders identically on
every toolchain. The exact messages are pinned by `crates/ohno/tests/ui/*.stderr`.

Full statement, including the argument-rooting rules: `error_display.md`.

### R1.6 `#[from(...)]`

`#[from(Type1, Type2)]` generates one `From` per type. A type may carry field
expressions: `#[from(std::io::Error(kind: ErrorKind::Io, message: "…"))]`, keyed
by field name for a named struct and by index for a tuple struct.

The core field is built with `OhnoCore::from(error)`. Every other field takes
its expression if one was given and `Default::default()` otherwise.

Rejected: `#[from]`, `#[from()]`, `#[from = "…"]`, and a non-integer key for a
tuple field. Several `#[from(...)]` attributes on one struct accumulate.

### R1.7 `#[no_debug]` and `#[no_constructors]`

Bare markers on the struct. `#[no_debug]` suppresses the generated `Debug` so a
hand-written one can stand; without it, a manual `#[derive(Debug)]` collides.
`#[no_constructors]` suppresses `new` and `caused_by`, and is rejected under
`#[ohno::error]`.

## R2 — `#[ohno::error]`

Rewrites a struct, then applies `#[derive(ohno::Error)]` to it.

- Named struct: appends a field `ohno_core: ohno::OhnoCore`, renamed
  `ohno_core_1`, `ohno_core_2`, … on collision.
- Tuple struct: appends `ohno::OhnoCore` as the last field.
- Unit struct: rewritten as a tuple struct holding the core.
- Anything that is not a struct is rejected.

The added field carries the reserved doc marker, which is what makes it findable
in R1.2 without listing a helper attribute that would then show up in rustdoc.

The attribute runs before it adds anything, so it can tell a hand-written marker
from its own and rejects both `#[error]` on any field and a hand-written
reserved marker. Doc comments and other attributes on the struct survive.

## R3 — `#[enrich_err(...)]`

Wraps a function body so that an `Err` gains an enrichment entry carrying a
message, `file!()` and `line!()`.

Arguments:

| Form | Message |
| --- | --- |
| none | `"error in function <name>"` |
| `"text"` with no braces | the literal |
| `"text {field}"` | `format!` of the literal |
| `"text {}", expr, …` | `format!` of the literal and arguments |

The first token has to be a string literal; anything else is rejected. A
function with no return type is rejected.

The body is rewritten into an immediately-invoked closure so that `?` inside it
still returns from the closure, `.await`ed when the function is `async`. Every
part of the signature has to survive: visibility, `const`, `unsafe`, `extern`
ABI, generics, lifetimes, bounds, where clauses, `impl Trait` and `dyn Trait`
parameters, `self` receivers in every form, doc comments, and other attributes.
`const` survives faithfully enough that a `const fn` is then rejected by `rustc`
rather than by the macro, which `design.md` records under Limits.

## R4 — Diagnostics

A macro reports what it can rather than emitting code that fails to compile.
Errors reach the user as `compile_error!` at a span in their own source, never
as a panic and never as a `rustc` error pointing into generated code. A few
inputs survive from the implementation this replaces where that does not hold;
`design.md` records them under Limits.

Where an input breaks several rules, all of them are reported at once rather
than one per compile cycle.

Wherever a diagnostic covers more than one token it is spanned with
`syn::Error::new_spanned`.
