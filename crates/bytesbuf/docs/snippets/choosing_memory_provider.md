# Choosing a memory provider

If you are writing bytes to or reading bytes from an object that either itself implements
[`Memory`][crate::mem::Memory] or exposes an implementation via [`HasMemory`][crate::mem::HasMemory],
you should use [`Memory::reserve()`][crate::mem::Memory::reserve] from this provider
to obtain memory to store bytes in.

Otherwise, use a shared instance of `GlobalPool` when the `std` feature is enabled. In `no_std`
environments, applications that already own a specialized memory provider can integrate it by
implementing [`Memory`][crate::mem::Memory]. This crate does not provide a general-purpose
allocator-backed provider without `std`.
