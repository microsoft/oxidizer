<div align="center">
 <img src="./logo.png" alt="Fetch Options Logo" width="96">

# Fetch Options

[![crate.io](https://img.shields.io/crates/v/fetch_options.svg)](https://crates.io/crates/fetch_options)
[![docs.rs](https://docs.rs/fetch_options/badge.svg)](https://docs.rs/fetch_options)
[![MSRV](https://img.shields.io/crates/msrv/fetch_options)](https://crates.io/crates/fetch_options)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Configuration options for HTTP client transport behavior.

This crate provides types for configuring various aspects of HTTP connections,
including connection keep-alive behavior, connection pooling, and HTTP version support.

## Example

```rust
use std::time::Duration;

use fetch_options::{ConnectionLifetime, ConnectionPoolOptions};

let pool = ConnectionPoolOptions::default()
    .max_connections(64)
    .connection_idle_timeout(Duration::from_secs(90))
    .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(300)));
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_options">source code</a>.
</sub>

