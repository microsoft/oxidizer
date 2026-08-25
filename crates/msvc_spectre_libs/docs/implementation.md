# `msvc_spectre_libs` implementation

This document describes how the crate realizes the contract recorded in
[design.md](design.md): the host/target model, the discovery flow, how source is
shared between the library and its build script, flag parsing, path handling,
directive emission, and diagnostic aggregation.

Everything here is a replaceable implementation choice. Where a choice is
load-bearing for a contractual promise, this document says so and points at the
relevant part of the design.

## 1. Layout

```text
crates/msvc_spectre_libs/
  build.rs           // the only thing that has an effect at build time
  src/lib.rs         // crate documentation; re-exports the two modules
  src/resolve.rs     // architecture mapping, variable names, path layout
  src/flags.rs       // CARGO_ENCODED_RUSTFLAGS decoding, requirement checking
  docs/design.md
  docs/implementation.md
```

`lib.rs` contains no logic. The crate's entire runtime behavior lives in
`build.rs`; the library exists to publish the naming and mapping rules
(design.md section 2.8) and to carry the documentation.

### 1.1 Sharing source with the build script

`build.rs` pulls `src/resolve.rs` and `src/flags.rs` in with `#[path]` module
declarations rather than depending on the library. A build script cannot depend
on the library it belongs to -- that would be circular -- so the alternatives are
to duplicate the logic, to `#[path]`-include it, or to factor it into a third
package that both depend on.

`#[path]` inclusion was chosen because the shared surface is small and
self-contained: pure string and path manipulation with no state and no I/O. The
cost is that these two files are compiled twice, once into the build-script
binary and once into the library, and that the `pub` items are unreachable in
the build-script copy. The `unreachable_pub` lint is silenced there with an
`#[expect]` carrying that reason, so the suppression is scoped to exactly the two
inclusions and will itself warn if the situation changes.

Extracting a third package remains the option to reach for if the shared surface
grows beyond pure functions, or if it starts to need dependencies of its own.

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

`TARGET` and `CARGO_CFG_TARGET_ARCH` are read with `expect`, because Cargo
always sets them for a build script; their absence indicates a broken invocation
rather than a user error, and a panic there is a more useful signal than a
warning. `CARGO_CFG_TARGET_OS` and `CARGO_CFG_TARGET_ENV` are read with
`unwrap_or_default`, since an empty string simply fails the gate.

## 3. Resolution flow

`add_spectre_link_search` tries three sources in the order design.md section 2.4
and 2.5 promise.

**Override.** `resolve::override_var_name(target)` produces
`MSVC_SPECTRE_LIB_DIR_<target>` with the triple's hyphens and periods replaced by
underscores, matching the convention Cargo and `cc` use for target-suffixed
variables. If either it or the unsuffixed `MSVC_SPECTRE_LIB_DIR` is set, that
value is used and the function returns -- either successfully, or with a
diagnostic naming the directory that does not exist. It never falls through to
discovery, per design.md section 2.4.

Both are read with `env::var_os` rather than `env::var`, because a directory path
need not be valid Unicode and there is no reason to reject one that is not.

**`VCToolsInstallDir`.** A Visual Studio developer command prompt, and anything
that has run `vcvars`, exports this variable pointing at the MSVC build tools
root -- exactly the directory the Spectre libraries sit beneath. Using it avoids
registry probing entirely in the common developer case, and avoids the
parent-directory climbing described below. A `VCToolsInstallDir` that does not
contain the libraries is not itself an error; the script falls through to
registry discovery, which may find a different installation.

**Registry discovery.** `cc::windows_registry::find_tool(target, "cl.exe")`
locates an MSVC installation for the selected target. It is target-aware, so it
works for cross-architecture builds, and `cc` is already an established
dependency for exactly this kind of lookup.

`find_tool` returns the path of `cl.exe`, which lives at
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

`emit_link_search` checks that the directory exists and, if so, prints
`cargo:rustc-link-search=native=<dir>`. It returns whether it emitted, which is
how each resolution step decides whether to continue to the next.

The directive is emitted unconditionally for an existing directory. An earlier
revision suppressed it when the same directory already appeared in the `LIB`
environment variable, which was wrong on two counts. Explicit `-L` search paths
are consulted *before* `LIB` and in the order given, so mere membership in `LIB`
does not mean the mitigated directory wins -- an ordinary CRT directory earlier
in `LIB` would. And the comparison case-folded, which conflates genuinely
distinct directories on a case-sensitive host. A duplicate search path costs the
linker nothing, so emitting unconditionally is both simpler and correct.

## 6. Required-linker-argument verification

`verify_required_link_args` implements design.md section 2.7.

`required_link_args_value` reads `MSVC_SPECTRE_REQUIRED_LINK_ARGS_<target>` and
then the unsuffixed `MSVC_SPECTRE_REQUIRED_LINK_ARGS`, returning the first that
is set. Both are matched on `env::VarError` explicitly rather than through
`ok()`: `NotPresent` means the check is disabled and is not an error, but
`NotUnicode` means the integrator opted into a check whose value cannot be read.
Treating the latter as "unset" would silently skip a check that was explicitly
requested, which violates the never-silently-downgrade tenet, so it becomes a
diagnostic.

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

`main` runs both steps, collects their `Result`s, and only then reports. Neither
step short-circuits the other, so a build that has both a missing toolchain and a
dropped linker argument reports both at once instead of revealing the second only
after the first is fixed.

Every failure is a `String` built at the point where the context is known, and
every message names the environment variable that would fix it. Messages are
printed as `cargo:warning=` lines first; `println!` is line-buffered, so they
reach Cargo before any exit. Under the `error` feature, a non-zero exit follows
once both messages have been printed. Under the default configuration nothing
else happens and the build proceeds, per design.md section 2.6.

The `exit` import is itself `#[cfg(feature = "error")]`, so the default build
does not carry an unused import.

## 8. Rebuild triggers

The script prints `cargo:rerun-if-env-changed` for every environment variable it
consults: both override variables, `VCToolsInstallDir`, both requirement
variables, and `CARGO_ENCODED_RUSTFLAGS`. The requirement variables are declared
even when unset, so that *setting* one later triggers a re-run rather than
leaving a stale success cached.

Only variables that are actually read are declared. Over-declaring would rebuild
the crate on unrelated environment churn; under-declaring would let a
configuration change go unnoticed.

## 9. Testing

The two modules are pure functions over strings and paths, so they are unit- and
doc-tested directly, with no filesystem or environment dependency: architecture
mapping including the unmapped case, variable-name construction for both the
suffixed and unsuffixed forms, requirement-list splitting including blank
entries, flag decoding for each accepted spelling, and the missing/complete
outcomes.

`build.rs` itself is not unit-tested. It is I/O against the environment,
filesystem, and registry, and its logic is thin glue over the tested modules; the
behavior that matters is exercised end to end by building a consumer and
inspecting the result with `dumpbin /headers`.

Both `mod tests` blocks carry `#[cfg_attr(coverage_nightly, coverage(off))]`, per
repository convention, so test code does not inflate coverage of the code under
test.
