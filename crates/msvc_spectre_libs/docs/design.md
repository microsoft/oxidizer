# `msvc_spectre_libs` design

This document describes the user-visible contract and the design tenets of the
`msvc_spectre_libs` crate. The internal strategy that realizes this contract --
host/target gating, discovery flow, source sharing, flag parsing, path handling,
and diagnostic aggregation -- is documented separately in
[implementation.md](implementation.md).

## 1. Purpose

Microsoft Visual C++ ships two copies of its C runtime import libraries: the
ordinary ones, and a set built with `/Qspectre`, which inserts speculative-load
hardening barriers into the runtime code. Only the second set satisfies the
security bar that requires all linked code, including the runtime, to carry
Spectre variant 1 mitigations.

`rustc` does not select the mitigated set. Nothing in Cargo, in a Rust target
specification, or in `RUSTFLAGS` names those libraries, and no stable per-crate
attribute reaches the linker's library search order. Every project that must
ship a mitigated binary therefore has to arrange the search path itself.

This crate exists so that arrangement is a dependency line rather than a piece
of build-system code duplicated per repository.

## 2. Contract

### 2.1 Adding the dependency is the entire integration

Declaring the crate under `[dependencies]` is sufficient. The crate exports no
items that a consumer is expected to call, and requires no source change,
attribute, or macro invocation. Its effect is produced entirely by its build
script.

The dependency must be a normal dependency, not a build-dependency. A
build-dependency belongs to the host build graph, so its link-search directive
would apply to the consumer's own build script rather than to the artifact being
shipped.

### 2.2 The effect is a link search path, and it propagates

The build script emits `cargo:rustc-link-search` for the directory holding the
Spectre-mitigated CRT import libraries. Cargo propagates that directive to every
crate that links this one, transitively, up to and including the final
executable or shared library. That propagation is why the crate can work as a
leaf dependency of an arbitrarily deep graph.

The crate never selects *which* CRT libraries are linked; it changes only where
the linker finds them. Linking the runtime remains the responsibility of the
normal Rust MSVC target.

### 2.3 Every non-Windows-MSVC target is a no-op

The crate is safe to depend on unconditionally, including in a workspace that
also builds for Linux, macOS, or a `*-windows-gnu` target. On any target that is
not Windows MSVC, the build script does nothing at all: no directive, no
diagnostic, no failure. Cross-compiling *to* Windows MSVC from another host is
supported and follows the ordinary path.

Because the target boundary is part of the contract, consumers do not need
`[target.'cfg(...)'.dependencies]` gating.

### 2.4 An explicit override always wins

A build system that already knows the toolchain layout -- because it provisions
MSVC from a package feed, or pins a specific toolchain version -- can supply the
directory directly:

- `MSVC_SPECTRE_LIB_DIR_<target>`, for example
  `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`, and
- `MSVC_SPECTRE_LIB_DIR`, which applies to every target.

The target-specific variable takes precedence over the target-agnostic one, and
both take precedence over any discovery the crate would otherwise perform. An
override that names an existing directory is used verbatim; no validation of its
contents is performed, because the crate cannot know what a given toolchain
version ships.

An override that does *not* name an existing directory is a diagnostic. It is
never silently ignored in favor of discovery: an integrator who set the variable
stated an intent, and quietly substituting a different directory would produce a
binary that does not match that intent.

### 2.5 Discovery is a convenience, not a guarantee

Absent an override, the crate tries to find the libraries itself, first from a
developer environment and then through the Windows registry. Discovery is
best-effort. The Spectre libraries are an optional Visual Studio component, and
a perfectly valid installation may not have them; the crate reports that rather
than pretending otherwise.

### 2.6 Failure policy is the consumer's choice

By default, an unresolvable directory produces a `cargo:warning` and the build
continues. Enabling the `error` feature turns the same condition into a build
failure.

The default is permissive because a crate deep in a dependency graph should not
break builds of consumers who never asked for this hardening. The `error`
feature exists because a release pipeline that must ship a mitigated binary
needs the opposite: an absent mitigation is worse than a failed build. Making it
a feature keeps the policy with the party that owns the compliance requirement.

