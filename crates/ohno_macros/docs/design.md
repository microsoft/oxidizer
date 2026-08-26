# Design

How the `ohno` macros are built. `requirements.md` says what they have to
deliver; this document says how they are arranged to deliver it. A requirement is
named wherever a rule below implements one.

The implementation spans two packages — `ohno_macros`, the proc-macro crate the
compiler sees, and `ohno_macros_impl`, the ordinary library behind it. The Two
crates section describes the split; until then, read "the crate" as both.

## The pipeline

The derive runs three phases, in order, each with one job.

```text
TokenStream ──parse──▶ Ast ──validate──▶ Model ──generate──▶ TokenStream
              syntax           rules              rendering
```

**parse** turns tokens into `Ast`, a record of what the user wrote with the
crate's own attributes decoded. It answers one question per attribute: *can I
read this?* A `#[display]` with no template and a `#[from = "x"]` fail here. It
judges nothing else — an unknown field name and a marked-twice field are both
readable, so both pass parse.

**validate** turns `Ast` into `Model` by applying the rules in
`requirements.md`. It answers the other question: *is what you wrote allowed?*
Every R1 rejection that is not a decoding failure is reported here. R2 belongs
to `#[ohno::error]` and R3 to `#[enrich_err]`, so each is applied by its own
entry point.

**generate** turns `Model` into tokens. It returns `TokenStream`, not
`Result<TokenStream>`, and cannot fail: a `Model` that would make it fail cannot
be built.

R4 forbids a diagnostic that points into generated code, so no fault survives
into `generate`. It has no access to `syn::Attribute`, no lookup that can miss,
and no branch for a case `validate` already ruled out.

`Ast` and `Model` carry different halves of the input. `Ast` holds decoded
attribute payloads together with the spans they came from, which is what R4's
diagnostics point at. `Model` holds resolved values and drops spans, so a
`generate` that reads only `Model` cannot anchor anything. `Ast` re-houses
neither `Generics` nor `Fields`: `Generics` passes through by value and the field
list is reduced to what the rules need, so the decoded form of the five helper
attributes is all `Ast` adds over `syn::DeriveInput`. It is consumed by exactly
one function, `validate`, in exactly one module.

The two phases are also what make the "skipped, not guessed" rule in the
Diagnostics section expressible. An attribute that fails to decode is reported by
parse and is *absent from `Ast`*, so validate has nothing to check there and
reports only faults it can see.

## Two crates

A crate with `proc-macro = true` can export nothing but macros, and its own tests
cannot call them: a `proc_macro::TokenStream` exists only inside a real
expansion. The logic therefore lives in an ordinary library, and the proc-macro
crate holds the entry points alone.

```text
crates/
  ohno_macros/          the proc-macro crate; `proc-macro = true`
    src/lib.rs          the three entry points, and nothing else
    docs/               this document and `requirements.md`

  ohno_macros_impl/     an ordinary library; everything below lives here
    src/...             the modules listed in the next section
    tests/public_api.rs the expansion snapshots (see Testing)
```

`ohno_macros_impl` exposes the three expansions as ordinary functions over
`proc_macro2::TokenStream`. Each entry point in `ohno_macros` converts the
token-stream type, delegates, and converts back:

```rust
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    ohno_macros_impl::derive_error(input.into()).into()
}
```

Nothing else lives in a shim, so the compiler-facing crate holds no branch a test
cannot reach.

`ohno_macros_impl` is not a public API. Its rustdoc says so, and `ohno` depends
on `ohno_macros` rather than on it. Only its three expansion functions are `pub`;
every module below them is private.

These documents sit under `ohno_macros/docs/` because they describe the macros
`ohno` re-exports. Read "the crate" below as "the two crates together" wherever
the distinction does not matter.

## Modules

Everything in this section lives in `ohno_macros_impl/src/`.

