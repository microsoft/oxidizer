# `msvc_spectre_libs` implementation

This document describes how the crate realizes the contract recorded in
[design.md](design.md): the host/target model, the discovery flow, how the
decision is separated from its effects, flag parsing, path handling, directive
emission, and diagnostic aggregation.

Everything here is a replaceable implementation choice. Where a choice is
load-bearing for a contractual promise, this document says so and points at the
relevant part of the design.

## 1. Layout

The work is split across two packages.

```text
crates/msvc_spectre_libs/
  build.rs           // the adapter: reads the world, applies the plan
  src/lib.rs         // crate documentation; no public API
  docs/design.md
  docs/implementation.md

crates/msvc_spectre_libs_build/
  src/plan.rs        // the decision, as a pure function over captured inputs
  src/toolchain.rs   // the two capabilities the plan cannot capture up front
  src/resolve.rs     // architecture mapping, variable names, path layout
  src/flags.rs       // CARGO_ENCODED_RUSTFLAGS decoding, requirement checking
```

`msvc_spectre_libs` is what a consumer depends on, and it is the only thing that
has an effect at build time. It exposes **no Rust API**: its library target
exists to carry documentation, so that nothing about the mechanism is frozen into
a versioned public surface that consumers could come to rely on.

`msvc_spectre_libs_build` is the build-time companion package, following the same
pattern as `routerama_build` elsewhere in this repository. It holds every rule
worth testing and nothing that touches the environment, the filesystem, or the
registry.

### 1.1 Why two packages

A build script cannot depend on the library it belongs to -- that would be
circular -- so logic shared between the two must either be duplicated,
`#[path]`-included, or factored into a third package.

An earlier revision used `#[path]` inclusion. It works, but it compiles the same
files twice, leaves the `pub` items unreachable in the build-script copy (needing
an `unreachable_pub` suppression), and gives the shared code no manifest of its
own. Splitting the package removes all three: each file is compiled once, owned
by exactly one package, with its own dependencies and its own lints.

### 1.2 The planner/adapter boundary

`plan::plan` is a pure function. It takes a `BuildEnvironment` -- a plain struct
of already-read environment values -- and a `&dyn Toolchain`, and returns a
`Plan`: the list of `rerun-if-env-changed` variables, the link-search directories
to emit, and any diagnostics. It prints nothing, reads nothing, and exits
nothing.

`build.rs` is the adapter. It captures the environment with
`BuildEnvironment::from_env`, calls `plan`, and then performs the plan's effects:
printing `cargo:` directives and, under the `error` feature, exiting non-zero.
Capturing the environment can itself fail (Cargo-guaranteed variables missing or
unreadable), so `from_env` returns a `Result` and the adapter funnels that error
into `Plan::reporting`, which produces a plan whose only content is that
diagnostic. There is exactly one reporting path.

`Toolchain` is a port with two methods, because two inputs to the decision cannot
be captured in advance: whether a candidate directory exists, and where `cl.exe`
is for the target. Both are only consulted along paths that earlier fallbacks did
not already settle, so eagerly evaluating them would probe the registry on builds
that do not need it. `SystemToolchain` is the real implementation --
`Path::is_dir` and `cc::windows_registry::find_tool` -- and the tests supply a
fake whose answers are fixed by a table.

Errors are `ohno::AppError`, the repository's application error type, rather than
bare strings. `Display` renders the message alone, so the text of a
`cargo:warning=` line is unchanged.

## 2. Host versus target

A build script is compiled for and executed on the **host**. Any `cfg!` macro
inside it therefore describes the machine running the build, not the machine the
artifact is being built for. Gating on `cfg!(target_os = "windows")` would
silently do the wrong thing in both directions: it would skip a Windows MSVC
target cross-compiled from Linux, and it would run on a Windows host building for
Linux.

The script therefore reads the target from the environment Cargo provides:

- `CARGO_CFG_TARGET_OS` and `CARGO_CFG_TARGET_ENV` gate the whole script; both
  must be `windows` and `msvc` respectively, otherwise it returns immediately.
  This is what makes design.md section 2.3 hold.
- `CARGO_CFG_TARGET_ARCH` selects the Spectre library subdirectory.
- `TARGET` supplies the full triple used to build target-suffixed environment
  variable names and to look up a toolchain through `cc::windows_registry`.

`TARGET`, `CARGO_CFG_TARGET_OS`, `CARGO_CFG_TARGET_ENV` and
`CARGO_CFG_TARGET_ARCH` are all required: Cargo sets every one of them for a
build script, so an absence means this is not running as one, and
`BuildEnvironment::from_env` fails outright rather than guessing.

