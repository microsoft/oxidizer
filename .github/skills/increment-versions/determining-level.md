# Choosing a semantic impact

Use this guide for every publishable package in the release-plan report. The decision concerns
the complete released change since the package's version anchor, including changes already
pending release.

The skill's [assessment model](SKILL.md#assessment-model) defines release history,
the merge target, package anchors, semantic impacts and version plans.

## Evidence to inspect

Read the package entry in `report.json`, its referenced diff when present, the package's
`Cargo.toml`, the workspace `Cargo.toml`, and the decisions for its dependencies. Direct
`[package]` and dependency-table edits appear in the package's released-content diff alongside
any other changed published files. Values inherited through `.workspace = true` instead
appear in the report's `changed` array with `source: "inherited"`, and locked dependency
identities appear with `source: "lockfile"`; neither has a package diff.

Evaluate every `source: "inherited"` entry for every package. A workspace-level edit affects
each package that inherits the edited field, so a package can require an increment on an
inherited value alone, without appearing in any original file diff. Read the package's own
`Cargo.toml` to establish which fields it inherits rather than assuming a workspace-wide
convention.

Evaluate every `source: "lockfile"` entry the same way. A package with an installable binary
releases its resolved dependency closure, so a locked dependency change is a released-content
change even though it produces no package diff. Judge the consumer impact of the moved
dependency; it establishes at least `patch`.

Library-only packages have no lockfile-based release effects, even when they contain executable
examples, benchmarks, tests, or build scripts. Published source and manifest requirement changes
still establish their normal release requirements.

A package that is already `pending-release` is judged by these same criteria. Its existing
version movement is retained, but it does not replace analysis of the accumulated changes: an
increment that no longer covers them is raised.

`cargo-semver-checks` detects part of the Rust API surface. Its per-package summary establishes
the floor described in the skill's decision stage. No summary means the tool could not
determine a required version increment; it does not establish that the package is compatible.

## Re-exported APIs

A **public facade** is a package consumers use while another package supplies its
implementation. A private implementation can provide public items through that
facade without offering an independently supported API of its own. `private-api`
does not exclude those exposed effects from semantic assessment.

Trace changed definitions and behavior through each affected public package, including
nested re-exports. Inspect methods, trait implementations, bounds and types reachable
through exported items, not only the names in `pub use`. A facade's unchanged source
or report status does not prove that its dependency-supplied API is unchanged.
Use source changes, documented promises and relevant consumer tests.