```text
lib.rs              `derive_error`, `enrich_err`, `error`: parse the input item
                    and dispatch; the crate's whole public surface
diagnostics.rs      the `Errors` accumulator and its `syn::Error` combining
message.rs          `Message`: a lowered `format!` call, built by two macros
paths.rs            the `::ohno::` paths generated code refers to
marker.rs           the reserved doc marker, its nonce, and its recognizer

derive_error/
  mod.rs            parse -> validate -> generate, and nothing more
  ast.rs            `Ast`: the input with the crate's attributes decoded
  parse.rs          `DeriveInput` -> `Ast`
  model.rs          `Model`, `Shape`, `Conversion`, and their constructors
  validate.rs       `Ast` -> `Model`; every rule in R1
  display/
    mod.rs          `#[display(...)]` -> `Message`
    template.rs     splitting a template into literal and placeholder segments
    argument.rs     rooting a positional argument in a field of `self`
  generate/
    mod.rs          `Model` -> `TokenStream`, one call per item in R1.3
    traits.rs       `Display`, `Error`, `Enrichable`, `ErrorExt`, `Debug`
    constructors.rs `new` and `caused_by`
    conversions.rs  `From<T>` and `From<Infallible>`

error_attr/
  mod.rs            reject, inject the core, re-emit under the derive
enrich_err/
  mod.rs            message, signature, and the body rewrite
```

`lib.rs` owns the one step the shim cannot do for it: turning the incoming tokens
into a `syn::DeriveInput` or `syn::Item` and reporting the parse failure as a
compile error. Everything past that point is a module above.

Only the derive carries the full pipeline.

`error_attr` keeps no `Model`. R2 gives it three rejections and one field
injection, and it hands its output to the derive, which validates the result
again.

`enrich_err` keeps no `Ast`. R3 gives it a message and a signature, and the
signature is re-emitted rather than read, so decoding and checking are one step
that yields a `Message`.

`message.rs` sits at the crate root rather than under `derive_error` because
both macros end at the same place — a `format!` string and a list of argument
expressions — even though they reach it differently. The derive lowers field
names into `self`-scoped accesses (R1.5); `enrich_err` passes the literal and
the arguments through unchanged, because its placeholders name function
parameters that `rustc` resolves and its arguments are ordinary expressions in
the function's own scope (R3).

## The types

### `Ast`

```rust
/// What the struct says, with the crate's own attributes decoded
struct Ast {
    ident: Ident,
    generics: Generics,
    style: Style,
    fields: Vec<AstField>,
    /// `None` when absent or when it failed to decode
    display: Option<DisplayAttr>,
    /// One per `#[from(...)]`; several accumulate (R1.6)
    conversions: Vec<FromAttr>,
    /// Whether `#[no_debug]` was written
    no_debug: bool,
    /// Whether `#[no_constructors]` was written
    no_constructors: bool,
}

/// Whether fields are named or positional. A unit struct never reaches `Ast`:
/// R1.1 rejects it in parse, because it has no room for a core.
enum Style {
    Named,
    Tuple,
}

struct AstField {
    /// `Named(Ident)` or `Unnamed(Index)`, ready to quote as `self.#member`
    member: Member,
    ty: Type,
    /// Every hand-written `#[error]` on this field, in order. A `Vec` rather
    /// than an `Option`, so "marked twice" is representable and therefore
    /// reportable (R1.2). The attribute is kept whole, so a diagnostic points
    /// at the marker the user wrote.
    marks: Vec<Attribute>,
    /// Whether the field carries the reserved doc marker `#[ohno::error]`
    /// writes. Separate from `marks`, because a generated marker and a
    /// hand-written one are different inputs and R1.2 treats them differently.
    generated: bool,
}

struct DisplayAttr {
    /// The template literal, kept whole so diagnostics can point at it
    template: LitStr,
    arguments: Vec<Expr>,
}

struct FromAttr {
    source: Type,
    /// The field expressions, keyed as the user wrote them. Whether a key names
    /// a field is a rule, so it is checked in validate, not here.
    overrides: Vec<FromOverride>,
}
```

### `Model`

```rust
/// A validated error type, ready to generate from
struct Model {
    ident: Ident,
    generics: Generics,
    shape: Shape,
    /// The `#[display(...)]` message, already lowered (R1.5)
    message: Option<Message>,
    conversions: Vec<Conversion>,
    /// R1.7
    debug: bool,
    /// R1.4, R1.7
    constructors: bool,
}
```

`Shape` is the value that makes "exactly one core" structural rather than
checked:

```rust
/// The fields in declaration order, split around the one holding the core
struct Shape {
    style: Style,
    /// Fields declared before the core
    before: Vec<ModelField>,
    /// The field holding the `OhnoCore`. Exactly one, always present
    core: ModelField,
    /// Fields declared after the core
    after: Vec<ModelField>,
}

