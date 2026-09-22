# Runtime analysis

The runtime-analysis tier complements the ordinary test, coverage, and mutation
tiers. It does not need to repeat every behavioral assertion under every
interpreter. Each check should retain the cases that can expose failures unique
to that check:

- Miri executes code that owns `unsafe` invariants or supplies a distinct input
  path into such code.
- `cargo careful` runs the native suite against a standard library built with
  additional runtime checks.
- Loom exhaustively explores the small concurrency models selected by the
  dedicated `required-features = ["loom"]` targets.
- Bolero exercises operation sequences that hand-written tests do not cover.

## Miri selection

Use `[package.metadata.anvil.miri] exclude = true` when a package contains no
runtime `unsafe` code, does not provide a unique path into an unsafe dependency,
and interpreting its suite would mostly repeat functional coverage. The package
continues to compile when selected packages depend on it, and its complete test
suite continues to run natively and under `cargo careful`.

Prefer a test-level `#[cfg_attr(miri, ignore = "...")]` when only one
integration scenario lacks Miri-specific value. The reason must name the
coverage retained elsewhere. Use `cfg(miri)` to reduce a stress-test cardinality
instead of ignoring the test when a smaller case reaches the same unsafe or
concurrency path. Native test cardinalities must remain unchanged.

Miri-specific reductions should preserve the transition or boundary that makes
the test valuable rather than preserving an arbitrary native workload size:

- depth and length guards run just beyond their enforced bound;
- oversized-allocation tests use a lower configurable threshold when the
  physical chunk boundary itself is not the subject of the test;
- statistical and differential tests retain representative samples while their
  full populations continue to run natively;
- synchronization tests retain every participating role and at least one
  complete state transition;
- recorder tests use the minimum supported event-ring capacity unless ring
  sizing or wraparound is the behavior under test.

Pure runtime limits, stack-safety stress, formatting breadth, code generation,
and safe integration scenarios may be excluded from Miri when direct tests
continue to exercise the unsafe implementation or dependency path they cover.
The native and `cargo careful` suites remain authoritative for the excluded
stress breadth.

Safe default-constructor, preallocation, formatting, allocation-count, and
hash-table rollback tests do not gain additional value merely by running under
Miri. Prefer a reasoned test-level ignore for those cases while retaining a
smaller test that reaches the package's actual unsafe storage or lifecycle
path. Platform-emulation matrices similarly retain representative Miri cases
for each unsafe callback/lifecycle transition while leaving exhaustive option
and validation combinations to native tests.

The following package-level exclusions are intentional:

- Code-generation and benchmark orchestration packages are covered through
  generated code in consuming crates or by native process-level tests.
- `cachet` and `ohno` contain no runtime `unsafe` code. Their behavioral suites
  run natively and under `cargo careful`; their dependencies retain their own
  Miri coverage.
- `seismograph_cli` and `seismograph_runtime` are safe integration layers over
  `seismograph`. The unsafe recorder and snapshot implementations remain in the
  Miri-selected `seismograph` package.
- `rest_over_grpc` keeps its runtime transcoding tests under Miri, but omits
  build-time descriptor and code-generation unit tests. The build half is safe
  Rust and remains covered by native tests and `cargo careful`.
- `rallocator` keeps its library and allocator integration suites under Miri,
  but omits `tests/telemetry.rs` and `tests/performables_telemetry.rs`. Those
  binaries repeatedly capture and decode process-wide snapshots or validate a
  safe dependency graph; direct Miri tests in the library, `performables`, and
  `seismograph` cover the unsafe internals, while native tests and
  `cargo careful` retain the end-to-end assertions.
- `templated_uri` remains selected because it is a consumer-facing integration
  surface, but deterministic pseudo-fuzz breadth is reduced under Miri.
- `internity` keeps unchecked storage and resolution paths under Miri. Native
  tests retain default-constructor, preallocation, and hash-table rollback
  breadth, while a bounded long-string case forces storage growth before
  resolving every handle under Miri.
- `http_extensions` remains selected as an integration surface, but its native
  allocation-count test is not interpreted by Miri.
- `performables` retains direct ownership and synchronization coverage. Its
  channel telemetry integration uses the minimum supported event-ring capacity
  only under Miri; ring-capacity behavior remains covered in `seismograph`.
- `seismograph` retains direct recorder and snapshot coverage. Under Miri its
  object-sampling population, repeated snapshot count, and default test event
  ring capacity are reduced while still exercising selection, capture, and
  buffer lifecycle transitions.
- `fetch_winhttp_impl` retains callback, raw-context, handle, operation-buffer,
  and foreign-thread lifecycle coverage. Native tests keep exhaustive TLS,
  protocol, and port-option matrices; Miri executes representative cases.
- `routerama` remains selected so its unsafe scanned-path and query-scanning
  helpers execute under Miri. Native-only depth keeps the full stack-safety
  stress, while Miri uses smaller paths that still cross resolver bounds and
  exercise iterative teardown.
- `multitude` retains direct Miri coverage across its unsafe allocation,
  ownership, destructor, and chunk-lifecycle paths. Tests use smaller
  configurable oversized thresholds or bulk initialization when per-element
  repetition does not add another unsafe state transition.
- `compressors` cannot run all backends under Miri; its manifest records the
  backend-specific rationale.

Do not infer that a safe package is automatically dispensable.
`rest_over_grpc`, for example, remains selected because its runtime tests
provide the workspace's Miri coverage for the transcoding buffer path over
`bytes` and `hyper`.

## Target-level selection limitations

The current cargo-anvil catalog supports only package-level Miri exclusion:

```toml
[package.metadata.anvil.miri]
exclude = true
```

It does not expose supported package metadata for excluding individual test
targets. Consequently, `--all-features --tests` may compile and replay empty
Loom or Bolero executables whose source is disabled by `cfg(loom)` or
`cfg(not(miri))`. Do not hand-edit generated `justfiles/anvil/**` to remove
them. Target-level omission requires a cargo-anvil catalog enhancement.

The Bolero recipe likewise has no supported manifest target-exclusion metadata;
on Linux it discovers every `bolero::check!` target and gives each a 60-second
libFuzzer smoke campaign. Repository code must not silently shorten or remove
attacker-facing and unsafe-lifecycle campaigns to work around this limitation.

## Whole-workspace measurement

Use the generated recipe with impact analysis disabled when evaluating the
complete validation boundary:

```powershell
$env:ANVIL_IMPACT = 'off'
just anvil-pr-runtime-analysis
```

Compare warm runs on the same host and toolchain. Report compilation separately
when a clean build is relevant; otherwise, runtime comparisons should not claim
compiler-cache changes as test improvements.