The checker has a [cross-crate limitation](https://github.com/obi1kenobi/cargo-semver-checks/issues/638):
dependency definitions are not fully present in a facade's rustdoc JSON. A passing facade
comparison therefore does not establish compatibility of those definitions.
The external-types check acknowledges permitted exposure; it is not a compatibility check.
Version groups align versions, and `public_origins` tracks defining packages for
type-identity propagation. Neither substitutes for this source-level assessment.

Classify the facade's effect independently: removal of an exposed method or stricter
requirements are breaking; a genuinely compatible new exposed method is nonbreaking;
a compatible behavior correction is patch. Unexported implementation items have no
independent facade API promise, but changes to them can still affect promised public behavior.
Do not treat a change to an implementation-only `pub` item as a public break merely
because it has Rust visibility, or copy its package's impact blindly to every dependent.

Record the public package, affected promise, chosen impact and supporting evidence.
An implementation can receive `patch` while its facade receives `nonbreaking`; let
the tool align group versions afterward. Obtain missing consumer evidence or report
uncertainty instead of defaulting to compatibility.

## Procedural macros

A procedural macro transforms consumer input into Rust code during compilation.
Cargo identifies its defining library as a `proc-macro` target. Direct comparison
of that target is unsupported, independently of its `private-api` declaration.
An explicit `unsupported-proc-macro` result supplies no semantic floor and does not
justify a `patch` or no-increment decision.

Assess supported invocations: macro availability, accepted syntax, attribute arguments
and helper attributes recognized by derive macros. Assess generated public items,
trait implementations, required bounds, and other obligations imposed on consumer
code. Also assess promised behavior of the generated code even when macro names
and signatures stay unchanged.

Apply the same review through a public facade, including when the defining macro is
private or the facade has no patch. A passing facade comparison is not evidence that
macro invocations or generated code remain compatible. For example, withdrawing a
supported invocation or promised implementation is breaking; adding a compatible
opt-in capability is nonbreaking; a compatible internal refactor is patch.
Calling a change a bug fix does not establish compatibility.

## Breaking

Choose `breaking` when existing consumers may need to change or may observe an incompatible
contract. Examine public API removal or reshaping, stricter input requirements, changed output
or persisted formats, incompatible command-line behavior, and changed semantic guarantees.

For a public package backed by implementation packages, judge the behavior and API exposed
through the public package. Internal handoff changes matter only when they alter that exposed
contract.

Feature gating participates in this judgement. Placing an existing API behind a Cargo feature is
breaking even when that feature is enabled by default, because a consumer building with default
features disabled loses the item. Removing a feature is breaking when it withdraws functionality
or public items a consumer could reach. Adding a new opt-in feature is not breaking; it is a
compatible capability.

A package also takes `breaking` when a defining package in its `public_origins`
releases an incompatible version, whatever its own diff shows. This report field
follows verified allow-list declarations transitively through exposed owners,
including private implementation packages. It does not attribute the identity of
every dependency transporting those types.

An incompatible origin version changes the identity of its types for consumers,
even when its breaking source change touches an unrelated item. Conversely,
sharing a compatible third package's types does not make an unrelated dependency
break propagate. Use the report's origins rather than inferring public exposure
from ordinary dependency edges.

Inference or method-call ambiguity caused solely by coexisting dependency versions'
trait implementations is intentionally considered nonbreaking. This exception does
not cover explicitly exposed trait/type identities or other contract changes.

Only a `breaking` dependency propagates this way. A dependency that adds API compatibly leaves
the exposure intact, so it establishes no more than the `patch` its requirement rewrite already
does **from that version/type-identity effect alone**. This is not a ceiling on the
dependent's own semantic impact. A new public method on a re-exported type is a
new facade capability even when its `pub use` line is unchanged; assess it as
`nonbreaking` when compatible.

## Nonbreaking

Choose `nonbreaking` for a meaningful compatible capability: a new API, supported input,
command, output option, or documented behavior that existing consumers can ignore.

Do not choose this impact merely because implementation volume is large. The impact describes
the consumer-visible change.

## Patch

Choose `patch` for compatible corrections, performance improvements, documentation changes
included in the published package, or internal changes that alter released content without
adding a meaningful consumer-facing capability.

A direct `[package]` metadata change or an inherited `[workspace.package]` change establishes
at least `patch` for every affected package. This rule applies to every package metadata field,
including `rust-version` (the minimum supported Rust version). Combine this minimum with the
package's other evidence and choose the strongest applicable semantic impact.

Treat dependency and feature-table changes separately: analyze the consumer impact of the
resulting dependency or feature behavior rather than assuming every `Cargo.toml` edit is
metadata-only.

A package also takes at least `patch` when applying the plan will rewrite an intra-workspace
requirement inside its own published manifest, which happens whenever it declares a version
requirement on a workspace package another decision moves. Every such requirement names the
version its target declares, so moving the target rewrites the requirement even though it would
still have admitted the new version. Such a package has no released-content change of its own
yet, so the report shows nothing for it; the rewrite arrives with the increment that moves the
dependency. Deciding it in the same pass keeps that package from being left behind at an
already-published version with a changed manifest.

## No increment

Choose no increment only when the complete evidence set supports it: every entry in the package's
`changed` array, its released-content diff when it has one, the changed inherited workspace
fields, the locked dependency changes, and the decisions recorded for its dependencies. Signal
this by omitting the package from `decisions.json`.

Membership of a version group does not change this. A group whose members declare different
versions is realigned mechanically when the plan is generated, even when `consistent` is true
because a member absent from the comparison history is exempt from that consistency check.
Alignment normally uses the highest version any publishable or non-publishable member declares.
The group is patch-incremented when exact alignment would rewrite a dependency inside a
publishable member that otherwise kept a published version, or when the highest version is not a
plain SemVer triplet. The tooling chooses the realignment. Judge each publishable member on its
own released changes and choose no increment when it has none. Do not assess source changes in
`non_publishable_packages` or assign those non-publishable packages a semantic impact.

A publishable package in `report.json.packages` that has no anchor has no comparison release
for this assessment. Do not assign a semantic impact merely because it is new to the release
branch, and do not infer that it has never reached crates.io. Configured mode follows the
[first-publication handoff](SKILL.md#first-publication-handoff): a maintainer manually
publishes its bootstrap version before the first merge and configures Trusted Publishing.
The first merge must carry a higher version for the second publication, the first automated
one. Record that bootstrap and intended automated-release version separately from Git anchors;
version-group expansion may supply the required increase.

Standalone mode has no bootstrap or Trusted Publisher prerequisite. Retain a new
package's intended initial version unless the assessed plan's group/dependency
effects require movement, and record the absence of a comparison anchor in the
version-increment PR. Publication after that PR is outside the standalone workflow.