impl Shape {
    /// Splits `fields` around `core`.
    ///
    /// # Panics
    ///
    /// Panics when `core` does not index `fields`. `validate` derives the index
    /// from the very field list it then maps into `fields`, so the two cannot
    /// disagree
    fn new(fields: Vec<ModelField>, core: usize, style: Style) -> Self;
    /// The field holding the core
    fn core(&self) -> &ModelField;
    /// Every field, in declaration order. What `Debug` prints (R1.3)
    fn all(&self) -> impl Iterator<Item = &ModelField>;
    /// Every field but the core, in declaration order. What constructors take
    /// and what a conversion initializes (R1.4, R1.6)
    fn data(&self) -> impl Iterator<Item = &ModelField>;
}

struct ModelField {
    /// How the field is written in an expression: `path`, or `0`
    member: Member,
    /// How the field is bound as a constructor parameter: `path`, or `param_0`
    /// for a tuple field, by index and skipping the core (R1.4)
    binding: Ident,
    ty: Type,
}
```

Splitting around the core rather than carrying an index into one list removes a
class of check. An index can dangle, so a generator using one would have to
handle a core that is not there; `before`/`core`/`after` cannot express that, and
declaration order is still recoverable from it. The split also removes the "named
struct with a positional core" case: `Style` and `member` are read from the same
value, so they cannot disagree.

`Shape::new` is infallible, and an out-of-range `core` panics. The index is not
user input by the time it arrives: `validate` finds the core in `ast.fields` and
maps that same list, one for one, into the `ModelField`s it passes alongside the
index. A disagreement between the two is a fault in `validate`, not an input the
author can act on, so there is no diagnostic to report and none is invented — R4
forbids one that points into generated code. The panic is documented on the
constructor and named in the `expect` that raises it.

`Member` also removes most of the named-versus-tuple branching from `generate`.
`self.#member` is `self.path` or `self.0`, so every read of a field takes one
form. `Style` is consulted by exactly two items: `Debug`, which needs
`debug_struct` for a named struct and `debug_tuple` for a tuple one (R1.3), and
`construct`, which emits `Self { .. }` or `Self(..)`.

### `Message`

```rust
/// A lowered `format!` call
enum Message {
    /// Rendered as a string literal, with `{{` and `}}` escapes already resolved
    Literal(String),
    /// Rendered as `format!(template, arguments...)`
    Formatted {
        /// A `format!` string. Every placeholder that named a field has become
        /// positional; a format spec is kept, so `{name:?}` becomes `{:?}`
        template: String,
        /// One expression per placeholder that consumes an argument, in order
        arguments: Vec<TokenStream>,
    },
}

impl Message {
    /// A `&'static str` or a `String`; both satisfy the `Into<Cow<_, str>>`
    /// bound the runtime asks for
    fn render(&self) -> TokenStream;
}
```

A message with no arguments is rendered as a literal rather than as a
one-argument `format!`, so a static `#[display("...")]` costs no allocation at
run time.

By the time a `Message` built by the derive reaches `generate`, every field it
names exists and every argument is rooted in a field or a method of `self`
(R1.5) — the latter because
`expand` emits nothing but diagnostics once a fault was recorded, not because
`Message` could not hold such an argument. Each argument is already wrapped as
`&(self.<arg>)` — the parentheses being load-bearing, so `count as u64` casts
the field rather than a reference to it. Rendering it is one `quote!`.

### `Conversion`

