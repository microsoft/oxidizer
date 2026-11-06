# Changelog

## [0.6.1] - 2026-11-06

- Preserve self-borrows in deferred futures by keeping their storage stable while polling.
- Avoid notifying the worker again when a task running on that worker wakes itself.
- Clarify task creation, worker scheduling, and transfer behavior in the documentation.
- Update the local `arty_executor` requirement from `0.6.0` to `0.6.1`.
- Update the local `arty_io_host` requirement from `0.6.0` to `0.6.1`.
- Update the local `arty_macros` requirement from `0.6.0` to `0.6.1`.
- Update the local `observed` requirement from `0.30.0` to `0.30.1`.
- Update the local `performables` requirement from `0.3.1` to `0.4.0`.
- Update the local `seismograph` requirement from `0.4.0` to `0.5.0`.
- Update the local `tick` requirement from `0.9.0` to `0.9.1`.

## [0.6.0] - 2026-11-02

- Breaking: Replace `arty::task::Builtins` with `arty::task::Context` in task factories and macro
  entry points. A task factory creates a task's future.
- Breaking: Replace `RuntimeOperations` with `RuntimeControl`. Use `RuntimeControl::workers` and
  `Scheduler::spawn_at` instead of `spawn_everywhere` to select the threads that run tasks.
- Breaking: Remove the `time` feature from your feature list. Clocks are always available, and
  `test-util` supports controlled clocks without a runtime.
- Breaking: Manage Observed telemetry in application code. Arty removes `RuntimeBuilder::sink`,
  `Context::sink`, `AsRef<observed::Sink>`, runtime events, and automatic transfer of Observed event
  context.
- Add `Context::io` to create I/O drivers when a worker first needs them. A worker is a thread that
  runs tasks.
- If an I/O driver fails, the runtime stops. During shutdown, it cancels tasks before stopping drivers.
- Add `PanicPolicy` and the `panic = shutdown` or `panic = ignore` macro configuration. By default,
  caught panics in tasks and blocking callbacks stop the runtime.
- Add `MonitorPolicy` and `monitor = enabled|disabled` to store monitoring configuration. This
  configuration does not start a monitor or add task measurements.
- Start one worker by default. Honor the requested worker count even when workers must share
  processors, and support `workers = all` in runtime macros.
- Arty updates thread-aware task results for the worker that awaits them. It reports invalid task
  destinations through `JoinError::is_invalid_destination`.
- Keep runtime owners on threads outside Arty's worker pools. `Runtime::stop` and runtime
  destruction panic on task and blocking workers.
- Use `RuntimeControl::request_stop` to request shutdown from worker code.
  `RuntimeScheduler::block_on` rejects calls from task and blocking workers.
- Reduce competition for scheduler locks and combine task completion and shutdown handling.
- Add a requirement for local package `arty_io_host` at `=0.6.0`.
- Remove the version requirement for local package `data_privacy` (previously `0.13.1`).
- Add a requirement for local package `ohno` at `0.5.4`.
- Add a requirement for local package `plurality` at `0.2.4`.
- Add a requirement for local package `seismograph` at `0.4.0`.
- Update the requirement for local package `arty_executor` from `=0.5.0` to `=0.6.0`.
- Update the requirement for local package `arty_io_core` from `0.3.0` to `0.4.0`.
- Update the requirement for local package `arty_macros` from `=0.5.0` to `=0.6.0`.
- Update the requirement for local package `observed` from `0.29.0` to `0.30.0`.
- Update the requirement for local package `performables` from `0.3.0` to `0.3.1`.
- Update the requirement for local package `thread_aware` from `0.12.1` to `0.12.2`.
- Update the requirement for local package `thread_aware_core` from `0.1.2` to `0.1.3`.
- Update the requirement for local package `tick` from `0.8.0` to `0.9.0`.
- Update the development requirement for `gungraun` from `=0.19.4` to `=0.20.0`.
- Update the development requirement for `tokio` from `1.48.0` to `1.53.3`.

## [0.5.0] - 2026-10-20

- Adopt the breaking `tick` 0.8 timer contract and harden runtime task, shutdown, panic, and worker lifecycle behavior.
- Update local dependency requirements: arty_executor =0.1.3 -> 0.5.0; arty_io_core ^0.2.0 -> 0.3.0; arty_macros =0.4.0 -> 0.5.0; data_privacy ^0.13.0 -> 0.13.1; observed ^0.28.0 -> 0.29.0; performables ^0.2.0 -> 0.3.0; thread_aware ^0.12.0 -> 0.12.1; thread_aware_core ^0.1.1 -> 0.1.2; tick ^0.7.1 -> 0.8.0.

## [0.3.1] - 2026-10-05

- 🔧 Maintenance

  - Now requires `0.7.1` of `tick`

- 🔄 Continuous Integration

  - complete cargo-anvil adoption ([#759](https://github.com/microsoft/oxidizer/pull/759))

## [0.3.0] - 2026-09-10

### Breaking

- Now requires `0.1.1` of `thread_aware_core`
- Now requires `0.7.0` of `tick`

### Features

- introduce runtime facade and core crates ([#727](https://github.com/microsoft/oxidizer/pull/727))
