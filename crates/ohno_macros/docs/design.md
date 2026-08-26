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
attributes is all `Ast` adds over `syn::DeriveInput`. It is consumed by the
validation phase alone, in exactly one module.

The two phases are also what make the "skipped, not guessed" rule in the
Diagnostics section expressible. An attribute that fails to decode is reported by
parse and is *absent from `Ast`*, so validate has nothing to check there and
reports only faults it can see.

## Two crates

A crate with `proc-macro = true` can export nothing but macros, and its own tests
cannot call them: a `proc_macro::TokenStream` exists only inside a real
expansion. The logic therefore lives in an ordinary library, `ohno_macros_impl`,
and the proc-macro crate `ohno_macros` holds the entry points alone.

`ohno_macros_impl` exposes the three expansions as ordinary functions over
`proc_macro2::TokenStream`. Each entry point in `ohno_macros` converts the
token-stream type, delegates to its counterpart, and converts back. Nothing else
lives in a shim, so the compiler-facing crate holds no branch a test cannot
reach.

`ohno_macros_impl` is not a public API. Its rustdoc says so, and `ohno` depends
on `ohno_macros` rather than on it. Only its three expansion functions are `pub`;
every module below them is private. Its expansion snapshots are described under
Testing.

These documents sit under `crates/ohno_macros/docs/` because they describe the
macros `ohno` re-exports. Read "the crate" below as "the two crates together"
wherever the distinction does not matter.

Where they name a path, they name it from the workspace root. Anything that
cannot be written that way is prose naming a role instead of a file, which is
also why the module map below stops at directories: which file holds which
function is not a decision this document owns, and it is the part that rots when
the code moves.

## Modules

The implementation crate is organised by *what a module decides*, not by which
type it holds. Everything below sits under its source root.

```text
one module per macro     derive_error, error_attr, enrich_err
shared by all three      diagnostics, message, paths, marker
inside derive_error      one module per phase: parse, validate, generate,
                         with the phase's own types beside it
inside display           the template scanner and the argument rooting that
                         `#[display(...)]` lowering needs
inside generate          one module per family of generated item: the trait
                         impls, the constructors, the conversions
```

The crate root holds the three expansion functions and the two steps the shim
cannot do for them: turning the incoming tokens into a `syn::DeriveInput` or
`syn::Item` and reporting the parse failure as a compile error, and rejecting
arguments given to `#[ohno::error]`, which takes none. Everything past that point
is a module above.

Only the derive carries the full pipeline.

`error_attr` keeps no `Model`. R2 gives it three rejections and one field
injection, and it hands its output to the derive, which validates the result
again.

`enrich_err` keeps no `Ast`. R3 gives it a message and a signature, and the
signature is re-emitted rather than read, so decoding and checking are one step
that yields a `Message`.

`Message` is shared rather than owned by the derive, because both macros end at
the same place — a `format!` string and a list of argument expressions — even
though they reach it differently. The derive lowers field names into
`self`-scoped accesses (R1.5); `enrich_err` passes the literal and the arguments
through unchanged, because its placeholders name function parameters that `rustc`
resolves and its arguments are ordinary expressions in the function's own scope
(R3).

## The types

### `Ast`

`Ast` records what the struct says, with the crate's own attributes decoded: the
identifier, the generics, the field style, the fields, and one slot per helper
attribute — the `#[display]` payload, the `#[from(...)]` entries, and the two
suppressing flags. The `#[display]` slot is absent both when the attribute was
not written and when it failed to decode, which is what lets validate skip a
check whose input it does not have. There is one conversion entry per type
listed, across every `#[from(...)]` written, since R1.6 allows several of each.

Style is named or tuple only. A unit struct never reaches `Ast`: R1.1 rejects it
in parse, because it has no room for a core.

A field records its member, its type, the hand-written `#[error]` markers on it,
and whether it carries the reserved doc marker `#[ohno::error]` writes. Two
choices there are load-bearing. The markers are a **list**, not an optional
single value, so "marked twice" is representable and therefore reportable
(R1.2), and each is kept as a whole attribute so a diagnostic can point at the
marker the user wrote. The generated marker is tracked **separately** from
hand-written ones, because R1.2 treats the two as different inputs.

The `#[display]` payload keeps its template as a whole string literal so
diagnostics can anchor at it. The `#[from(...)]` payload keeps its field
expressions keyed as the user wrote them; whether a key names a field is a rule,
so it is checked in validate rather than at decode time.

### `Model`