```rust
/// One `From<T>` from a `#[from(...)]` entry (R1.6)
struct Conversion {
    source: Type,
    /// One initializer per non-core field, aligned with `Shape::data()`.
    /// Built by `Conversion::new`, which fills a field the user did not name
    /// with `Default::default()`
    initializers: Vec<Expr>,
}
```

### Where the invariants live

Two of the three invariants `generate` relies on are structural: `Shape` makes
"exactly one core" unrepresentable, and `Message` names only fields that exist,
because the only way to obtain either type is through validation.

An argument that is not rooted in a field is procedural rather than structural.
Lowering records the fault and still returns the `Message`, so a bad template and
a bad argument are reported together; the argument is kept out of generated code
by `expand`, which emits the diagnostics alone whenever any fault was recorded.

The third is procedural too. "`Conversion::initializers` is as long as
`Shape::data()`" is a relation between two values, which Rust cannot hold in a
struct shape without an encoding heavier than the check it replaces. A module
boundary holds it instead: `initializers` is private to `model.rs`, and the only
constructor is

```rust
impl Conversion {
    /// Distributes `overrides` over `shape.data()`, defaulting the rest.
    /// Reports an override whose key names no non-core field
    fn new(shape: &Shape, source: Type, overrides: &[(Member, Expr)], errors: &mut Errors) -> Option<Self>;
}
```

so the alignment is established once, where the field list is in hand, rather
than at each use.

## Generating the items in R1.3

`generate::mod` calls one function per row of the R1.3 table and concatenates
the results. Generics thread through identically everywhere:

```rust
let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
```

Every generated `impl` is then
`impl #impl_generics Trait for #ident #ty_generics #where_clause`, and the
constructors go in one inherent
`impl #impl_generics #ident #ty_generics #where_clause`. This covers lifetimes,
type parameters and where clauses in one rule, which is what R1.3's "all of them
carry the input's generics" asks for.

Below, `#core` is `model.shape.core.member` and `#name` is the type's name as a
string literal — the default message the runtime falls back to when nothing else
renders.

**`Display`** delegates to the core, passing the lowered message as an override
when there is one. `OhnoCore::format_error` is what appends `caused by:`, the
enrichment lines, and the backtrace, so the generated code decides only the
message:

```rust
impl #impl_generics ::core::fmt::Display for #ident #ty_generics #where_clause {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        self.#core.format_error(f, #name, #override_message)
    }
}
```

`#override_message` is `::core::option::Option::None` when `model.message` is
`None`, and `::core::option::Option::Some(::std::borrow::Cow::from(..))`
otherwise. `Cow::from` takes both a `&'static str` and a `String`, so a static
`#[display("...")]` renders as a string literal and allocates nothing, while a
formatted one renders as a `format!` call — without the generator branching on
which it is.

**`std::error::Error`** delegates `source`:

```rust
fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
    self.#core.source()
}
```

**`ohno::Enrichable`** delegates `add_enrichment` to the core, which is the
trait `OhnoCore` itself implements. **`ohno::ErrorExt`** delegates `backtrace`
to `self.#core.backtrace()` and `message` to
`self.#core.format_message(#name, #override_message)` — the same override the
`Display` impl builds, which is why the two agree by construction rather than by
being written twice.

**`From<Infallible>`** is emitted unconditionally with an `unreachable!` body,
so a fallible conversion in user code can be written once and stay correct when
the error type becomes infallible.

**`Debug`** is emitted unless `model.debug` is false. It prints every field
including the core, so it iterates `shape.all()` and branches once on
`shape.style`: `f.debug_struct(#name).field("path", &self.path)…` or
`f.debug_tuple(#name).field(&self.0)…`.

It is the one generated item that is **not** `#[automatically_derived]`.
Dead-code analysis ignores field reads inside a derived `Debug`, so marking this
one would make every field that only `Debug` reads look unused in the user's own
crate.

**Constructors** are emitted unless `model.constructors` is false. Both iterate
`shape.data()`, so the core is skipped and declaration order is kept:

```rust
pub(crate) fn new(#(#binding: impl ::core::convert::Into<#ty>),*) -> Self {
    Self { #(#member: #binding.into(),)* #core: ::ohno::OhnoCore::default() }
}

pub(crate) fn caused_by(
    #(#binding: impl ::core::convert::Into<#ty>,)*
    error: impl ::core::convert::Into<
        ::std::boxed::Box<dyn ::std::error::Error + Send + Sync>,
    >,
) -> Self {
    Self { #(#member: #binding.into(),)* #core: ::ohno::OhnoCore::from(error) }
}
```

`pub(crate)` is fixed by R1.4, settled by ADO 7675155.

**`From<T>`** is emitted once per `Conversion`, zipping `shape.data()` with
`conversion.initializers()`, and building the core with `OhnoCore::from(error)`.
The binding is named `error`, which is the name a field expression refers to.
The data initializers are evaluated into one tuple first, because they may
borrow `error` while the core consumes it, and a struct literal evaluates its
fields in the order they are written:

```rust
impl #impl_generics ::core::convert::From<#source>
    for #ident #ty_generics #where_clause
{
    fn from(error: #source) -> Self {
        let __ohno_fields = (#(#initializer,)*);
        Self { #(#member: __ohno_fields.#index,)* #core: ::ohno::OhnoCore::from(error) }
    }
}
```

One tuple, not one local per field. A generated name is not hygienic, so a local
per field would be in scope while the next initializer is evaluated, and an
initializer naming an outer item the derive happened to shadow would read the
generated local instead — silently, or as an error in generated code when the
outer item is a `const`, which turns the `let` into a pattern. A tuple's elements
are all evaluated before its binding exists, so only the one name is ever in
scope, and never during an initializer.

Generated code names the crate `::ohno`. The leading `::` is safe inside `ohno`
itself, which declares `extern crate self as ohno`.

## `#[ohno::error]`

R2 is a rewrite, so `error_attr` is one pass over a `syn::ItemStruct`:

1. Reject anything that is not a struct.
2. Reject `#[error]` on any field, a hand-written reserved doc marker on any
   field, and `#[no_constructors]` on the struct. All three are checked before
   anything is added, which is what lets the attribute say a marker was
   hand-written — a distinction the derive cannot make, because by the time it
   runs the two are the same text.
3. Add the core: `ohno_core: ohno::OhnoCore` for a named struct, renamed
   `ohno_core_1`, `ohno_core_2`, … on collision; an appended field for a tuple
   struct; a rewrite to a one-field tuple struct for a unit struct.
4. Put the reserved doc marker on the added field, and re-emit the struct under
   `#[derive(ohno::Error)]`, leaving every other attribute and doc comment in
   place.

The added field is marked with a doc comment rather than a helper attribute
because a derive helper attribute has to be listed in `attributes(...)` to be
inert, and everything in that list appears in the derive's public rustdoc.

## `#[enrich_err(...)]`

R3 is one module. The arguments are decoded into a `Message` — the default
`"error in function <name>"` when there are none, and otherwise the literal and
whatever follows it, passed through unchanged. The function is parsed as
`syn::ItemFn`, a missing return type is rejected, and the body is re-emitted
inside an immediately-invoked closure so that `?` still returns from the wrapped
body:

```rust
{
    let __ohno_result = (|| #block)();                  // sync
    let __ohno_result = async #block.await;             // async
    __ohno_result.map_err(|mut error| {
        ::ohno::Enrichable::add_enrichment(
            &mut error,
            ::ohno::EnrichmentEntry::new(#message, file!(), line!()),
        );
        error
    })
}
```

The message is applied through `map_err` rather than through `Enrichable` on the
result directly, because the return type is not always a `Result`: an
implemented `Future::poll` returns `Poll<Result<..>>`, which carries `map_err`
but is not itself `Enrichable`.

Neither arm names the declared return type. The wrapper's tail is the function's
return expression, so inference reaches it from the signature, and naming the
type would put it in a closure return type or a `let` annotation — positions an
opaque type is not allowed in, which would cost every function returning
`Result<impl Trait, E>`.

The closure is not `move`. Capture is left to inference, so a body that consumes
`self` takes it by value while a body that only reads it borrows, and the
message can still name a parameter the body did not consume.

Everything else in the signature — visibility, `const`, `unsafe`, `extern` ABI,
generics, bounds, where clauses, `impl Trait` and `dyn Trait` parameters, every
form of `self` receiver, doc comments, other attributes — is re-emitted
untouched, because the macro reads only the return type and the body.

**Diagnostics re-emit the function.** A rejected input still emits the original
function beside the `compile_error!`, so `rustc` reports the one fault the macro
found rather than that fault plus every call site of a function that no longer
exists.

## Limits

Some inputs reach `rustc` as an error instead of being reported by a macro. This
is where the R4 guarantee stops being structural.

**A `const fn` under `#[enrich_err(...)]`.** The closure rewrite is not usable in
a `const fn`, and `const` is re-emitted faithfully, so such a function is
rejected by `rustc` rather than by the macro. No test exercises the combination.