### 2.7 Required-linker-argument verification is opt-in

Some hardening arguments cannot be delivered by this crate at all.
`cargo:rustc-link-arg` from a build script applies only to the emitting
package's own artifacts and, unlike `cargo:rustc-link-search`, does not
propagate to dependents. Arguments such as `/CETCOMPAT` must therefore come from
`.cargo/config.toml` or `RUSTFLAGS`.

That delivery path has a failure mode with no natural diagnostic: a `RUSTFLAGS`
environment variable *replaces* the `target.<triple>.rustflags` table rather than
merging with it, so an unrelated ambient `RUSTFLAGS` silently discards the
configured arguments. The build succeeds and the artifact is unhardened.

To make that visible, a consumer lists the arguments it requires in

- `MSVC_SPECTRE_REQUIRED_LINK_ARGS_<target>`, or
- `MSVC_SPECTRE_REQUIRED_LINK_ARGS`,

again with the target-specific variable taking precedence. The build script then
checks the flags Cargo reports and diagnoses any that are absent, under the same
failure policy as section 2.6.

The check is off by default, and the crate ships no built-in list of required
arguments. Which arguments are mandatory depends on the toolchain version and on
the compliance regime the consumer is subject to, neither of which this crate
can determine.

### 2.8 The consumed crate has no public API

`msvc_spectre_libs` is added as a dependency for its build-script effect alone.
It therefore exposes no Rust items: nothing about the mechanism is frozen into a
versioned surface, and a consumer cannot come to depend on an internal detail
that the crate would then owe compatibility for.

The naming and mapping rules the build script uses -- override and requirement
variable names, the target-architecture to Spectre-architecture mapping, and the
`lib\spectre\<arch>` layout -- live in the companion `msvc_spectre_libs_build`
package. A build system that computes these values ahead of time can depend on
that package directly and derive them from the same source as the crate, instead
of re-deriving a string format that could drift. Making that an explicit, separate
dependency keeps the contract intentional rather than incidental.

## 3. Design tenets

**Configuration assurance, not artifact assurance.** Everything this crate
checks is a property of the build's *configuration*: whether a directory
resolved, and whether the configured arguments appear in the flags Cargo reports
to `rustc`. It never observes the linker invocation and never inspects the
produced binary. A successful build -- including with `error` enabled --
establishes that the build was configured to link the mitigated runtime, not
that the artifact provably did. Artifact-level evidence requires post-link
inspection such as `dumpbin /headers`, and this crate does not replace it. The
crate's documentation states this boundary explicitly so that it is not mistaken
for a stronger guarantee than it is.

**Never silently downgrade.** Any condition that would result in a build that
looks hardened but is not -- a missing directory, an override that does not
exist, a required argument that did not arrive -- produces output. The severity
of that output is the consumer's choice; its existence is not.

**No policy of our own.** The crate does not decide which linker arguments a
consumer needs, does not enable `error` by default, and does not fail a build
for a target it was never meant to harden. It supplies a mechanism and leaves
the policy to the party that owns the requirement.

**Explicit configuration beats discovery.** Discovery exists to make the common
developer case work without setup, but it is the last resort. A build system
that states a path gets that path.

**Report everything a build can learn at once.** When both the search-path step
and the verification step have something to say, both are reported in a single
build, rather than forcing the consumer to fix one problem to discover the next.

## 4. Non-goals

- **Compiling anything.** The crate builds no C or C++ code and links no object
  files of its own. It has a `cc` build-dependency only for the registry-based
  toolchain lookup that `cc::windows_registry` provides.
- **Applying `/Qspectre` to C code.** That is a property of how the C code is
  compiled and is the responsibility of whatever compiles it.
- **Mitigating Rust code.** The mitigated libraries are the MSVC C runtime.
  `rustc` does not offer an equivalent speculative-load-hardening switch, and
  this crate does not simulate one.
- **Verifying the output binary.** See the assurance boundary above.
- **Supporting `*-windows-gnu`.** Those targets do not use the MSVC CRT.
