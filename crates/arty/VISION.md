# Arty Vision

Arty is an application runtime for teams that want familiar async Rust with
stronger efficiency, testability, and operational insight. These are product
commitments and design goals; some, including dynamic scaling, describe the
destination rather than functionality that is complete today.

## Familiar

Arty APIs should feel familiar to Tokio users so migration is straightforward.
Familiarity is a bridge, not a constraint: Arty will choose a better design when
there is a clear reason not to copy Tokio.

## Fast

Arty aims to outperform Tokio on relevant workloads. Performance claims must be
supported by reproducible benchmark evidence.

## Efficient

Arty should consume fewer resources than Tokio. The intended default is a single
thread at idle, with dynamic scaling that adapts to demand so most customers do
not need to predict resource requirements in advance. Expert users retain
explicit controls when they need them.

## Compatible

Arty provides an easy, practical compatibility and migration path for
applications and libraries built around Tokio.

## Testable

Testing is a first-class runtime capability. Arty includes deterministic
facilities for reliable tests and remains compatible with Miri.

## Observable

Telemetry and actionable runtime insights are built in, not added as an
afterthought.

## Wide platform support

Arty supports all major platforms. Windows and Linux receive first-class
performance engineering and validation.

## Stable APIs

For the next five years, Arty and designated important libraries built on it
commit to API stability, with no breaking changes during that compatibility
window.