**A `#[display(...)]` format spec is not checked.** The text after the `:` is
carried into the generated `format!` as written, so a spec that refers to another
argument — a width or precision naming something the generated code does not
have — is reported by `rustc` against the derive rather than by the macro against
the template. Checking it would mean parsing the format grammar a second time.

**A return type carrying no `map_err`.** `#[enrich_err(...)]` checks that a
return type is present, not what it is, so a function returning a plain value is
rejected by `rustc` against the generated `map_err` call. This cannot be decided
syntactically: `Result` is reachable through an alias such as `io::Result<T>`,
and the supported `Poll<Result<..>>` is not a `Result` at all, which is why the
wrapper goes through `map_err` rather than naming the type.

## Diagnostics

`diagnostics.rs` holds one type:

```rust
/// Zero or more faults, reported together
#[derive(Default)]
struct Errors(Option<syn::Error>);

impl Errors {
    /// Anchors at `tokens`, which is always `syn::Error::new_spanned`
    fn add(&mut self, tokens: impl ToTokens, message: impl Display);
    fn combine(&mut self, error: syn::Error);
    fn is_empty(&self) -> bool;
    /// One `compile_error!` per fault; empty when nothing was recorded
    fn into_compile_error(self) -> TokenStream;
}
```

`add` is the only way to record a fault, and it takes tokens rather than a
`Span`, so R4's "wherever a diagnostic covers more than one token it is spanned
with `syn::Error::new_spanned`" is enforced by the accumulator's signature
rather than by remembering. `syn::Error` renders a combined error as one
`compile_error!` per fault, so the accumulation costs nothing at the boundary.

### Accumulating without cascading

Accumulation runs across *independent* concerns. Within one macro invocation,
core selection (R1.2), the display message (R1.5), the conversions (R1.6) and
the flags (R1.7) are checked in full, whatever the others found.

Core selection and display validation are independent even though both concern
fields, because the set of referenceable fields is "every field the user wrote"
— defined by the reserved marker, not by which field was selected. A struct that
marks two fields still gets its template checked.

A concern whose own input failed is **skipped, not guessed at**. An undecodable
`#[display]` never reaches validate, so no field or argument-count fault is
invented from a repaired template. A `#[from(...)]` whose source type did not
parse contributes no field-key faults. This is the rule the parse/validate split
exists to make expressible.

### Choosing a span

Five anchors, applied in this order:

1. A fault in what an attribute **says** → the attribute's `Meta`
   (`#[error(nonsense)]` underlines `error(nonsense)`).
2. A fault in an attribute **being there**, or being there twice → the whole
   `Attribute`, `#` included.
3. A fault in a **field's declaration** → the whole `Field`, its attributes
   included.
4. A fault **inside a `#[display]` template** → the template's string literal.
5. A fault **in a `#[display]` argument** → the smallest sub-expression carrying
   the fault, which for a rooting fault is the root term.

A fault about the type as a whole, with no attribute or field to point at, is
anchored at the struct's `ident`.

### Every rejection, its message and its anchor

Messages already pinned by a `.stderr` snapshot are quoted exactly and must not
drift. The rest are quoted as implemented; they have no snapshot, so a `.stderr`
fixture may be added for any of them without changing the text.

