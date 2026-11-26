# Choosing a memory provider

The application, meaning the composition root of the executable binary, owns
memory providers. Library code receives a provider from the application
through its constructor or builder. It does not create a provider for general
use.

If an object implements [`Memory`][crate::mem::Memory] or exposes a provider
through [`HasMemory`][crate::mem::HasMemory], reserve memory from that provider.
This lets the object provide memory configured for its use case, such as
network I/O.

Otherwise, receive an application-owned provider that suits the general use
case. With the `std` feature, an application can create one
`GlobalPool`, keep it for the process lifetime, and pass clones to libraries. A
library should not call `GlobalPool::new()` for each use or hold a process-wide
`GlobalPool` of its own.

A library that must store an arbitrary provider can accept
[`OpaqueMemory`][crate::mem::OpaqueMemory] through its builder and then reuse
that value. The application can pass a `GlobalPool` directly or wrap a custom
provider.

Each test and benchmark binary should model application ownership. If it does
not need a specific provider, give the binary one static, lazily initialized
`GlobalPool` and pass clones or shared references to the code under test. Tests
for provider behavior can create providers as their scenario requires.

In a `no_std` environment, an application can integrate its own specialized
provider by implementing [`Memory`][crate::mem::Memory]. This crate does not
provide a general-purpose allocator-backed provider without `std`.
