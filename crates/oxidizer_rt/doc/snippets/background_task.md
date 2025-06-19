Background tasks are the Oxidizer Runtime equivalent of `thread::spawn()` - they run in the
background, largely independent of any other tasks, and may perform any operations they choose.

Background tasks are not restricted in how long they may block the thread with compute workloads or
with blocking API calls. For any processing-intensive workloads or workloads that make long-lasting
calls into blocking APIs, use a background task. You may still perform asynchronous calls and I/O
from a background task; these tasks are simply executed in a more isolated manner to avoid
interference with foreground tasks.