## 3. Resolution flow

`plan_link_search` tries three sources in the order design.md section 2.4
and 2.5 promise.

**Override.** `resolve::override_var_name(target)` produces
`MSVC_SPECTRE_LIB_DIR_<target>` with the triple's hyphens replaced by
underscores, matching the convention Cargo and `cc` use for target-suffixed
variables. If either it or the unsuffixed `MSVC_SPECTRE_LIB_DIR` is set, that
value is used and resolution stops -- either successfully, or with a
diagnostic naming the directory that does not exist. It never falls through to
discovery, per design.md section 2.4.

Both are captured as `EnvValue`, a three-state value -- `Absent`, `NotUnicode`,
`Present` -- rather than an `Option`. A `cargo:` directive is a UTF-8 text line,
so a path that is not valid Unicode could only be emitted lossily -- pointing the
linker at a directory other than the one `is_dir` validated. Treating it as unset
would be worse still: resolution would fall through to discovery and quietly link
the unmitigated CRT. It is therefore a hard diagnostic, like an unreadable
required-link-args variable. `push_link_search` enforces the same rule for every
source, rejecting any directory it cannot render as UTF-8. Keeping the third
state in the captured data, rather than resolving it during the read, is what
lets the planner decide it and the tests exercise it.

A `cargo:` directive is also a *single* line, and Cargo reads a build script's
output line by line: a value carrying a line break would end its directive early
and leave the remainder to be read as another directive -- a
`cargo:rustc-link-arg` smuggled in through a directory name, say. Every string
the plan carries is therefore checked before it is written out.
`push_link_search` refuses a directory that spans more than one line, the
planner refuses a target triple that does (the triple is what the suffixed
variable names are built from), and each diagnostic is flattened to one line as
it is recorded. The `Plan` fields document these guarantees, and a test asserts
them over a whole plan rather than one value at a time.

**`VCToolsInstallDir`.** A Visual Studio developer command prompt, and anything
that has run `vcvars`, exports this variable pointing at the MSVC build tools
root -- exactly the directory the Spectre libraries sit beneath. Using it avoids
registry probing entirely in the common developer case, and avoids the
parent-directory climbing described below. A `VCToolsInstallDir` that does not
contain the libraries is not itself an error; the script falls through to
registry discovery, which may find a different installation.

**Registry discovery.** `Toolchain::find_cl_exe` locates an MSVC installation
for the selected target; `SystemToolchain` implements it with
`cc::windows_registry::find_tool(target, "cl.exe")`. That lookup is target-aware,
so it works for cross-architecture builds, and `cc` is already an established
dependency for exactly this kind of lookup. It sits behind the port because it is
the one input that must be evaluated lazily -- probing the registry on a build
whose override already answered the question would be pure cost.

`find_cl_exe` returns the path of `cl.exe`, which lives at
`<root>\bin\Host<arch>\<arch>\cl.exe`. `toolchain_root` recovers the root by
dropping the file name and the three directories above it. The count is
expressed as chained `parent()` calls with a comment naming each component
rather than as an ordinal, so that the comment stays true if the chain is
edited.

## 4. Path construction

`resolve::spectre_lib_dir(root, arch)` joins `lib`, `spectre`, and the
architecture directory beneath the toolchain root. It takes a `SpectreArch`
rather than a string so that an unmappable architecture cannot reach path
construction at all; the mapping failure is handled once, at the call site, with
a diagnostic that names the architecture.

`SpectreArch` is `#[non_exhaustive]`, because Visual Studio may add
architectures. `from_target_arch` maps Rust's `CARGO_CFG_TARGET_ARCH` values
(`x86_64`, `x86`, `aarch64`, `arm`) to the toolchain's directory names (`x64`,
`x86`, `arm64`, `arm`); the two vocabularies differ, and keeping the translation
in one typed place keeps the difference from leaking into the build script.
`aarch64` covers Arm64EC as well, which uses the same mitigated libraries.

Paths are built with `Path::join` throughout; no separator is ever written by
hand.

## 5. Emitting the directive

`push_link_search` checks that the directory exists and, if so, records it on the
plan; `build.rs` prints one `cargo:rustc-link-search=native=<dir>` line per
recorded directory. It returns whether it recorded anything, which is how each
resolution step decides whether to continue to the next.

The directive is emitted unconditionally for an existing directory. An earlier
revision suppressed it when the same directory already appeared in the `LIB`
environment variable, which was wrong on two counts. Explicit `-L` search paths
are consulted *before* `LIB` and in the order given, so mere membership in `LIB`
does not mean the mitigated directory wins -- an ordinary CRT directory earlier
in `LIB` would. And the comparison case-folded, which conflates genuinely
distinct directories on a case-sensitive host. A duplicate search path costs the
linker nothing, so emitting unconditionally is both simpler and correct.

