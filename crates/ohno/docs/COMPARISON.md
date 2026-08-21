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

**With a source**, the source's message is printed as it stands — no type name,
and no `caused by:` line. This is what `#[error(transparent)]` is written for in
`thiserror`, except that it is the default here rather than an opt-in, and it
carries none of that attribute's restrictions: the type may hold other fields,
and no attribute has to be spelled to get it.

**Without a source**, the type's own name is printed, so a caller sees
`Error: ConfigError` rather than a message. `thiserror` has no equivalent,
because it will not compile a type that declares no message.

The two defaults sit one source apart, which is worth knowing when porting a
`#[error(transparent)]` wrapper: in `thiserror` such a wrapper cannot exist
without its single inner error, while the `ohno` equivalent can be constructed
empty, and then renders as its own bare name.

See "How error text is rendered" in the crate documentation for the full account
with examples.

## A string cause is not a source

`ohno` constructors accept either an error value or a string. Both render
identically, but only an error value joins the chain — for a string cause,
`source()` returns `None`. `thiserror` has no counterpart, since a source there
is always a typed field.

## `Display` carries more than the message

In `thiserror`, `format!("{e}")` is the message and nothing else. In `ohno` it is
the message, then any enrichment entries added along the way, then the backtrace
if one was captured:

```text
no such file: /etc/app.toml
> failed to load the service configuration (at src/config.rs:6)

Backtrace:
   0: std::backtrace::Backtrace::capture
   ...
```

Code that logs `{e}` and expects a single line therefore needs review when
porting. `ErrorExt::message()` returns the message on its own.

Because every `ohno` error owns its own `OhnoCore`, and every core renders its
own backtrace, a chain of wrappers prints the message once and one backtrace
block per level.

## Structs only

`ohno` derives on structs; an enum is rejected. A `thiserror` enum ports to one
struct per variant, or to a struct holding an enum field that the `#[display]`
template reads.
