Signals the runtime that it is time to shut down. The runtime may still operate for a short
time as it shuts down and cleans up resources. During shutdown, existing and new tasks may
be ignored by the runtime and silently dropped.

It is safe to call this function multiple times.