# HTTP compression memory implementation

`Compression::client` and `Compression::server` receive `Resources` by value
and store them in shared layer configuration. `CompressionLayer::resources`
replaces that stored value when the caller needs a different engine pool or
memory provider.

The fetch client creates resources from the client body builder's provider.
This keeps memory selection in the composition root and avoids a library-owned
global pool.