## 6. Required-linker-argument verification

`plan_required_link_args` implements design.md section 2.7.

The captured `BuildEnvironment` carries `MSVC_SPECTRE_REQUIRED_LINK_ARGS_<target>`
and the unsuffixed `MSVC_SPECTRE_REQUIRED_LINK_ARGS` as `EnvValue`s, and the
first that is `Present` wins. The three-state capture matters here too:
`Absent` means the check is disabled and is not an error, but `NotUnicode` means
the integrator opted into a check whose value cannot be read. Treating the latter
as "unset" would silently skip a check that was explicitly requested, which
violates the never-silently-downgrade tenet, so it becomes a diagnostic.

The value is a `;`-separated list, chosen because `;` cannot appear in an MSVC
linker argument and because it is the separator Windows build systems already
use for list-valued variables. Blank entries are dropped, so a trailing or
doubled separator is harmless.

The observed flags come from `CARGO_ENCODED_RUSTFLAGS`, which Cargo sets for
build scripts with the final resolved flag list, `0x1f`-separated. That variable
is the only place the merged result of `.cargo/config.toml` and any ambient
`RUSTFLAGS` is visible, which is precisely the divergence the check exists to
catch. Its absence is not an error: an older or differently configured Cargo
simply gives nothing to check, and a diagnostic would have no basis.

`flags::missing_required_link_args` decodes the separated list and accepts a
requirement supplied through any of the spellings `rustc` recognizes:
`-Clink-arg=X`, `-C link-arg=X` (as two arguments), and `-Clink-args="X Y"`,
where a single option carries space-separated arguments. Matching only one
spelling would produce a false diagnostic against a correctly configured build.

The parser walks the decoded arguments with an iterator and `while let`, never
with a manual index. That is deliberate: an index-based loop with a manual
increment can be mutated into an infinite loop, which turns a mutation-testing
run into a timeout rather than a caught mutant. The iterator form has no such
mutation.

## 7. Diagnostics

`plan` runs both steps and collects the diagnostics of each. Neither step
short-circuits the other, so a build that has both a missing toolchain and a
dropped linker argument reports both at once instead of revealing the second only
after the first is fixed.

Every failure is an `ohno::AppError` built at the point where the context is
known, and every message names the environment variable that would fix it. The
adapter prints them as `cargo:warning=` lines first; `println!` is line-buffered,
so they reach Cargo before any exit. Under the `error` feature, a non-zero exit
follows once every message has been printed. Under the default configuration
nothing else happens and the build proceeds, per design.md section 2.6.

The `exit` import is itself `#[cfg(feature = "error")]`, so the default build
does not carry an unused import.

## 8. Rebuild triggers

The plan records `rerun-if-env-changed` for every environment variable it
consults: both override variables, `VCToolsInstallDir`, both requirement
variables, and `CARGO_ENCODED_RUSTFLAGS`. The requirement variables are declared
even when unset, so that *setting* one later triggers a re-run rather than
leaving a stale success cached.

Only variables that are actually read are declared. Over-declaring would rebuild
the crate on unrelated environment churn; under-declaring would let a
configuration change go unnoticed.

## 9. Testing

`plan` is a pure function over a captured `BuildEnvironment` and a `Toolchain`,
so the whole policy is table-driven unit tested against a fake toolchain: each
resolution source in turn, the non-Unicode variant of every variable, a
`VCToolsInstallDir` that does not contain the libraries, an unmappable
architecture, a non-Windows and a non-MSVC target, each accepted spelling of a
required linker argument, and the case where both steps fail at once. The
assertions are over the returned `Plan`, so they check the exact directives that
would be printed without printing anything.

`resolve` and `flags` remain pure functions over strings and paths and are unit-
and doc-tested directly: architecture mapping including the unmapped case,
variable-name construction for both the suffixed and unsuffixed forms,
requirement-list splitting including blank entries, and flag decoding for each
accepted spelling.

`build.rs` itself is not unit-tested. Everything it does other than I/O now lives
in `plan`, so what remains is capturing the environment, printing the plan's
directives, and exiting; the end-to-end behavior that matters is exercised by
building a consumer and inspecting the result with `dumpbin /headers`.
`SystemToolchain` is likewise untested for the same reason -- it is two calls
into `std` and `cc`, with the fake standing in for it everywhere the decision is
checked.

Every `mod tests` block carries `#[cfg_attr(coverage_nightly, coverage(off))]`, per
repository convention, so test code does not inflate coverage of the code under
test.
