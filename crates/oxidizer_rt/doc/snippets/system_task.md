System tasks execute synchronous code that interacts with the operating system and
potentially blocks for significant time spans. Oxidizer moves many operating system interactions
into system tasks that it awaits from other tasks, to ensure that asynchronous tasks continue
being processed while a system call is in progress.

System tasks should only be used for calls into the operating system, not for any compute
operations - use background tasks for mixed or compute workloads. This functionality is primarily
meant to support the platform adaptation layer of Oxidizer itself, though may also prove useful
for service code that makes calls directly into the operating system.