`Model` is a validated error type, ready to generate from: the identifier, the
generics, the `Shape`, the lowered message if there is one (R1.5), the
conversions, and the two flags governing whether `Debug` and the constructors are
emitted (R1.4, R1.7).

`Shape` makes "exactly one core" structural rather than checked. It holds the
style and the fields in declaration order, **split around** the one holding the
core: those before it, the core itself, those after. It offers the core, every
field in declaration order (what `Debug` prints, R1.3), and every field but the
core (what constructors take and what a conversion initializes, R1.4 and R1.6).

Splitting around the core rather than carrying an index into one list removes a
class of check. An index can dangle, so a generator using one would have to
handle a core that is not there; a before/core/after split cannot express that,
and declaration order is still recoverable from it. The split also removes the
"named struct with a positional core" case: the style and the members are read
from the same value, so they cannot disagree.

The constructor is infallible, and an out-of-range core index panics. The index
is not user input by the time it arrives: `validate` finds the core in the `Ast`
fields and maps that same list, one for one, into the model fields it passes
alongside the index. A disagreement between the two is a fault in `validate`, not
an input the author can act on, so there is no diagnostic to report and none is
invented — R4 forbids one that points into generated code. The panic is
documented on the constructor and named in the `expect` that raises it.

A model field carries how it is written in an expression — `path` or `0` — and
how it is bound as a constructor parameter, which is its own name for a named
struct and `param_0`, `param_1`, … by index for a tuple one (R1.4). The first of
those removes most of the named-versus-tuple branching from `generate`: a field
read is one form either way. Style is consulted by exactly two things — `Debug`,
which needs `debug_struct` for a named struct and `debug_tuple` for a tuple one
(R1.3), and the shared builder that emits `Self { .. }` or `Self(..)`, which the
constructors and every generated `From` go through.

### `Message`

`Message` is a lowered `format!` call, in one of two forms. A message with no
arguments is a plain string and renders as a string literal, so a static
`#[display("...")]` costs no allocation at run time. A message with arguments
carries a `format!` string and one expression per placeholder that consumes an
argument, in order, and renders as a `format!` call. In the template every
placeholder that named a field has become positional, and a format spec is
preserved, so `{name:?}` becomes `{:?}`.

The derive resolves `{{` and `}}` escapes when it builds a literal, since that
text no longer passes through `format!`. `enrich_err` cannot: it does not
interpret its template, so it keeps a message literal only when there are no
arguments *and* no braces at all — a brace may open a placeholder `format!` still
has to resolve.

Rendering yields either a `&'static str` or a `String`, and both satisfy the
`Into<Cow<_, str>>` bounds the runtime asks for, so nothing downstream branches
on which form it is.

By the time a `Message` built by the derive reaches `generate`, every field it
names exists and every argument is rooted in a field or a method of `self`
(R1.5) — the latter because the derive's entry point emits nothing but
diagnostics once a fault was recorded, not because `Message` could not hold such
an argument. A named
placeholder lowers to a reference to the field it resolved. A positional argument
is wrapped as a reference to a *parenthesized* access, the parentheses being
load-bearing: a bare `&self.<argument>` would bind the reference to the leftmost
term alone, so `count as u64` would cast the reference rather than the field.

### `Conversion`

One `From<T>` from a `#[from(...)]` entry (R1.6): the source type, and one
initializer per non-core field, aligned with the model's non-core field list. A
field the user did not name is initialized with `Default::default()`.

### Where the invariants live

Two of the three invariants `generate` relies on are structural: `Shape` makes
"exactly one core" unrepresentable, and `Message` names only fields that exist,
because the only way to obtain either type is through validation.

An argument that is not rooted in a field is procedural rather than structural.
Lowering records the fault and still returns the `Message`, so a bad template and
a bad argument are reported together; the argument is kept out of generated code
by the derive's entry point, which emits the diagnostics alone whenever any fault
was recorded.

The third is procedural too. "the initializer list is as long as the non-core
field list" is a relation between two values, which Rust cannot hold in a struct
shape without an encoding heavier than the check it replaces. A module boundary
holds it instead: the initializers are private to the model module, and the only
constructor distributes the user's overrides over the non-core fields, defaults
the rest, and reports an override whose key names no non-core field. The
alignment is therefore established once, where the field list is in hand, rather
than at each use.

## Generating the items in R1.3

