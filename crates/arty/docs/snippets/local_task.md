# Local tasks

A local task runs on the calling worker and can share non-`Send` values with
other tasks there. Its factory takes no arguments. Captures and results may be
non-`Send`, but still need to be `'static`; local spawning does not borrow the
caller's stack.
