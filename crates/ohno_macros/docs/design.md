# Design

The shape `ohno_macros` is being rewritten into. This is a proposal; nothing
here is implemented yet.

## The problem with the previous shape

The old crate mixed the three things a proc macro does. `impl_error_derive`
walked the `DeriveInput` seven times, once per generated item, and every
generator re-derived facts the previous one had already worked out — which field
holds the core, whether the struct is named or tuple, what the generics split
into. Each of those re-derivations could fail, so every generator returned
`Result`, and the same "expected named field for named struct" impossibility was
re-checked in `constructors.rs` and again in `from_impls.rs`. Validation was
scattered across the file that happened to need it first, and no single value
ever held "what the user asked for".

## The pipeline

Three phases, in order, each with one job.

```text
TokenStream ──parse──▶ Ast ──validate──▶ Model ──generate──▶ TokenStream
              syntax           rules              rendering
```

**parse** turns tokens into `Ast`, a faithful record of what the user wrote. It
fails only on input that is not syntactically an error type: a `#[display]` with
no template, a `#[from = "x"]`. It judges nothing.

**validate** turns `Ast` into `Model` by applying the rules in
`requirements.md`. This is where every diagnostic that says "you cannot do that"
lives — one field marked twice, a template naming a field that does not exist,
an argument prefixed with `self.`. It is the only phase that reports rule
violations.

**generate** turns `Model` into tokens. It returns `TokenStream`, not
`Result<TokenStream>`. It cannot fail, because a `Model` that could make it fail
cannot be constructed.

That last point is the design. The two types are what makes it structural rather
than a convention: `generate` has no access to `syn::Attribute`, no lookup that
can miss, and no branch for a case validation already ruled out.

## Modules

```text
src/
  lib.rs              three proc-macro entry points, nothing else
  diagnostics.rs      Errors accumulator, bail!/bail_spanned!
  paths.rs            the `ohno::` paths generated code refers to
  marker.rs           the reserved doc marker and its nonce

  derive_error/
    mod.rs            parse -> validate -> generate, and nothing more
    ast.rs            Ast: what the struct says
    model.rs          Model: what will be generated
    parse.rs          Ast::parse
    validate.rs       Ast -> Model
    display.rs        template -> Message, its own parse/validate pair
    generate/
      mod.rs          Model -> TokenStream, one call per item
      constructors.rs
      conversions.rs  From<T>, From<Infallible>
      traits.rs       Display, Error, Enrichable, ErrorExt, Debug

  error_attr/
    mod.rs            reject -> inject core -> re-emit with the derive
  enrich_err/
    mod.rs            parse -> validate -> generate
    ast.rs
    model.rs
```

`error_attr` keeps no `Model`: it rewrites its input and hands the result to the
derive, so its whole job is three rejections and one field injection.

## The types

`Model` is the interesting one. It holds resolved values, not syntax:

```rust
/// A validated error type, ready to generate from
struct Model {
    name: Ident,
    generics: Generics,
    /// How the core is reached, and how every other field is named
    shape: Shape,
    /// The message, already lowered to a format string and its arguments
    message: Option<Message>,
    conversions: Vec<Conversion>,
    debug: bool,
    constructors: bool,
}

/// The fields, in declaration order, with the core singled out
enum Shape {
    Named { core: Ident, data: Vec<NamedField> },
    Tuple { core: Index, data: Vec<TupleField> },
}
```

`Shape` is why the old "expected named field for named struct" checks disappear.
The old code carried the field kind and the core reference as two independent
values and had to re-check that they agreed at every use. Here they are one
value, so the disagreement is unrepresentable and the generators match on
`Shape` once.

`Message` is likewise fully lowered:

```rust
struct Message {
    /// A `format!` string with every placeholder already normalized to `{}`
    template: String,
    /// One expression per placeholder, already scoped to `self`
    arguments: Vec<TokenStream>,
}
```

By the time it exists, every field it names has been checked to exist and every
argument has been checked to be rooted in a field. Rendering it is one `quote!`.

## Diagnostics

The old code returned on the first problem, so a struct with three bad
placeholders took three compile cycles to fix. The rewrite accumulates:
`validate` collects into a `syn::Error` built with `combine`, and reports them
together.

Two rules carry over unchanged, because `crates/ohno/tests/ui/*.stderr` pins
them:

- a diagnostic covering more than one token is built with
  `syn::Error::new_spanned`, never from a node span;
- a diagnostic points at what the user wrote, never at generated code.

## Testing

The split gives each phase a test style that fits it, and none of them needs the
proc-macro bridge:

| Phase | Test |
| --- | --- |
| parse | `parse_quote!` in, `Ast` asserted |
| validate | `Ast` in, `Model` or the exact diagnostic asserted |
| generate | hand-built `Model` in, `insta` snapshot of the pretty-printed tokens |

Generator snapshots stop depending on the parser, so a change to attribute
syntax no longer churns the expansion snapshots, and a `Model` that never comes
out of a real parse can still be covered.

Above all of it, `crates/ohno/tests/**` stays the spec: it compiles the macros
for real and is the only thing that proves the generated code works.

## Open questions

1. **One `Ast` type or two phases sharing `DeriveInput`?** The proposal parses
   into an owned `Ast`. The cheaper alternative is to let `validate` read
   `DeriveInput` directly and drop `Ast`, which removes a type but puts
   `syn::Attribute` back in the validating code.
2. **Does `enrich_err` earn the full pipeline?** Its model is a message and a
   signature. Two phases may be enough.
3. **Accumulated diagnostics vs. the pinned `.stderr` files.** Reporting several
   errors at once changes the compile-fail snapshots that currently show one.
   The snapshots would be regenerated; worth confirming that is wanted.
4. **`pub(crate)` constructors.** Kept as-is by R1.4, and settled by ADO 7675155
   as designed. The rewrite is not the place to reopen it.