| Req | Rejection | Message | Anchor |
| --- | --- | --- | --- |
| R1.1 | derive on an enum | ``` `#[derive(ohno::Error)]` supports structs only. An enum has no single field to hold the OhnoCore ``` | `ident` |
| R1.1 | derive on a union | ``` `#[derive(ohno::Error)]` supports structs only ``` | `ident` |
| R1.1 | derive on a unit struct | ``` `#[derive(ohno::Error)]` needs a field to hold the OhnoCore, and a unit struct has none. Declare one, or use `#[ohno::error]`, which adds it ``` | `ident` |
| R1.2 | `#[error]` with an argument | `` `#[error]` takes no arguments `` | attribute `Meta` |
| R1.2 | two fields marked | ``Multiple fields marked with `#[error]`. Mark only the field holding the OhnoCore`` | the second marking `Attribute` |
| R1.2 | one field marked twice | ``Duplicate `#[error]` on the same field. Mark it once`` | the second `Attribute` |
| R1.2 | `#[error]` beside the generated marker | as the two-marked-fields message | the marking `Attribute` |
| R1.2 | no marker, no `OhnoCore` field | ``No field holds the OhnoCore. Declare one, mark it with `#[error]` if its type is spelled through an alias, or use `#[ohno::error]`, which adds the field itself`` | `ident` |
| R1.2 | no marker, several `OhnoCore` fields | ``Several fields hold an OhnoCore and none is marked. Mark the one holding the error representation with `#[error]``` | `ident` |
| R1.5 | placeholder names no field | ``unknown field `pth` in `#[display(...)]`, available fields: `path`, `code``` | template literal |
| R1.5 | the same, on a type with nothing referenceable | ``unknown field `pth` in `#[display(...)]`, the error type has no fields that can be referenced`` | template literal |
| R1.5 | argument root names no field | the same message | the root term |
| R1.5 | `{}` with no argument left | ``` `#[display(...)]` template has more `{}` placeholders than arguments ``` | template literal |
| R1.5 | an argument no `{}` consumes | ``` `#[display(...)]` argument is not consumed by any `{}` placeholder ``` | the argument |
| R1.5 | argument written `self.x` | `` `#[display(...)]` positional arguments are implicitly scoped to `self`, so a field is referenced by its bare name, without a `self.` prefix `` | the `self` token |
| R1.5 | argument rooted elsewhere | `` `#[display(...)]` positional arguments are implicitly scoped to `self`, so each argument must be rooted in a field or method of `self` `` | the whole argument |
| R1.5 | `{` with no `}` | `` `#[display(...)]` template has a `{` with no matching `}`. Close the placeholder, or write `{{` for a literal brace `` | template literal |
| R1.5 | `}` with no `{` | `` `#[display(...)]` template has a `}` with no matching `{`. Open the placeholder, or write `}}` for a literal brace `` | template literal |
| R1.6 | `#[from]`, `#[from = "…"]` | ``` `#[from(...)]` takes a parenthesized list of types, such as `#[from(std::io::Error)]` ``` | attribute `Meta` |
| R1.6 | `#[from()]` | ``` `#[from(...)]` needs at least one type, such as `#[from(std::io::Error)]` ``` | attribute `Meta` |
| R1.6 | a key naming no non-core field | ``unknown field `missing` in `#[from(...)]`, available fields: `kind`` | the key |
| R1.6 | a key naming the core | ``` `#[from(...)]` cannot initialize `inner`, which holds the OhnoCore and is built from the source error ``` | the key |
| R1.6 | a non-integer key on a tuple struct | ``` `#[from(...)]` field keys for a tuple struct are field indexes, not names, so `kind:` names no field ``` | the key |
| R1.7 | `#[no_constructors]` under `#[ohno::error]` | `` `#[no_constructors]` is not supported under `#[ohno::error]`. A constructor has to initialize the OhnoCore field, and the field inserted by `#[ohno::error]` is an implementation detail with no stable name or position, so it must not be referred to in code. Use `#[derive(ohno::Error)]` and declare the OhnoCore field explicitly `` | the whole `Attribute` |
| R2 | `#[ohno::error]` on a non-struct | ``` `#[ohno::error]` supports structs only. A struct is what can hold the OhnoCore field it adds ``` | the item |
| R2 | `#[error]` under `#[ohno::error]` | `` `#[ohno::error]` adds the OhnoCore field itself and generates the error representation from it, so no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use `#[derive(ohno::Error)]` to place the core explicitly `` | the whole `Attribute` |
| R2 | a hand-written reserved marker | `` This doc comment is reserved for `#[ohno::error]`, which puts it on the OhnoCore field it adds. Remove it; if this is the field holding the OhnoCore, use `#[derive(ohno::Error)]` and mark it with `#[error]` `` | the whole `Field` |
| R3 | first token is not a string literal | `syn`'s own `expected string literal` | the offending token |
| R3 | function with no return type | ``` `#[enrich_err(...)]` needs a return type to enrich. A function returning `()` has no error to carry the message ``` | the signature |
| R3 | input is not a function | ``` `#[enrich_err(...)]` applies to functions only ``` | the item |

