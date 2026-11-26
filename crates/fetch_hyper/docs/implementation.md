# Hyper transport memory implementation

`HyperTransportBuilder::new` receives and stores a complete `HttpBodyBuilder`.
Its `build` method clones that value and passes it to `build_hyper_handler`.
No constructor fallback allocates a memory provider.

The builder's `body_builder` method can replace the supplied value before
construction. It does not create a provider.

The `fetch` Tokio transport builds one body builder from `TokioDeps::memory`
and passes it to this constructor. This keeps one application-selected
provider across request and response body paths.
