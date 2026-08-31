# Publishing private test, bench and example utils

Some crates need substantial shared scaffolding — a mock server, a fixture
harness, a scripted backend — before their tests, benchmarks and examples can
say anything useful. This page describes how to host that scaffolding without
either shipping it as public API or splitting the consumers away from the code
they exercise.

`fetch_winhttp` is the worked example: its localhost HTTP/1.1, HTTP/2, TLS and
HTTP/3 servers are reached as `fetch_winhttp_impl::testing`.

## The problem

The obvious placement — a `publish = false` fixture package that depends on the
crate under test — does not work, because the crate under test then has to
dev-depend on the fixture package to use it:

```text
fetch_winhttp ──[dev-dependency]──> fixtures ──[dependency]──> fetch_winhttp
```

That is a cycle. `cargo ensure-no-cyclic-deps` runs on every platform in
`static-analysis` and treats dev-dependency cycles as cycles, and the root
`Cargo.toml` bans them outright: they confuse `rust-analyzer` and leave the
publishing order ambiguous.

Moving the tests, benchmarks and examples into the fixture package breaks the
cycle, but it separates them from the public API they document, and it leaves
the published crate with no tests of its own.

The remaining option — a feature on the published crate that exposes the
fixtures — grows that crate's *public* API. A feature is part of the published
surface: consumers can enable it, its items appear in the documentation, and
removing it later is a breaking change. Test scaffolding does not belong there.

## The shape that works

Split the crate in two and point both edges at the lower half:

```text
        fetch_winhttp                     the published, supported API:
        (facade + tests +                 re-exports, plus every test,
         benches + examples)              benchmark and example
              │        │
   dependency │        │ dev-dependency
              │        │ features = ["private-test-util"]
              v        v
       fetch_winhttp_impl                 the implementation, plus the
       (impl + `pub mod testing`)         fixtures behind the feature
```

Both arrows point the same way, so there is no cycle. The tests, benchmarks and
examples stay in the facade, beside the API they exercise, and reach the
fixtures through the implementation crate.

Cargo's resolver activates a dev-dependency's features **only when
dev-dependencies are built**. A consumer running `cargo build` gets
`fetch_winhttp_impl` without `private-test-util`; `cargo test`, `cargo bench`
and `cargo run --example` in this workspace get it with.

## Rules

- **Name the feature `private-test-util`.** The `test-util` name is reserved for
  features that are a supported part of a crate's public API, such as
  `tick`'s. The `private-` prefix marks a feature that exists only for the
  facade's own development code, on a crate no one is meant to depend on.

- **Put the feature on the implementation crate, never on the facade.** This is
  the whole point of the split: the facade's public surface must not grow a
  feature, a module, or a dependency because of test scaffolding.

- **Say so in the implementation crate's documentation.** Its crate-level docs
  should state that it is an implementation detail of the facade, that its
  public items exist only to be re-exported, and that nothing in it is
  supported.

- **Declare the fixture dependencies `optional = true`** and list them in the
  feature as `dep:name`. They must not be reachable when the feature is off.
  This is an exception to the workspace "Optional Dependencies in Test Builds"
  rule: the fixture module is gated on `feature = "private-test-util"` alone
  (not `cfg(any(test, feature = ...))`), and those optional dependencies are
  not mirrored as non-optional dev-dependencies of the implementation crate,
  because the implementation crate's own unit tests do not consume the fixtures.

- **Use this split only when the fixtures themselves need the crate under
  test.** If they do not, a `publish = false` fixture package with no reverse
  dependency remains the simpler answer.

- **Exempt the fixture module from coverage and mutation testing.** It is
  scaffolding driven by the tests, not code under test. Mark the module
  `#[cfg_attr(coverage_nightly, coverage(off))]` and add its path to
  `exclude_globs` in `.cargo/mutants.toml`.

- **Group the two packages in `TEST_GROUPS`** in `scripts/mutants.rs`. The
  implementation crate's integration tests live in the facade, so mutating it
  in isolation would report mutants that its own package cannot catch.

- **Allow the implementation crate's types through the facade's
  external-type check.** The facade re-exports them, so
  `allowed_external_types` needs a `fetch_winhttp_impl::*`-style entry; the
  facade is what constrains the surface that actually ships.

## Costs to accept

The implementation crate is published, so the fixture sources ship inside its
`.crate` and the fixture dependencies appear in its manifest as optional
entries. Neither is built by a consumer, and this is the price of keeping the
facade's public API clean.

## When not to use this

If the scaffolding is genuinely useful to consumers — a mock implementation of
a trait the crate exports, say — it is public API. Ship it from the published
crate behind an ordinary `test-util` feature, or as its own `*_testing` package
that consumers can depend on, as `observed_testing` does.

If the crate has no separate implementation package and does not need one, a
`required-features` entry on the `[[bench]]` and `[[example]]` targets is
simpler; `bytesbuf` does this. That still puts the feature on the published
crate, so it is only appropriate when the feature is one consumers may use.