Generation calls one function per row of the R1.3 table and concatenates the
results. Generics thread through identically everywhere: the model's generics are
split once into their impl, type and where-clause forms, and every generated
trait `impl` carries all three, as does the single inherent `impl` holding the
constructors. That covers lifetimes, type parameters and where clauses in one
rule, which is what R1.3's "all of them carry the input's generics" asks for.

Two values recur below: the core field's member, and the type's name as a string
literal — the default message the runtime falls back to when nothing else
renders.

**`Display`** delegates to the core, passing the lowered message as an override
when there is one. `OhnoCore::format_error` appends `caused by:`, the enrichment
lines and the backtrace, so the generated code decides only the message. The
override is `None` when the model has no message, and otherwise a `Cow` built
from the rendered one. `Cow::from` accepts both a `&'static str` and a `String`,
so a static `#[display("...")]` allocates nothing and a formatted one becomes a
`format!` call, without the generator branching on which it is.

**`std::error::Error`** delegates `source` to the core.

**`ohno::Enrichable`** delegates `add_enrichment` to the core, which is the trait
`OhnoCore` itself implements. **`ohno::ErrorExt`** delegates `backtrace` to the
core, and `message` to the core's `format_message` with the same override the
`Display` impl builds — which is why the two agree by construction rather than by
being written twice.

**`From<Infallible>`** is emitted unconditionally with an `unreachable!` body, so
a fallible conversion in user code can be written once and stay correct when the
error type becomes infallible.

**`Debug`** is emitted unless the model suppresses it. It prints every field
including the core, so it iterates the full field list and branches once on
style, into `debug_struct` for a named struct or `debug_tuple` for a tuple one.

It is the one generated item that is **not** `#[automatically_derived]`.
Dead-code analysis ignores field reads inside a derived `Debug`, so marking this
one would make every field that only `Debug` reads look unused in the user's own
crate.

**Constructors** are emitted unless the model suppresses them. `new` takes one
parameter per non-core field and defaults the core; `caused_by` takes the same
parameters plus a source error and builds the core from it. Both iterate the
non-core fields, so the core is skipped and declaration order is kept, and both
take each parameter as `impl Into<_>` of the field's type. `pub(crate)` is fixed
by R1.4, settled by ADO 7675155.

**`From<T>`** is emitted once per conversion, zipping the non-core fields with
that conversion's initializers and building the core from the source error. The
source binding is named `error`, which is the name a field expression refers to.

The data initializers are evaluated into one tuple before the struct literal is
built. They may borrow `error` while the core consumes it, and a struct literal
evaluates its fields in the order they are written, so the core has to be built
last. One tuple, not one local per field: a generated name is not hygienic, so a
local per field would be in scope while the next initializer is evaluated, and an
initializer naming an outer item the derive happened to shadow would read the
generated local instead — silently, or as an error in generated code when the
outer item is a `const`, which turns the `let` into a pattern. A tuple's elements
are all evaluated before its binding exists, so only the one name is ever in
scope, and never during an initializer.

Generated code names the crate `::ohno`. The leading `::` is safe inside `ohno`
itself, which declares `extern crate self as ohno`.

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
inside an immediately-invoked closure — an `async` block for an `async` function
— so that `?` still returns from the wrapped body. Its result then goes through
`map_err`, which adds one enrichment entry carrying the message, the file and the
line.

The message is applied through `map_err` rather than through `Enrichable` on the
result directly, because the return type is not always a `Result`: an implemented
`Future::poll` returns `Poll<Result<..>>`, which carries `map_err` but is not
itself `Enrichable`.

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

One shared module holds the diagnostics accumulator, a value carrying zero or
more faults reported together. It wraps an optional `syn::Error`, and offers recording a fault,
merging a `syn::Error` from elsewhere, asking whether anything was recorded, and
rendering everything into `compile_error!` invocations — one per fault, and no
tokens at all when nothing was recorded.

Recording is the only way to add a fault, and it takes **tokens** rather than a
`Span`, so R4's "wherever a diagnostic covers more than one token it is spanned
with `syn::Error::new_spanned`" is enforced by the signature rather than by
remembering. `syn::Error` renders a combined error as one `compile_error!` per
fault, so the accumulation costs nothing at the boundary.

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

### Every rejection and its anchor

The wording of each message lives in the source. Those with a fixture under
`crates/ohno/tests/ui/` are pinned there by a `.stderr` snapshot that CI compares
on every run. This table records what is rejected and where the diagnostic
points, which is the part R4 constrains; it does not restate the text, which
would be a second copy that nothing checks.

