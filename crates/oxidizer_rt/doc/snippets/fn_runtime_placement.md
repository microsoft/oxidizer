Returns token representing the placement of the current task on runtime controlled threads.

This token can be passed to `Placement` metadata of subsequently spawned tasks to control their placement on runtime controlled threads.

Returns `None` if placement of task isn't replicable (for example, task was placed on a temporary thread).