Accumulation does not change the snapshots under `crates/ohno/tests/ui/`. Each
fixture struct there breaks exactly one rule, and separate structs are separate
macro invocations, so the errors were already reported together. A future
fixture that breaks several rules in one struct will report all of them.

## Testing

The regression boundary is the public API of `ohno_macros_impl`.
`crates/ohno_macros_impl/tests/public_api.rs` calls `derive_error`, `enrich_err`
and `error` — the same three functions the shim delegates to, taking the same
arguments — and snapshots the pretty-printed expansion of each case with `insta`.

Those three functions take `proc_macro2::TokenStream`, so a test can call them;
the `#[proc_macro]` entry points above them cannot be called at all. An input
that cannot be provoked from there cannot be provoked by a user of `ohno`, so an
uncovered line below is dead code rather than a missing test.

Cases are grouped by behavior, not filed one per input. Related inputs share a
snapshot, separated by `// === label ===` headers, so the number of snapshot
files tracks the number of behaviors and the variants of one behavior are read
together. Each case records the source a user would write above the expansion it
produces, so a snapshot is reviewable without the test file open beside it.

| Behavior | Case shape |
| --- | --- |
| `#[derive(Error)]` | the item in, the generated impls snapshotted |
| `#[ohno::error]` | the item in, the rewritten struct snapshotted |
| `#[enrich_err(...)]` | arguments and function in, the rewritten body snapshotted |
| any rejection | the offending input in, the emitted `compile_error!`s snapshotted |

A rejection is snapshotted like a success, in the file of the behavior it belongs
to. `expand` emits diagnostics *instead of* items whenever a fault was recorded,
so "what the macro wrote" is the whole answer either way.

`insta` and `testing_aids` are dev dependencies of `ohno_macros_impl`.
`testing_aids::render_expansion` does the pretty-printing; the source side goes
through `render_tokens_lossy`, because several cases feed a macro something that
is not an item at all.

Everything that produces tokens is snapshotted whole rather than searched for
substrings. What these macros have to get right is the *shape* of what they emit
— which body runs where, which field lands in which position, what survives
beside it — and a substring assertion cannot see shape. Snapshotting the
pretty-printed output also keeps the expected value readable as Rust rather than
as the space-separated token soup a `TokenStream` renders to.

A rejection snapshot carries every diagnostic the input produces, not the first,
so accumulation cannot regress silently.

There are no phase-local tests. Nothing feeds `Ast` to `validate` or a
hand-built `Model` to `generate`, so a `Model` that no real input produces cannot
be covered, and `generate`'s branches — `Style`, the presence of a message, an
empty `data()` — are reached only through an input that provokes them. A branch
that no input reaches is dead code and is removed. Every case runs the whole
pipeline, so a change to attribute syntax moves expansion snapshots.

`cargo mutants` matters most on `validate.rs` and `display/`, where the rules
live and where a surviving mutant means a rule is unenforced (R5).

One class of mutant survives by construction without meaning a rule is
unenforced. The three entry points in `ohno_macros/src/lib.rs` cannot be called
from a unit test at all — a proc-macro crate's own tests do not get the bridge —
so only `crates/ohno/tests/` kills them, and they carry
`#[cfg_attr(test, mutants::skip)]`. They hold nothing but delegation, which is
why that is the only untestable part.

The template scanner is driven by an iterator rather than by an index it
increments, so a mutant that corrupts its arithmetic produces a wrong segment a
test asserts on instead of a scan that never ends. A hanging mutant is reported
as a timeout rather than as a failed assertion, which reads as an unenforced
rule when it is not one.

**The tests under `crates/ohno/tests/` are the only proof that the generated code
works.** Neither crate's own tree compiles an expansion. The snapshots in
`public_api.rs` assert the shape of tokens; only the integration tests and the
`ui/*.rs` compile-fail pairs run `rustc` over what the macros produce, and only
the `.stderr` snapshots pin where a diagnostic points. A change that keeps every
snapshot green and breaks the tests under `crates/ohno/tests/` is a broken
change.

`just trybuild` runs those compile-fail tests alone while iterating on a
diagnostic, and `just trybuild-overwrite` rewrites the `.stderr` snapshots when a
message or a span changes on purpose. Always read the resulting diff: a snapshot
that changed for a reason you cannot name is a regression in a diagnostic, not a
refresh.
