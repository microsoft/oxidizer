# Changelog

## [0.1.0] - 2025-12-30

- ✨ Features

  - Introduce the layered crate with `Service` trait and layer composition system
  - Add `Execute` wrapper for turning async functions into services
  - Add `Stack` trait for composing layers with tuples
  - Add `Intercept` middleware for observing and modifying inputs/outputs (`intercept` feature)
  - Add `DynamicService` for type-erased services (`dynamic-service` feature)
  - Add Tower interoperability via `Adapter` (`tower-service` feature)
