<div align="center">
 <img src="./logo.png" alt="Data Privacy Core Logo" width="96">

# Data Privacy Core

[![crate.io](https://img.shields.io/crates/v/data_privacy_core.svg)](https://crates.io/crates/data_privacy_core)
[![docs.rs](https://docs.rs/data_privacy_core/badge.svg)](https://docs.rs/data_privacy_core)
[![MSRV](https://img.shields.io/crates/msrv/data_privacy_core)](https://crates.io/crates/data_privacy_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Core data classification types and traits.

The `data_privacy_core` crate contains the trait definitions and the [`DataClass`][__link0] type
with no support for `#[derive()]` or attribute macros.

In crates that use `#[taxonomy]`, `#[classified]`, `#[derive(RedactedDebug)]`, or
`#[derive(RedactedDisplay)]`, you must depend on the **[`data_privacy`][__link1]**
crate, not `data_privacy_core`.

In crates that hand-write implementations of data privacy traits, or only use them as trait
bounds, depending on `data_privacy_core` is permitted. But `data_privacy` re-exports all of
these traits and can be used for this use case too. **If in doubt, disregard `data_privacy_core`
and always use `data_privacy`.**

## Contents

* [`DataClass`][__link2] - identifies a data class within a taxonomy
* [`Classified`][__link3] - trait for types that hold classified data
* [`Redactor`][__link4] - trait for types that can apply redaction
* [`RedactedDebug`][__link5] / [`RedactedDisplay`][__link6] / [`RedactedToString`][__link7] - redaction-aware formatting traits


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/data_privacy_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbJMSGY2z7YbEblsBSe-58K48b62Bomn7PG1Ebw8HBurz5KcZhZIGCcWRhdGFfcHJpdmFjeV9jb3JlZTAuMS4y
 [__link0]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=DataClass
 [__link1]: https://docs.rs/data_privacy
 [__link2]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=DataClass
 [__link3]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=Classified
 [__link4]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=Redactor
 [__link5]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=RedactedDebug
 [__link6]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=RedactedDisplay
 [__link7]: https://docs.rs/data_privacy_core/0.1.2/data_privacy_core/?search=RedactedToString
