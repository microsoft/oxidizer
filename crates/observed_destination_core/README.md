<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Destination Core Logo" width="96">

# Observed Destination Core

[![crate.io](https://img.shields.io/crates/v/observed_destination_core.svg)](https://crates.io/crates/observed_destination_core)
[![docs.rs](https://docs.rs/observed_destination_core/badge.svg)](https://docs.rs/observed_destination_core)
[![MSRV](https://img.shields.io/crates/msrv/observed_destination_core)](https://crates.io/crates/observed_destination_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## Observed Destination Core

This crate provides an integration layer between OpenTelemetry and Observed ecosystems.
The central concept is a “destination” that receives log records or metric data.
Each destination implements [`LogDestination`][__link0] or
[`MetricDestination`][__link1] (or both via separate types).

This crate carries only the destination abstractions and the shared building
blocks that destinations are written against. The destinations themselves live
in companion crates such as `observed_destination_stdout`, so an application
depends on exactly the ones it uses — and can implement its own against the
same abstractions when none of them fits.

[`FlushableMetricsProvider`][__link2] extends OpenTelemetry’s meter-provider contract
with explicit flushing. A processor can retain a full
provider, including a raw-measurement forwarding provider, without depending
on a particular SDK implementation or deriving ownership from cached instruments.
Its flush method uses [`DestinationResult`][__link3] so implementations need not
expose an OpenTelemetry SDK’s error type.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_destination_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbHwtXGd867-ob3nWM5uSm9h8bnQ2dC0LWWk4br65QPJ4at05hZIGCeBlvYnNlcnZlZF9kZXN0aW5hdGlvbl9jb3JlZTAuMS4w
 [__link0]: https://docs.rs/observed_destination_core/0.1.0/observed_destination_core/trait.LogDestination.html
 [__link1]: https://docs.rs/observed_destination_core/0.1.0/observed_destination_core/trait.MetricDestination.html
 [__link2]: https://docs.rs/observed_destination_core/0.1.0/observed_destination_core/?search=FlushableMetricsProvider
 [__link3]: https://docs.rs/observed_destination_core/0.1.0/observed_destination_core/type.DestinationResult.html
