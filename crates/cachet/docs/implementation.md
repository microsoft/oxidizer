# Cachet serialization implementation

`CacheBuilder::serialize` and `FallbackBuilder::serialize` accept an `OpaqueMemory` value.
Each builder clones the provider into `PostcardEncoder` and `PostcardCodec`.
This supplies the provider to key encoding and value encoding without a global provider.

`PostcardEncoder` and `PostcardCodec` store the supplied provider as `OpaqueMemory`.
The encode helper writes the format version and postcard payload into a `BytesBuf`.
The decode path reads the format version and postcard payload from the byte view.
The provider selection does not change these bytes.
