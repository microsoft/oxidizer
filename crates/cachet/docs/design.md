# Cachet serialization design

The serialization boundary converts typed cache keys and values to `BytesView` values.
The boundary must keep the typed cache behavior outside the boundary and the byte behavior inside it.

The caller supplies one `OpaqueMemory` provider to `CacheBuilder::serialize`.
The builder uses that provider for key encoding and value encoding.
The same rule applies to `FallbackBuilder::serialize`.

The serialized format keeps format version `1` and postcard payloads.
The boundary must not change cache insertion, lookup, fallback, promotion, or eviction behavior.
