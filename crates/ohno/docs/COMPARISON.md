<!-- Copyright (c) Microsoft Corporation. -->
<!-- Licensed under the MIT License. -->

# Coming from `thiserror`

A translation guide for readers who know `thiserror` and are reading `ohno` for
the first time. The crate documentation stands on its own and does not assume
this file; everything here is a mapping, not a prerequisite.

The two crates agree on the shape of the problem — derive the error traits,
declare the message next to the type — so most of the move is mechanical. The
part that is not mechanical is what an error renders when no message is
declared, which is where the defaults differ most.

## Attribute mapping

| `thiserror` | `ohno` |
| --- | --- |
| `#[derive(Error)]` | `#[derive(ohno::Error)]`, or `#[ohno::error]` to have the core field inserted |
| `#[error("...")]` | `#[display("...")]` |
| `#[error(transparent)]` | the default: omit `#[display]` on a type that has a source |
| `#[source]`, or a field named `source` | the `OhnoCore` field, marked `#[error]` or inserted by `#[ohno::error]` |
| `#[from]` on a field | `#[from(Type1, Type2, ...)]` on the type |
| `#[error("{0}")]`, `#[error("{path}")]` | the same placeholders |
| `#[error("{}", self.path.display())]` | `#[display("{}", path.display())]` — the `self.` prefix is implicit, and rejected if written |

## The message is optional

`thiserror` requires a display attribute on every type and variant, and refuses
to compile without one. `ohno` treats it as an override: a type with no
`#[display]` still implements `Display`, and renders one of two defaults.

**With a cause**, the cause's message is printed as it stands — no type name,
and no `caused by:` line. This is what `#[error(transparent)]` is written for in
`thiserror`, except that it is the default here rather than an opt-in, and it
carries none of that attribute's restrictions: the type may hold other fields,
and no attribute has to be spelled to get it.

**Without a cause**, the type's own name is printed, so a log line reads
`ConfigError` rather than a message. (`Error: ` in front of it, as `main`
printing a `Result` produces, comes from the caller — `ohno` never writes a
prefix.) `thiserror` has no equivalent, because it will not compile a type that
declares no message.

The two defaults sit one cause apart, which is worth knowing when porting a
`#[error(transparent)]` wrapper: in `thiserror` such a wrapper cannot exist
without its single inner error, while the `ohno` equivalent can be constructed
empty, and then renders as its own bare name.

See [How error text is rendered](../README.md#how-error-text-is-rendered) for the
full account with examples.

## A string cause is not a source

`ohno` constructors accept either an error value or a string. Both render
identically, but only an error value joins the chain — for a string cause,
`source()` returns `None`. That is why the defaults above are stated in terms of
a *cause* rather than a *source*: `source()` does not predict what is rendered.
`thiserror` has no counterpart, since a source there is always a typed field.

## `Display` carries more than the message

In `thiserror`, `format!("{e}")` is the message and nothing else. In `ohno` it is,
for each level of the chain: the message, then that level's enrichment entries,
then that level's backtrace if one was captured:

```text
no such file: /etc/app.toml
> failed to load the service configuration (at src/config.rs:6)

Backtrace:
   0: std::backtrace::Backtrace::capture
   ...
```

Code that logs `{e}` and expects a single line therefore needs review when
porting. `ErrorExt::message()` drops this level's own enrichment and backtrace,
but it is not a single-line guarantee: with a `#[display]` template and a cause
it returns `<message>\ncaused by: <cause>`, and on a transparent wrapper it
renders the cause's full `Display` — which brings that level's enrichment and
backtrace back with it.

Because every `ohno` error owns its own `OhnoCore`, and every core renders its
own backtrace, a chain of wrappers prints the message once and one backtrace
block per level. The ordering above holds *per level*, not across the chain: the
levels are written innermost first, so a wrapper's enrichment entries appear
after the inner level's `Backtrace:` block rather than grouped with the inner
level's entries.

## Structs only

`ohno` derives on structs; an enum is rejected. A `thiserror` enum ports to one
struct per variant, or to a struct holding an enum field that the `#[display]`
template reads. The first form is the one to reach for: it keeps each failure
condition a separate type, which
[M-ERRORS-CANONICAL-STRUCTS](https://microsoft.github.io/rust-guidelines/guidelines/libs/ux/index.html#M-ERRORS-CANONICAL-STRUCTS)
prefers over an exposed kind enum. See
[`examples/error_enum_replacement.rs`](../examples/error_enum_replacement.rs) for
a six-variant enum ported that way.
