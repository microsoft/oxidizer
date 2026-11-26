# HTTP compression memory

The application supplies `compressors::Resources` when it creates a client or
server compression layer. The resources contain the memory provider and the
recycled engine state used by every transformation in that layer.

The layer never creates a general-purpose memory provider. A caller that
already owns an HTTP body builder can create `Resources` from its provider so
compression and HTTP bodies use the same application-owned memory policy.

The `resources` builder method can replace the supplied resources before the
layer is attached. It never hides provider selection behind a default.
