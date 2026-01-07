# Choosing a memory provider

If you are writing bytes to or reading bytes from an object that either itself implements
[`Memory`][crate::mem::Memory] or exposes an implementation via [`HasMemory`][crate::mem::HasMemory],
you should use [`Memory::reserve()`][crate::mem::Memory::reserve] from this provider
to obtain memory to store bytes in.

Otherwise, use a shared instance of [`GlobalPool`][crate::mem::GlobalPool], which is a reasonable
default when there is no specific reason use a different memory provider.
