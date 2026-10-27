# Panic policy

When unwinding is enabled, Arty contains panics at three runtime-owned
boundaries:

- Creation of an async future by its task factory.
- Polling of an async future.
- Execution of a blocking callback.

The panic policy applies when the panic occurs at one of these boundaries. Join
observation does not control the policy. A detached task and a dropped join
handle use the same policy.

`PanicPolicy::default()` and `PanicPolicy::shutdown()` start coordinated runtime
shutdown. `PanicPolicy::ignore()` keeps the runtime available. Configure one
runtime with `RuntimeBuilder::panic`.

The `#[arty::main]` and `#[arty::test]` attributes accept the same values:

```rust
#[arty::main(panic = shutdown)]
# async fn main(_cx: arty::task::Builtins) {}

#[arty::test(panic = ignore)]
# async fn continues(_cx: arty::task::Builtins) {}
```

Both attributes use the shutdown policy when `panic` is omitted.

Arty publishes the panicking task result before it starts shutdown. The task
join remains a panic failure. Pending work and later submissions receive
shutdown failures under the existing cancellation rules.

The runtime shutdown cause records that a contained runtime operation panicked.
It does not replace the original panic payload in the task join.
`Runtime::stop()` returns the shutdown cause after all workers stop.

The first shutdown cause is authoritative. Concurrent panics cannot replace it.
A panic after shutdown starts still retires its task resources, but it does not
replace an earlier stop or failure cause.

The ignore policy contains only the panicking operation. Later submissions can
run normally. The caller must still inspect the failed join and decide whether
the application state is safe to use.

Arty does not guarantee recovery from panics raised by destructors, custom
wakers, join notifications, telemetry processors, or other cleanup and
notification hooks. Arty also does not recover poisoned state, failed
invariants, or unsafe-contract violations.

Arty can catch some excluded panics to maintain memory safety. Callers cannot
depend on continued operation or complete cleanup after these panics. The
process can abort, resources can leak, or runtime operations can stop making
progress. Arty remains memory-safe under all panic conditions.
