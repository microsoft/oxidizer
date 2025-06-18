Creates an opportunity for other asynchronous tasks to execute on the current thread. There
is no guarantee what task will execute next on the current thread - it may even be the
current task, despite other tasks being ready to run.