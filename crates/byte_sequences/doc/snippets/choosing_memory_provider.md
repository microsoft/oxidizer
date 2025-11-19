# Choosing a memory provider

If you are writing bytes to or reading bytes from an object that either itself implements
[`Memory`][crate::Memory] or exposes an implementation via [`HasMemory`][crate::HasMemory],
you should use [`Memory::reserve()`][crate::Memory::reserve] from this provider
to obtain memory to store bytes in.

Otherwise, use [`GlobalPool`][crate::GlobalPool], which is a reasonable
default when there is no specific reason use a different memory provider.
