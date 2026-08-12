# Optional dependencies in test builds

Feature-dependent code is gated behind `cfg(any(test, feature = "foo"))` rather
than `cfg(feature = "foo")`, so that a crate's own test build compiles the
feature-dependent code without the test target having to enumerate features.

```rust
#[cfg(any(test, feature = "dynamic-service"))]
mod dynamic;

#[cfg(any(test, feature = "dynamic-service"))]
pub use dynamic::{DynamicService, DynamicServiceExt};
```

`cfg(test)` and Cargo features are resolved by different mechanisms. A test
build turns on `cfg(test)`, which pulls the gated code into the compilation. It
does not turn the feature on, so nothing the feature would have activated is
activated. The manifest has to supply that separately.

## Features are additive

The pattern depends on features being additive: enabling one never removes or
changes behaviour that was available without it, and enabling all of them at
once is a valid configuration.

Mutually exclusive features are an invalid pattern. Two features that cannot be
enabled together cannot both be on in a test build, and a consumer combining two
unrelated dependents of the crate would break through no fault of their own.

## Optional dependencies are also dev-dependencies

A feature commonly activates an optional dependency. That dependency is not
linked in a test build, so the gated code fails to resolve it. Every optional
dependency is therefore also declared as a non-optional dev-dependency:

```toml
[dependencies]
tower-service = { workspace = true, optional = true }

[dev-dependencies]
tower-service = { workspace = true }
```

Where the dependency is another member of this workspace, the dev-dependency is
path-only — no version, no `workspace = true` — so that publish ordering
resolves correctly:

```toml
[dependencies]
plurality = { workspace = true, optional = true }

[dev-dependencies]
plurality = { path = "../plurality" }
```

Downstream consumers never build a dependency's dev-dependencies, so this does
not weaken the feature gate for them.

## Dependency features are also mirrored

Reproducing the dependency by name is not enough. The dev-dependency also
carries every feature that a feature build would give it, otherwise the gated
code compiles against a smaller API surface than the feature build provides.

Two sources contribute. The `[dependencies]` entry declares features directly,
and the `[features]` table activates further ones through `dep/feature` entries:

```toml
[features]
codegen = ["dep:syn", "bytesbuf/test-util"]

[dependencies]
syn = { workspace = true, optional = true, features = ["full", "parsing"] }

[dev-dependencies]
syn = { workspace = true, features = ["full", "parsing"] }
bytesbuf = { path = "../bytesbuf", features = ["test-util"] }
```

A feature that implies another feature of the same crate is not reproduced this
way, because no manifest entry can turn on a crate's own feature for its test
build. Code reached through such an implication is gated on the implied feature
as well.

## The test build is a superset

A path dev-dependency enables its target's default features, whereas the
matching `[dependencies]` entry inherits `default-features = false` from the
workspace. A test build therefore sees at least as much of each dependency as
any feature build does, and compiling under `cfg(test)` does not prove that the
same code compiles under the feature.

`just anvil-cargo-hack` checks the feature powerset of each library and is the
authority on whether feature-gated code compiles in a real feature build.

## Detecting a missing dev-dependency

A missing dev-dependency surfaces as gated code failing to resolve its
dependency:

```text
error[E0432]: unresolved import `plurality`
error[E0433]: cannot find module or crate `plurality` in this scope
```

Building test targets with default features surfaces it:

```powershell
cargo check -p <crate> --tests
```

Commands that pass `--all-features` enable the feature and link the optional
dependency, so they do not exercise this.
