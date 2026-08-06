# `#[error]` and the `OhnoCore` field

How `#[derive(ohno::Error)]` decides which field holds the `OhnoCore`, and how
`#[ohno::error]` interacts with that choice.

## The error field

Every generated error has exactly one *error field*: the field holding the
`OhnoCore`. `source()`, the backtrace, enrichment, and the `caused by:` part of
`Display` are all generated from it. Every other field is data.

## Designating the error field

There are two ways, tried in this order.

1. **An explicit `#[error]` marker** on a field.
2. **Auto-detection**, when no field is marked: the one field whose type names
   `OhnoCore` in its final path segment.

Auto-detection reads the spelling and resolves nothing, so `OhnoCore`,
`ohno::OhnoCore` and `crate::OhnoCore` are all found, while a core reached
through a type alias or an import rename is not. The marker is the only way to
designate one of those, which is why the marked field's type is deliberately
**not** checked: the marker is an explicit statement about a field, so the type
it names is left to `rustc` to resolve in the generated implementations.

```rust
type Core = ohno::OhnoCore;

#[derive(ohno::Error)]
#[display("failed for {path}")]
struct AliasedError {
    path: String,
    #[error]
    inner: Core,      // auto-detection would never find this
}
```

## `#[error]` grammar

The marker is a bare path. It takes no arguments in any form, so
`#[error(anything)]`, `#[error()]` and `#[error = "x"]` are all rejected rather
than ignored. At most one field may be marked, and a field may not carry the
marker twice.

Marking is optional. A struct with no marker and one `OhnoCore` field is the
ordinary case, and is what the crate's own examples use.

## `#[ohno::error]`

The attribute macro always adds the `OhnoCore` field itself and always generates
the error representation from the field it adds. Two consequences follow.

**No field may be marked with `#[error]`.** A marker asks for a different field
to be the error representation, which the attribute cannot honour. Use
`#[derive(ohno::Error)]` on its own to place the core by hand.

**A field of type `OhnoCore` the user declares is ordinary data.** It is passed
to the generated constructors like any other field, appears in the generated
`Debug`, and can be interpolated by a `#[display(...)]` template. It is never
read for `source()`, the backtrace, or enrichment.

```rust
#[ohno::error]
#[display("failed for {path}, carrying {carried}")]
struct DeclaredCoreError {
    path: String,
    carried: ohno::OhnoCore,   // data, not the error
}
```

Applied to a unit struct, the attribute rewrites it as a tuple struct holding
the injected field. `#[derive(ohno::Error)]` on its own rejects a unit struct,
because there is nowhere for the core to live.

## The injected field marker

`#[ohno::error]` marks the field it injects with a reserved doc string, so the
rest of the macro can tell it apart from a core the user declared. The marker is
internal and is never part of the `#[error]` grammar, which is what keeps that
grammar argument-free.

Unlike an attribute, a doc string cannot be rejected when a user writes it — it
can only fail to match. The marker therefore carries a nonce, so an ordinary doc
comment cannot collide with it. An `#[error]` marker in a struct that already
carries the injected field is reported, which also keeps the two markers
mutually exclusive: field lookup treats either as decisive and takes the first
match, and that is only sound because a struct with both never reaches it.

## Rejected inputs

| Input | Reported as |
| --- | --- |
| `#[error(...)]`, `#[error()]`, `#[error = "x"]` | `` `#[error]` takes no arguments `` |
| Two fields marked | Multiple fields marked with `` `#[error]` `` |
| One field marked twice | Duplicate `` `#[error]` `` on the same field |
| `#[error]` beside the injected field | `#[ohno::error]` already added the field holding the OhnoCore |
| `#[error]` under `#[ohno::error]` | `#[ohno::error]` adds the OhnoCore field itself |
| No marker and no `OhnoCore` field | No field marked with `` `#[error]` `` found |
| No marker and several `OhnoCore` fields | Multiple OhnoCore fields found |
| An enum, or a unit struct under the derive alone | The derive supports neither |

## Scope

The generated implementations refer to the crate by the path `ohno`. Exposing
the package under a different crate name is not supported.
