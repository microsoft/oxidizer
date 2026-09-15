# Stabilization

Every dependency whose API Arty re-exports or exposes must be stable before Arty stabilizes.
Public I/O integration contracts in `arty_io_core` must also be stable.

Arty's planned runtime surface exposes and uses the telemetry emitter API now provided by
`observed`, including its `Sink` and `emit!` surface. This API previously lived in
`observer_core` and is treated as a highly stable dependency surface. Changes to it must preserve
the compatibility expected of Arty's public runtime API.
