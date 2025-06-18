Note that the operating system may also have its own asynchronous resource release mechanisms,
which are not possible to await and may keep a file/socket/port/... locked for some time after
it is released by the I/O subsystem.

If an I/O primitive implements multiple shutdown modes (e.g. graceful vs forced) then dropping
an instance of this type or calling `close()` will use the "forced" mode of releasing resources.
If a more graceful resource release is desired, other APIs external to this type should instead
be used directly on the platform-specific I/O primitive wrapped by this type.