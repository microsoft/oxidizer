<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Io Logo" width="96">

# Seismograph Io

[![crate.io](https://img.shields.io/crates/v/seismograph_io.svg)](https://crates.io/crates/seismograph_io)
[![docs.rs](https://docs.rs/seismograph_io/badge.svg)](https://docs.rs/seismograph_io)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_io)](https://crates.io/crates/seismograph_io)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Low-overhead I/O event instrumentation for [`seismograph`][__link0].

[`Resource`][__link1] lazily acquires its identity when an enabled I/O event is first
recorded. [`Operation`][__link2] pairs start and finish events without reading a
clock or allocating any identity outside [`seismograph::record`][__link3].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_io">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEG8L0BlFx7qUFG6hOHGrQwgjBG76tlxTGGiDFG3EvcFav2ocvYWSCgmtzZWlzbW9ncmFwaGUwLjEuMIJuc2Vpc21vZ3JhcGhfaW9lMC4xLjA
 [__link0]: https://crates.io/crates/seismograph/0.1.0
 [__link1]: https://docs.rs/seismograph_io/0.1.0/seismograph_io/struct.Resource.html
 [__link2]: https://docs.rs/seismograph_io/0.1.0/seismograph_io/struct.Operation.html
 [__link3]: https://docs.rs/seismograph/0.1.0/seismograph/?search=record
