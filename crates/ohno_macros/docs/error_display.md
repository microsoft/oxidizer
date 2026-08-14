# `#[display(...)]`

The message a generated error renders.

## Form

```rust
#[display("template")]
#[display("template", arg1, arg2)]
```

The template is a `format!` string. Anything that is not a plain field name goes
in the argument list.

## Placeholders

| Placeholder | Meaning |
| --- | --- |
| `{name}` | the field called `name` |
| `{0}` | tuple field 0 |
| `{name:spec}` | the field, with `spec` as the format spec |
| `{}` | the next argument |
| `{{`, `}}` | a literal brace |

## Arguments are scoped to `self`

A field is written by its bare name. The `self.` is implicit, which is where the
crate differs from `thiserror`.

| Argument | Works |
| --- | --- |
| `path.display()` | yes |
| `count * 2` | yes |
| `self.path.display()` | no, `self.` is already there |
| `.path.display()` | no, not valid Rust |

An argument is scoped by its leftmost term, so that term is the one that has to
name a field, or call a method of `self`: `count * 2` is rooted at `count`,
`t.0.message()` at `t`, and `describe()` calls a method. An argument rooted
anywhere else — a constant, an associated function, a string literal — reaches
nothing on `self` and is rejected.

An unsuffixed numeric literal is the exception, because that is how a tuple field
is named. `0` is a field root, so `{0}` and `0.abs()` are both rooted at field
`0`, and a float root is read as nested tuple access (see below). A suffix is not
part of a tuple index, so `0u8` is rooted nowhere and is rejected.

The operator applies to the field's value, not to a reference to it, so
`count as u64` casts the field and `count * 2` uses the value's `Mul`.

## Which fields can be named

Every field the user wrote. By name for a named struct, by index for a tuple
struct.

The `OhnoCore` field added by `#[ohno::error]` is left out. Printing it would
print the error's own chain, and naming it in an error message would point at a
field that is not in the user's code. A core field the user wrote is *not* left
out. It is plain data. See `error_error.md`.

Raw identifiers keep their prefix. A field declared as `r#type` is written
`{r#type}`, and error messages offer `` `r#type` ``, because `type` alone does
not parse there.

## Error reporting

The macro reports these itself rather than letting the expansion fail. Otherwise
a bad argument reaches `rustc` as a field access in code the user cannot see,
and the added `OhnoCore` field appears in rustc's own list of available fields.

Errors point at what the user wrote: at the template for a template error, at
the offending term for an argument error.

An error covering more than one token has to be built with
`syn::Error::new_spanned`, not from a node span. A node span covers the whole
node only where `Span::join` exists, and shrinks to the first token elsewhere.
The same error would then underline different amounts of code on different
toolchains, and no single `.stderr` snapshot could match both.

## What gets rejected

- A placeholder naming something that is not a field.
- A `{}` with no argument left, or an argument no `{}` uses.
- An argument written with a `self.` prefix.
- An argument rooted in anything other than a field or method of `self`.
- An unbalanced brace: a `{` with no matching `}`, or a `}` with no matching `{`.

## Implementation notes

**A template is parsed before it is judged.** Splitting it into segments first
keeps "what the template says" apart from "whether that names a field", so
neither decision is made halfway through a scan. Every segment borrows from the
template, which works because none of them is rewritten on the way out.

**An unbalanced brace is reported, not repaired.** Letting an unterminated `{`
run to the end of the template would honor `"{path"` as `"{path}"`, so a typo
would render as a working message and never surface. A stray `}` is rejected for
the mirror-image reason: it would otherwise be copied into the generated
`format!` string, where `rustc` reports it against code the user cannot see. Both
are spanned at the template, which is the only thing the user wrote.

**The scoping prefix is applied to the argument's leftmost term.** Which
expression forms may legally follow a dot is answered by enumerating them, and
anything else is reported rather than prefixed.

**The result is then wrapped as `&(...)`.** The parentheses are load-bearing: a
bare `&self.<argument>` binds the reference to the leftmost term alone, so
`count as u64` would cast the reference rather than the field, and `count * 2`
would multiply it.

**Roots are found by walking left.** Field access, method calls, indexing,
binary operators, casts, `await`, `?` and ranges all keep a term in leftmost
position, which is where the prefix lands. A call in that position is a method
of `self`, so it is prefixed without being looked up as a field. A `self` root
is reported separately, because it would otherwise expand to `self.self`.

**A nested tuple index arrives as a float.** `0.1` lexes as one literal, and only
its leading component names a field of `self`; the rest reaches into that
field's own type and is left to `rustc`.

**Raw identifiers keep their `r#`.** Field names are compared as text, so a name
spelled `r#type` in the template has to match the field spelled the same way.
