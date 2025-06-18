Used to get a scheduler with a shorter lifetime. Mostly for backwards copatibility with the current `TaskContext` which doesn't return
references to the scheduler. Will likely go away once the story of scheduler lifetimes is resolved.