| Req | Rejection | Anchor |
| --- | --- | --- |
| R1.1 | derive on an enum | `ident` |
| R1.1 | derive on a union | `ident` |
| R1.1 | derive on a unit struct | `ident` |
| R1.2 | `#[error]` with an argument | attribute `Meta` |
| R1.2 | two fields marked | the second marking `Attribute` |
| R1.2 | one field marked twice | the second `Attribute` |
| R1.2 | `#[error]` beside the generated marker | the marking `Attribute` |
| R1.2 | no marker, no `OhnoCore` field | `ident` |
| R1.2 | no marker, several `OhnoCore` fields | `ident` |
| R1.5 | placeholder names no field | template literal |
| R1.5 | argument root names no field | the root term |
| R1.5 | `{}` with no argument left | template literal |
| R1.5 | an argument no `{}` consumes | the argument |
| R1.5 | argument written `self.x` | the `self` token |
| R1.5 | argument rooted elsewhere | the whole argument |
| R1.5 | `{` with no `}`, or `}` with no `{` | template literal |
| R1.5 | a second `#[display(...)]` | attribute `Meta` |
| R1.6 | `#[from]`, `#[from = "…"]`, `#[from()]` | attribute `Meta` |
| R1.6 | a key naming no non-core field | the key |
| R1.6 | a key naming the core | the key |
| R1.6 | a non-integer key on a tuple struct | the key |
| R1.7 | a suppressing flag with an argument | attribute `Meta` |
| R1.7 | `#[no_constructors]` under `#[ohno::error]` | the whole `Attribute` |
| R2 | `#[ohno::error(...)]` with arguments | the arguments |
| R2 | `#[ohno::error]` on a non-struct | the item |
| R2 | `#[error]` under `#[ohno::error]` | the whole `Attribute` |
| R2 | a hand-written reserved marker | the whole `Field` |
| R3 | first token is not a string literal | the offending token |
| R3 | function with no return type | the signature |
| R3 | input is not a function | the item |

Wherever a name was not found, the message lists the fields that could have been
named, so a rejection under R1.5 or R1.6 offers them. The "takes no arguments"
rejections for `#[error]`, `#[no_debug]` and `#[no_constructors]` come from one
check over a bare marker attribute, so they cannot drift apart.

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
to. A macro emits diagnostics *instead of* items whenever a fault was recorded,
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

There are no phase-local tests. Nothing feeds an `Ast` to validation or a
hand-built `Model` to generation, so a `Model` that no real input produces cannot
be covered, and generation's branches — the field style, the presence of a
message, an empty non-core field list — are reached only through an input that
provokes them. A branch that no input reaches is dead code and is removed. Every
case runs the whole pipeline, so a change to attribute syntax moves expansion
snapshots.

`cargo mutants` matters most on validation and on `#[display(...)]` lowering,
where the rules live and where a surviving mutant means a rule is unenforced.

One class of mutant survives by construction without meaning a rule is
unenforced. The three entry points in `crates/ohno_macros/src/lib.rs` cannot be
called from a unit test at all — a proc-macro crate's own tests do not get the
bridge — so only `crates/ohno/tests/` kills them, and they carry
`#[cfg_attr(test, mutants::skip)]`. They hold nothing but delegation, which is
why that is the only untestable part.

The template scanner is driven by an iterator rather than by an index it
increments, so a mutant that corrupts its arithmetic produces a wrong segment a
test asserts on instead of a scan that never ends. A hanging mutant is reported
as a timeout rather than as a failed assertion, which reads as an unenforced
rule when it is not one.

**The tests under `crates/ohno/tests/` are the only proof that the generated code
works.** Neither macro crate's own tree compiles an expansion. The expansion
snapshots assert the shape of tokens; only the integration tests and the
compile-fail pairs under `crates/ohno/tests/ui/` run `rustc` over what the macros
produce, and only the `.stderr` snapshots pin where a diagnostic points. A change
that keeps every snapshot green and breaks the tests under `crates/ohno/tests/`
is a broken change.

The compile-fail tests are ordinary integration tests, so `just test` runs them
too. `just trybuild` narrows to them while iterating on a diagnostic — pass the
test target's name as the filter, as in
`just package=ohno trybuild display_diagnostics` — and `just trybuild-overwrite`
rewrites the `.stderr` snapshots when a message or a span changes on purpose.
Always read the resulting diff: a snapshot that changed for a reason you cannot
name is a regression in a diagnostic, not a refresh.
