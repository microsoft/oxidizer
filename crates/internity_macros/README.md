<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Internity Macros Logo" width="96">

# Internity Macros

[![crate.io](https://img.shields.io/crates/v/internity_macros.svg)](https://crates.io/crates/internity_macros)
[![docs.rs](https://docs.rs/internity_macros/badge.svg)](https://docs.rs/internity_macros)
[![MSRV](https://img.shields.io/crates/msrv/internity_macros)](https://crates.io/crates/internity_macros)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Derive macros for interner-aware serialization and deserialization in
[`internity`][__link0].

The [`DeserializeIn`][__link1]
and [`SerializeIn`][__link2]
derives thread a reader or lexicon through Serde so [`Sym`][__link3] fields are
encoded and decoded through the interner.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/internity_macros">source code</a>.
</sub>

 [__link0]: https://docs.rs/internity
 [__link1]: https://docs.rs/internity/latest/internity/derive.DeserializeIn.html
 [__link2]: https://docs.rs/internity/latest/internity/derive.SerializeIn.html
 [__link3]: https://docs.rs/internity/latest/internity/struct.Sym.html
