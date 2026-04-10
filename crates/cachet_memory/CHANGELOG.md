# Changelog
## [0.1.1] - 2026-04-10

Initial release.

- ✨ Features

  - Add core transform module with Encoder/Codec traits and TransformBuilder ([#355](https://github.com/microsoft/oxidizer/pull/355))
  - add cachet crates with multi-level caching abstractions ([#240](https://github.com/microsoft/oxidizer/pull/240))

- ✔️ Tasks

  - replace futures::executor::block_on in async cachet tests with tokio::test ([#354](https://github.com/microsoft/oxidizer/pull/354))

- ♻️ Code Refactoring

  - Make CacheTier::len()/is_empty() async ([#350](https://github.com/microsoft/oxidizer/pull/350))

