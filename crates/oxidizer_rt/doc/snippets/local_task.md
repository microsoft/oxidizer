The term "local" in this context means it is executed on the same thread as an existing
foreground task (the one that schedules the local task), and can therefore access single-
threaded objects shared between the two tasks. In all other aspects, a local task is a
regular foreground task.