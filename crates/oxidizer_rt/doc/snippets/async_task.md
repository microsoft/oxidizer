Async tasks are the default type of task, intended for short bursts of computation, with
most of the I/O-bound. They must not block the thread for significant spans of time (single digit
milliseconds at most) - do not perform long computation or calls blocking APIs in
a foreground task.