# Zygote design

## Status

This document captures the version-one design implemented by `zygote_control`
and `zygote_rt`. Later sections distinguish implemented guarantees from
possible future hardening.

Zygote acceleration targets Linux only and may use Linux-specific facilities
without portability compromises. The controller API also supports Windows and
macOS through a native-spawn backend that starts a fresh process for every
launch.

The accelerated backend supports Linux kernel 5.3 and newer on glibc and musl.
Linux-specific operations use stable kernel syscalls directly when the libc
does not expose a suitable wrapper. Calls introduced after Linux 5.3 are
feature-detected at runtime and have bounded fallbacks; they do not silently
raise the minimum kernel.

## Objective

Launching a dynamically linked process normally repeats work in the kernel,
dynamic loader, C runtime, Rust runtime, and library initialization. The goal is
to pay those costs once for a target executable and then launch independent
instances with substantially lower latency.

On Linux, the target executable starts once and becomes a **zygote** immediately
before entering application code. The zygote waits for launch requests and
calls `fork()` for each request. The child specializes its process state and
invokes the application's entry point. The zygote remains unchanged enough to
service later requests.

The Linux hot launch path does not call `execve()`, `posix_spawn()`, or re-enter
the dynamic loader. Those operations would discard the initialized process
image whose reuse provides the expected speedup. Windows and macOS deliberately
use their ordinary process-creation facilities and provide API compatibility,
not zygote-level launch performance.

The initial release includes both transparent pre-runtime zygotes and explicit
prepared zygotes. Prepared mode is a version-one requirement, not a later
extension.

## Crate responsibilities

### `zygote_control`

The controller-side crate is expected to:

- select the Linux zygote backend or native-spawn fallback for the host;
- on Linux, start a target executable and establish its inherited control
  channel;
- on Windows and macOS, retain the target configuration and spawn a fresh
  process for each command;
- wait for and validate the zygote readiness handshake;
- encode bounded launch requests;
- pass launch-specific file descriptors;
- expose child identity, status, signalling, and cancellation;
- detect protocol, zygote, and child failures without turning them into
  successful launches; and
- shut down the zygote and account for outstanding children.

The controller may be multithreaded and may eventually expose both blocking and
asynchronous integrations. The foundational API should not require an async
runtime.

## Controller API

The controller API should look and behave as much like
[`std::process`](https://doc.rust-lang.org/std/process/) as the zygote process
model permits. Familiarity is more valuable than exposing protocol concepts in
the common path.

Creating the reusable zygote is necessarily an extra step because a normal
`std::process::Command` represents only one launch:

```rust,no_run
use zygote_control::{Stdio, Zygote};

let zygote = Zygote::builder("/path/to/target").spawn()?;

let output = zygote
    .command()
    .arg("--mode")
    .arg("fast")
    .env("REQUEST_ID", "42")
    .current_dir("/work")
    .stdin(Stdio::null())
    .output()?;

assert!(output.status.success());
```

`Zygote::builder()` configures the one-time template launch.
`Zygote::command()` creates an independent per-launch `Command` builder whose
program is the target executable associated with the zygote. Commands may be
prepared and launched repeatedly without restarting that executable.

On Windows and macOS, `Zygote::builder(...).spawn()` creates a native backend
handle without starting the target. Each `command().spawn()`, `status()`, or
`output()` delegates to ordinary platform process creation. Starting a probe
process merely to validate the executable would introduce observable side
effects and is not acceptable.

The public `Command` and `Child` types hide the backend:

```text
Command
  -> Linux zygote request
  -> Windows/macOS std::process::Command

Child
  -> Linux controller-backed child
  -> Windows/macOS std::process::Child
```

Callers should not need platform branches for ordinary arguments, environment,
working directory, stdio, waiting, signalling, or output capture. A capability
query may report whether launches are accelerated, but it must not be necessary
for correct use.

One owning `Zygote` controls template lifetime and shutdown. It creates
inexpensive cloneable `Launcher` handles that are `Send + Sync` and may issue
commands concurrently:

```rust,no_run
let zygote = Zygote::builder("/path/to/target").spawn()?;
let launcher = zygote.launcher();

let worker_launcher = launcher.clone();
let worker = std::thread::spawn(move || {
    worker_launcher.command().arg("--worker").spawn()
});
```

One zygote worker serializes its fork and specialization operations while
allowing launched children to execute concurrently. Callers with bursty launch
traffic can configure `ZygoteBuilder::workers` to start independent templates;
requests are distributed round-robin so fork and specialization can proceed in
parallel. Each worker has its own channel reader and routes out-of-order events
to `Child` handles by request identifier. The default remains one worker to
avoid multiplying prepared-state memory.

`Launcher` cannot shut down the zygote. Explicit shutdown requires the owning
`Zygote`, so hidden launcher clones do not make lifecycle timing
nondeterministic. The behavior of shutdown while launchers or children remain
active is specified separately.

Linux zygote bootstrap failure must not silently fall back to native spawning.
Such fallback would hide deployment errors and unexpectedly change performance,
isolation, argument, and process-tree behavior. Native spawning is selected by
platform, not as recovery from a broken Linux zygote.

The intended blocking surface mirrors `std::process`:

- `Command::arg`, `args`, `env`, `envs`, `env_remove`, `env_clear`,
  `current_dir`, `stdin`, `stdout`, and `stderr`;
- `Command::spawn`, `status`, and `output`;
- `Command::get_program`, `get_args`, `get_envs`, and `get_current_dir`;
- `Child::id`, `kill`, `wait`, `try_wait`, and `wait_with_output`;
- `Child::stdin`, `stdout`, and `stderr`;
- `Output` and `ExitStatus` behavior matching their standard-library
  counterparts; and
- Unix extensions such as `arg0`, process-group selection, and credential
  changes where they can be represented safely in the launch protocol.

Where possible, the API should accept or return standard-library process types
such as `std::process::Stdio`, `Output`, and `ExitStatus`. If their private
representation prevents correct implementation, `zygote_control` should
provide intentionally compatible types rather than expose file descriptors in
the common API.

Builder defaults should match `std::process::Command`: inherited environment,
inherited working directory, inherited stdin/stdout/stderr for `spawn`, and
captured stdout/stderr for `output`. Internally, the controller resolves those
defaults into an exact launch request so the zygote does not accidentally use
its own stale environment or descriptors.

Captured output is intentionally bounded: the default retains at most 8 MiB
from either stream and 16 MiB across both. `Command::output_limits` can lower
or raise both finite limits. Exceeding either limit signals the collector,
cancels both readers so their handles are dropped without waiting for EOF,
attempts child termination within a finite cleanup deadline, and returns an
error instead of a partial `Output`. Cleanup or collector-shutdown failures are
included in that error.

The following differences must remain explicit:

- `Command::new(program)` has no direct analogue for each launch because the
  executable is fixed when the zygote is created.
- `std::os::unix::process::CommandExt::exec` would destroy the controller and
  is not applicable.
- `pre_exec` and `before_exec` closures cannot be serialized into the zygote
  and will not be supported. Common setup operations should have typed,
  protocol-backed methods instead.
- Per-launch arguments are delivered to the adapted application entry point;
  they do not replace the original kernel `argv` visible through
  `/proc/<pid>/cmdline` or Rust runtime internals.
- A spawned `Child` is not an operating-system child of the controller.
  Nevertheless, its observable waiting, signalling, stdio, and exit-status
  behavior should match `std::process::Child`.

An asynchronous API, if added, should be a separate adapter over the same
controller and protocol. It must not force an async runtime into either
foundational crate or change the blocking API's semantics.

### `zygote_rt`

The target-side crate is expected to:

- provide an explicit application entry-point adapter;
- find and validate the inherited control channel;
- remain single-threaded for its entire lifetime;
- decode one bounded request at a time;
- fork a fresh child for each accepted request;
- specialize child process state before application code runs;
- report launch and exit information to the controller; and
- reap every child it creates.

`zygote_rt` must remain small. In particular, it should avoid an async runtime,
background threads, tracing subscribers, general serialization frameworks, and
dependencies that may create hidden process-global state.

On Windows and macOS, `zygote_rt` provides source-compatible pass-through
entry integration rather than a forkserver. Transparent mode invokes the
application normally. Prepared mode runs preparation once for that process and
then invokes the application once. This lets one target source build on all
three platforms even though only Linux amortizes preparation across launches.

## Proposed process model

```text
 controller process
     |
     | start target once, inheriting a private control socket
     v
 zygote target process
     |
     | readiness handshake
     |
     |<---------------- launch request ----------------|
     |                                                  |
     | fork()                                           |
     +--------------------+                             |
     |                    |                             |
     | parent             | child                       |
     |                    |                             |
     | report PID --------|---------------------------->|
     |                    | specialize process state    |
     |                    | invoke application entry    |
     |                    | exit                        |
     | reap child         |                             |
     | report status ---------------------------------->|
     |                                                  |
     |<---------------- next launch request ------------|
```

The zygote is a child of the controller, but launched application instances are
children of the zygote and therefore grandchildren of the controller. The
zygote owns `wait`/`waitid` responsibility and must relay terminal status.
Linux pidfds may give the controller a race-free signalling and polling handle,
but they do not transfer reaping responsibility.

Multiple application children may run concurrently even though the zygote
accepts and forks launch requests serially.

Every Linux launch requires a pidfd. The zygote opens the pidfd while it still
owns an unreaped child and passes it to the controller with `SCM_RIGHTS`.
`Child::id()` returns the numeric PID for compatibility with
`std::process::Child`, but signalling, cancellation, and readiness polling use
the pidfd so PID reuse cannot redirect an operation to an unrelated process.

The pidfd is an internal part of `Child`; Linux-specific `AsFd` support may
expose a borrowed descriptor without making it part of the cross-platform
common API. Exit status still comes from the zygote's `waitid`, because holding
a pidfd does not make the controller the process's parent or transfer reaping
responsibility.

## Entry-point integration

The preferred integration candidate is transparent link-time interception of
the C-ABI `main` shim that `rustc` generates around the user's Rust entry point.
On ELF linkers that support GNU `--wrap`, the final binary can redirect the C
runtime's reference to `main` to a function supplied by `zygote_rt`:

```text
ELF _start
  -> libc startup
    -> __wrap_main(original argc, original argv)
       -> no zygote channel: __real_main(original argc, original argv)
       -> zygote channel: forkserver loop
          -> child: __real_main(request argc, request argv)
```

`__real_main` is the ordinary Rust-generated shim. Calling it in the launch
child enters the Rust runtime and then the user's completely unmodified `main`.
The zygote parent never enters the Rust runtime or application entry point.

This has three important properties:

- applications do not rewrite, annotate, or delegate from `main`;
- the child can call the Rust runtime with the request's actual `argc` and
  `argv`, allowing `std::env::args` and `std::env::args_os` to work normally;
  and
- main-rewriting procedural macros remain unaware of `zygote_rt`.

If no valid inherited control channel is present, the wrapper calls the real
main once with the original arguments. Linking `zygote_rt` therefore does not
by itself prevent ordinary direct execution.

### Direct execution

A zygote-enabled executable must remain an ordinary command-line program:

```text
$ target --mode normal
```

When started without the controller bootstrap, the link-time wrapper immediately
calls `__real_main(argc, argv)`. The application observes its original
arguments, environment, working directory, stdio, signals, and exit behavior.
Main-rewriting frameworks such as Tokio follow their normal path. No forkserver
is created and no zygote-specific command-line flag is required.

Entering zygote mode requires possession of the inherited control-socket
descriptor and successful completion of the bootstrap handshake. An environment
variable may identify the reserved descriptor and carry bootstrap metadata, but
an environment variable alone is not sufficient authorization. The runtime
must validate the descriptor type, socket state, protocol handshake, and random
per-launch nonce before parking instead of calling the application.

The three startup cases are distinct:

1. No bootstrap marker: run the application normally.
2. Marker plus valid inherited channel and handshake: enter zygote mode.
3. Marker present but descriptor or handshake invalid: report bootstrap failure
   and exit without running the application.

The third case fails closed. Silently treating a malformed controller launch as
ordinary execution could run an application with bootstrap arguments,
environment, or descriptors that were never intended for it.

Bootstrap environment entries and reserved descriptors must be removed before
the child enters the real main so a zygote-launched child sees only its requested
environment and cannot mistake itself for another zygote.

In prepared mode, direct execution performs preparation once and then invokes
the application once in the same process. This preserves the prepared entry
point's contract without creating a forkserver. Preparation failure is reported
as an ordinary startup failure.

This mechanism requires an explicit final-link integration such as
`-Wl,--wrap=main`. A dependency build script cannot generally inject arbitrary
link arguments into a downstream binary. The application therefore adds
`zygote_rt` as a build dependency with its `build` feature and calls
`zygote_rt::build::configure()` from `build.rs`. That helper emits only the
required linker arguments. Transparent targets also place `zygote_rt::link!()`
at module scope so the Rust wrapper object is retained by the final link.
Workspace-wide `RUSTFLAGS` are not required.

The public launch contract and bounded native parser/state-machine tests are
ordinary Rust test targets, so the repository's generated Anvil test,
coverage, MSRV, and mutation tiers discover them without a C compiler or
crate-specific workflow configuration.

### Constructor alternative

ELF `.preinit_array` and `.init_array` entries can run a zygote loop before
Rust `main`. AFL-style forkservers demonstrate that this mechanism works: the
zygote waits in a constructor, forks on request, and the child returns from the
constructor so normal startup continues.

A constructor alone is insufficient for fully transparent process semantics.
The platform startup code still calls `main` with the zygote's original `argc` and `argv`,
and constructor ordering cannot portably guarantee a hook immediately before
`main`. A pre-init hook runs before other constructors and therefore does not
amortize their work; a normal init-array hook has ordering interactions with
application and shared-library constructors.

Constructor registration remains a possible fallback if linker wrapping proves
unworkable, but that mode would need an explicit launch-argument API and a
weaker initialization guarantee.

### Pre-runtime implementation boundary

The wrapping function executes before Rust's `lang_start` machinery. It must
not assume that the Rust standard runtime has initialized process-global or
thread-local facilities. The pre-main component is therefore a constrained Rust shim that:

- validates the bootstrap descriptor;
- reads bounded protocol messages;
- checks that the process remains single-threaded;
- calls direct Linux or libc primitives needed for the forkserver;
- forks and performs child specialization; and
- calls `__real_main` only in the launch child.

It uses `core`, raw libc/syscalls, and explicitly bounded libc allocation. It
does not invoke `std`, Rust allocation, thread-local storage, formatting,
panicking paths, async runtimes, or tracing before `__real_main`. The weak
prepared-mode marker is declared at link level because stable Rust has no weak
symbol declaration.

The initial release also provides explicit preparation registration for safe
immutable setup before the forkserver begins. It uses the distinct prepared
entry mode described below because ordinary Rust callbacks cannot run before
`lang_start` without a separately justified runtime contract.

### Prepared zygotes

Application-level preparation is useful when every launch needs the same state
and constructing that state is materially more expensive than copying the
corresponding page tables during `fork()`. Good prepared state is:

- deterministic and identical for every child;
- immutable or overwhelmingly read-only after construction;
- self-contained in private memory rather than backed by live connections;
- free of background threads and thread-affine resources;
- free of child-specific identity, entropy, credentials, and secrets; and
- large or expensive enough that amortizing construction changes measured
  launch cost.

Representative uses include:

- parsed schemas, grammars, templates, and routing tables;
- compiled regular-expression automata and search indexes;
- Unicode, locale, tokenization, and normalization tables;
- decompressed dictionaries and immutable asset indexes;
- read-only model weights or other numerical lookup data;
- validated configuration that is common to all launches; and
- carefully designed compiled-code or plugin snapshots whose runtimes create no
  threads or hidden mutable services during preparation.

The zygote retains the prepared snapshot. Each child sees that snapshot through
copy-on-write. Mutations made by one child are private to that child and do not
affect the zygote or later siblings. Read-only pages remain physically shared;
pages dirtied by a child become private and reduce the memory advantage.

Poor candidates include database connections, network clients, async runtimes,
thread pools, logging exporters, mutable caches, temporary-file state, shared
file offsets, held locks, userspace random generators, and secret-bearing
session state. These objects either couple children through kernel resources,
duplicate unsafe runtime state, or require unique per-child initialization.

File-backed immutable data deserves measurement before being moved into a
prepared heap. Ordinary launches already benefit from the kernel page cache,
and a read-only `mmap` can provide natural cross-process sharing without a
zygote. Preparation pays when parsing, validation, decompression, relocation,
or computation dominates the mapping itself.

Large prepared images are not free. Fork must copy page tables, children incur
copy-on-write faults when they mutate pages, and allocator metadata can cause
otherwise read-only data pages to be dirtied. `Prepared::protected` places the
root `T` in a dedicated anonymous, page-aligned mapping and changes the mapping
to read-only with `mprotect` immediately before launch service begins.
Accidental root writes then fault instead of silently creating private pages.
Protection is deliberately not recursive: `Box`, `Vec`, `String`,
reference-counted pointers, and custom containers retain their existing
allocations. Applications that need a protected transitive image must arrange
separately protected backing mappings and keep only immutable handles in `T`.
Prepared state is retained for the process lifetime, so destructors do not run
after successful preparation in either ordinary or protected mode.

The transparent linker wrapper executes before Rust runtime initialization, so
it cannot safely call an arbitrary Rust preparation callback. Supporting rich
Rust preparation therefore requires a distinct, explicit mode, conceptually:

```text
fn prepare() -> Result<Prepared<PreparedState>, PrepError> {
    Ok(Prepared::new(build_expensive_immutable_state()?))
}

#[tokio::main]
async fn application_main(
    state: &'static PreparedState,
    launch: Launch,
) -> i32 {
    run_application(state, launch).await
}

zygote_rt::prepared_main!(prepare, application_main);
```

In this mode a small ordinary Rust entry point initializes the Rust runtime,
executes `prepare` while still single-threaded, and then enters the forkserver.
The framework-rewritten application function is invoked only in each child.
This mode sacrifices completely transparent `main` integration in exchange for
amortized application initialization.

The baseline transparent mode and prepared mode should remain separate
contracts. Linking `zygote_rt` must not unexpectedly execute application
initializers in the zygote, and prepared mode must make its stronger
single-threaded, no-live-resource safety requirements explicit.

#### Preparation hook and state type

Prepared mode requires exactly one preparation function and one application
function through `prepared_main!`. The macro emits the prepared-mode marker and
an ordinary Rust `main`; preparation therefore executes after `lang_start`, not
directly from the pre-runtime wrapper.

The implemented shape is:

```text
fn prepare() -> Result<Prepared<AppState>, PrepError> {
    Ok(Prepared::new(AppState::new()?))
}
```

`Prepared<T>` owns the state. The runtime retains it for the process lifetime,
and application children receive only `&'static T`. `T` must implement the
unsafe `ZygoteSafe` trait, making the semantic fork-safety assertion visible at
the type boundary.

The type system can enforce useful structural properties:

- state is `'static` and can outlive the preparation stack;
- application code receives shared rather than mutable access;
- an `unsafe ZygoteSafe` trait can state the semantic contract for types whose
  internals the crate cannot inspect.

`Send`, `Sync`, `Copy`, and `'static` are not proofs of fork safety. A socket,
mutex, atomic reference count, raw file descriptor, or handle to global state
can satisfy some of those bounds while still coupling children. There is no
stable Rust trait that means "contains no locks, live kernel objects, mutable
globals, secrets, or fork-sensitive library state."

The return type also cannot constrain side effects performed and discarded by
the hook. A function can start a thread, open and leak a descriptor, initialize
a global singleton, change a signal handler, or seed a process-global random
generator and still return `()`.

Version one therefore relies on the unsafe semantic contract plus a live-thread
check before each fork. It cannot discover hidden library locks, leaked
descriptors, mutable globals, or other discarded side effects. Prepared mode is
consequently an explicit advanced feature, while transparent pre-runtime mode
remains the stronger isolation default. A dedicated arena, resource registry,
and before-and-after process-state audit remain potential hardening work.

#### Prepared-state handoff and lifetime

The zygote owns a typed root pointing to `T`. `fork()` copies the zygote's
address space, so the root and every valid heap pointer have the same virtual
addresses in the child. No serialization, relocation, or reconstruction is
needed.

After child specialization, the prepared entry adapter obtains the inherited
root and calls the application entry point with `&'static T`:

```text
fn application_main(state: &'static AppState, launch: Launch) -> ExitCode {
    use_prepared_tables(state, launch)
}
```

The reference is logically scoped to that process even though its lifetime is
`'static`: each child has its own virtual address space. The type must not
contain references to preparation-stack storage or unsafe kernel resources;
the `ZygoteSafe` implementation asserts those requirements.

Prepared state is shared-only input at the API boundary. A child that needs
mutable per-launch state derives or clones that state into its ordinary private
heap. Interior mutability remains possible and is part of the unsafe
`ZygoteSafe` obligation. Any child write creates private copy-on-write pages;
no sibling or zygote mutation occurs.

The zygote keeps the state allocated for its entire lifetime and does not run
its destructor between launches. If the zygote exits while children are
running, their mappings remain valid: Linux retains their copied address spaces
until each child exits. A child therefore does not depend on the zygote
remaining alive to read prepared state.

### Main-rewriting frameworks

Because interception happens after linking, an attribute macro can continue to
own the ordinary Rust `main`:

```text
#[tokio::main]
async fn main() {
    run_application().await;
}
```

Tokio rewrites this function during compilation, and rustc subsequently emits
the normal C `main` shim around the result. `zygote_rt` intercepts that outer
shim. The Tokio runtime is consequently constructed only after the launch child
calls `__real_main`, never in the zygote parent.

The same ordering should naturally support other main rewriters. It still means
that each child pays for framework runtime construction. A multithreaded runtime
cannot safely be pre-created in the zygote.

### Command-line arguments

The main wrapper constructs a NUL-terminated `argv` vector from the launch
request and passes its count and pointer to `__real_main`. Arguments are Unix
byte strings, not UTF-8 strings, and embedded NUL bytes are invalid.

This should give Rust's standard argument APIs the same values they would
observe after an ordinary spawn. Linux `/proc/<pid>/cmdline` is separate: it
describes the kernel's original argument memory range and may continue to show
the zygote invocation unless privileged `PR_SET_MM` operations are used. The
crate should document that diagnostic difference rather than require privilege
or mutate process-title storage unsafely.

### Environment

Environment changes occur only in the forked child, never in the zygote. The
public builder should follow `std::process::Command` and therefore begin with
the controller's inherited environment plus requested modifications. Before
transmission, the controller resolves this into an exact environment so the
target launch is deterministic and does not inherit stale zygote state.

The controller starts the template with only the authenticated bootstrap
variables, and the runtime erases those strings before publishing readiness.
Consequently, Linux `/proc/<pid>/environ` exposes an erased initial environment
rather than the launch environment; ordinary `std::env` and libc environment
access sees the exact requested values. Making the kernel-reported range match
would require privileged `PR_SET_MM` operations and is intentionally outside
the portable launch contract.

This also means per-launch environment values are unavailable to the dynamic
loader during the one-time template startup. Variables such as
`LD_LIBRARY_PATH` and `LD_PRELOAD` cannot configure that load; deployed targets
must resolve their initial dependencies through standard loader paths or
embedded paths such as `RPATH`/`RUNPATH`. Inheriting the controller environment
would copy ambient credentials and other secrets into the template and every
forked child, so version one intentionally does not offer that behavior.

Environment names and values are Unix byte strings with the same restrictions
as `execve`: names cannot contain `=` or NUL, and values cannot contain NUL.
The implementation must establish the final environment before application
code can create threads. The exact interaction with Rust's environment APIs
needs a focused prototype before the API is fixed.

## Control transport

The controller should create a connected Unix-domain `SOCK_SEQPACKET` socket
pair and arrange for one endpoint to be inherited by the initial target on a
reserved descriptor. This provides:

- private capability-based access with no filesystem socket;
- preserved message boundaries;
- bidirectional request and event traffic;
- atomic delivery or explicit failure for bounded packets; and
- `SCM_RIGHTS` descriptor passing.

The descriptor number is an implementation detail communicated through a small
bootstrap contract, likely an environment variable plus a random nonce in the
first handshake. The descriptor must be close-on-exec everywhere except the
one intentional inheritance edge.

The controller treats channel closure before a valid readiness message as a
startup failure. A target with no bootstrap marker runs normally. A target with
a marker but an invalid channel fails closed.

## Launch request

The initial request model should be able to express:

- a controller-assigned request identifier;
- `argv`, including a logical `argv[0]`;
- an exact environment;
- working directory;
- stdin, stdout, and stderr descriptors;
- uid, gid, and supplementary groups;
- process-group or new-session policy;
- umask; and
- resource limits.

These are the version-one process-specialization attributes. They cover the
serializable equivalents of common Unix process-spawning and daemon setup.
Arbitrary `pre_exec` closures are not supported.

Sandboxing is an optional, typed, fail-closed launch layer. Portable
`SandboxPolicy` currently expresses reduction-only privilege intent. Exact
mechanisms remain platform APIs so that unlike guarantees are not collapsed:

- Linux accepts typed privilege reduction, bounded classic-BPF seccomp,
  caller-created Landlock rulesets, cgroup v2 directory capabilities from
  which the controller opens `cgroup.procs`, and descriptors for cgroup, IPC,
  UTS, network, time, and mount
  namespaces. User and PID namespace entry is rejected because each requires a
  different credential or process-lifecycle design.
- Windows creates every process with an extended startup attribute list and an
  explicit standard-handle allowlist. Optional attributes configure a Job
  Object, AppContainer security capabilities, and immutable mitigations; an
  optional restricted primary token supplies reduction-only identity. If
  resuming the suspended primary thread fails, synchronous cleanup has a finite
  deadline. Before any process is created, the backend initializes one durable
  cleanup service. An unresolved process and thread are transferred to that
  already-live service, which retains every pair of handles and services
  pending children round-robin with nonblocking status checks and termination
  retries until each is terminal. Service initialization failure prevents
  process creation and is retried by a later launch.
- Other targets reject non-empty portable sandbox policy with `Unsupported`.

Linux support can be inspected with `linux::Support::probe`; Landlock and
Windows Job/mitigation policies also expose exact probes. Probe results are
advisory because support and permission can change before launch. Launch
failure is always authoritative and never triggers an unsandboxed retry.

Path-based setup is vulnerable to races and namespace differences. Passing an
already-open directory descriptor and using fd-relative operations is preferable
where Linux exposes the needed operation.

Every count and byte length has a documented limit enforced before allocation.
Descriptor roles must agree exactly, in order, with ancillary data. Unknown
required features, malformed ancillary data, duplicate singular fields,
incompatible options, duplicate resource limits, embedded NUL bytes, and
trailing malformed encodings are protocol errors rather than values to ignore.

## Protocol

The wire protocol is the checked-in `zygote.proto` schema. Each
`SOCK_SEQPACKET` datagram contains exactly one Protobuf `Packet`, with:

- an incompatible protocol major version;
- a controller request identifier;
- sorted required feature identifiers;
- ordered descriptor roles for out-of-band `SCM_RIGHTS` data; and
- a `oneof` containing `Ready`, `Launch`, `Started`, `Exited`, `Error`, or
  `Shutdown`.

Expected message classes are:

- zygote ready or startup failure;
- launch request;
- launch accepted with PID and a required pidfd;
- child setup failure;
- child exit, signal, or core-dump status;
- shut down zygote; and
- protocol-level error.

Protocol negotiation rejects incompatible major versions and unknown required
features. Optional additive fields can be introduced without silently enabling
semantics an older receiver cannot enforce.

`zygote_rt` owns the schema and committed Prost message definitions, and
`zygote_control` uses Prost for normal encoding and decoding. Downstream builds
do not run `protoc`. The pre-runtime shim has a separate bounded,
allocation-free Protobuf wire decoder/encoder because Prost allocation is not
permitted before `lang_start`; both implementations are checked against the
same schema.

`zygote_rt` must keep this module independent of target runtime state and Linux
syscalls so the native-spawn controller backend can compile on Windows and
macOS. Protocol compatibility follows `zygote_rt`'s semantic version, and the
controller pins a compatible runtime version. Duplicating protocol definitions
between the crates is not acceptable.

## Child specialization

After `fork()` and before calling application code, the child may need to:

1. close the zygote listener and unrelated inherited descriptors;
2. reset or establish signal masks and dispositions;
3. configure process group or session;
4. install the working directory;
5. install the exact environment;
6. map the three standard descriptors without collisions;
7. enter requested cgroup, IPC, UTS, network, time, and mount namespaces;
8. join the caller-selected cgroup v2 through the `cgroup.procs` file opened
   relative to its directory descriptor;
9. apply resource limits and umask;
10. reduce the capability bounding set and lock securebits;
11. apply groups and credentials, then clear capability sets;
12. set and verify `no_new_privs` and dumpability;
13. apply the caller-created Landlock ruleset;
14. close every setup descriptor except the private acknowledgement descriptor
    and all descriptors above standard error;
15. install the architecture-checked seccomp filter;
16. notify the zygote that specialization, including seccomp installation,
    succeeded;
17. close the private acknowledgement descriptor; and
18. invoke the application entry point.

The exact order is security-sensitive. For example, privileges and namespaces
must not be changed before operations that require them, while untrusted
application code must not run before descriptor cleanup and restrictions are
complete.

Linux `close_range` is the preferred basis for descriptor hygiene, with a
bounded `/proc/self/fd` or descriptor-range fallback. Seccomp is installed
before the final acknowledgement so `Started` proves that the filter is active.
Policies must permit the exact `write(3, ...)` acknowledgement and subsequent
`close(3)` cleanup. An unexpected post-filter acknowledgement or close failure
exits with status 126 before application entry.

## Fork-safety boundary

The zygote must be permanently single-threaded. Forking a multithreaded Rust
process can leave allocator, loader, stdio, tracing, and library locks owned by
threads that do not exist in the child.

The runtime cannot reliably prove that no foreign library or constructor has
created a thread. The safety contract therefore needs both:

- a documented requirement that linking code must not start threads before the
  zygote loop; and
- a best-effort Linux check, such as verifying that `/proc/self/task` contains
  exactly one task before every fork.

A failed check must stop new launches rather than silently proceed. Preparation
callbacks, if introduced, must obey the same rule.

The child will inevitably need some operations after `fork()`. POSIX's strict
async-signal-safe rule is designed for a multithreaded parent; a verified
single-threaded zygote avoids the abandoned-lock failure mode but does not make
arbitrary library behavior automatically sound. The implementation should keep
this interval minimal, prefer direct Linux syscalls, pre-validate request data,
and document every operation performed there.

## Isolation and state inheritance

Fork cannot produce the same independence as `execve`. It deliberately copies
one process image and retains references to selected kernel objects. The design
goal is therefore a **clean fork baseline with explicit inheritance**, not a
claim that the kernel created an unrelated process.

Each launch request is an inheritance manifest. Nothing crosses the fork
boundary merely because the implementation happened to have it open. The
request names arguments, environment, working directory, standard descriptors,
credentials, resource limits, and process-group policy. Child specialization
removes or normalizes everything else before application code runs.

Sibling children do not share ordinary writable memory: after `fork()`, private
pages are copy-on-write and mutations are local to each child. Sneaky coupling
comes from shared mappings, shared kernel objects, and duplicated process-global
state, all of which require separate treatment.

### Memory

The zygote should contain as little mutable state as possible. Link-time main
interception keeps Rust runtime state, async runtimes, application globals, and
application secrets out of the zygote. The remaining image should primarily be
the loader, executable mappings, library mappings, and the small forkserver.

Future hardening may allocate zygote-only writable mappings separately and mark
them `MADV_DONTFORK`, and use `MADV_WIPEONFORK` for sensitive scratch state.
Version one starts the template with only its bootstrap environment, erases
that environment before readiness, securely erases launch request storage
after each fork, and relies on private copy-on-write memory plus complete
descriptor cleanup.

Unexpected writable `MAP_SHARED` mappings are sibling communication channels
even after all descriptors are closed. Before readiness, the runtime rejects
such mappings through `/proc/self/maps` and rejects descriptors other than
standard I/O and the authenticated control socket. The same validation also
requires the template to remain single-threaded. Prepared applications must
close temporary resources before returning from preparation.

Copy-on-write does not protect secrets already present at the fork point: every
child receives a private copy. Secret-bearing initialization therefore belongs
after the fork unless disclosure to every child is intentional.

### File descriptors and open file descriptions

Immediately after `fork()`, the child must close the zygote control socket,
zygote signal descriptors, listeners, bookkeeping descriptors, and every other
descriptor not named by the launch manifest. Linux `close_range` is preferred,
with descriptors remapped in a collision-safe order using `dup3`.

Closing unexpected descriptors prevents accidental communication, but
duplicating an allowed descriptor does not make its underlying kernel object
independent. File descriptors inherited through `fork`, `dup`, or `SCM_RIGHTS`
can refer to the same open file description and therefore share file offsets,
status flags, and some lock state. Pipes, sockets, eventfds, and shared-memory
descriptors are communication channels by definition.

The default API must create request-specific stdio pipes and descriptors.
Passing one descriptor to multiple launches is an explicit shared-resource
operation. Callers that require independent access to a regular file must open
it separately for each launch; duplicating one descriptor is insufficient.

The one short-lived setup acknowledgement channel between zygote and child has
a fixed 30-second deadline and must be closed before application entry. On
expiry, the zygote sends `SIGKILL`, synchronously reaps the ambiguous child,
removes it from active-child bookkeeping, and reports an error without sending
`Started`. A cleanup failure terminates the worker event loop rather than
admitting another launch with ambiguous state. After successful
acknowledgement, no hidden zygote-child IPC descriptor remains. Child exit
observation occurs through the kernel's parent relationship, not a channel held
by the child.

### Signals and process relationships

Child setup must install a known signal mask and dispositions, disable inherited
alternate signal stacks and timers where applicable, and ensure no
parent-death signal couples it to the zygote. Process-group and session
membership must follow an explicit launch policy rather than zygote defaults.

The zygote remains the child's parent until either process exits. That
relationship exists for reaping and status reporting but should convey no
lifetime policy. The zygote does not send implicit shutdown signals, and the
child does not retain the control channel.

### Process-global and kernel state

The child must establish exact or explicitly inherited values for:

- environment and working directory;
- umask and resource limits;
- credentials, capabilities, dumpability, and `no_new_privs`;
- scheduler, affinity, and process-group settings included in the API;
- namespaces and cgroup placement when isolation features request them; and
- userspace random generators initialized before the fork.

Some inherited restrictions, such as seccomp filters, namespaces, capabilities,
and `no_new_privs`, cannot be relaxed in the child. The zygote baseline must
therefore avoid restrictions that conflict with later specialization, or use
separate zygotes for incompatible policies.

### Independence limits

Several properties cannot be made fresh without executing a new image:

- children have correlated virtual address layouts and do not receive fresh
  ASLR;
- loader state and already loaded libraries are inherited;
- process-wide values initialized by ELF constructors may be duplicated,
  including userspace random state and security cookies;
- immutable file-backed code pages are intentionally shared;
- credentials, namespaces, and cgroup membership begin as copies of the
  zygote's state; and
- external supervisors may impose shared lifecycle or resource policy.

If a caller requires fresh ASLR, a fresh loader, no inherited address-space
secrets, or independently initialized ELF constructors, it requires
`execve`/`posix_spawn` rather than a zygote launch. The API and documentation
must expose this boundary plainly.

### Verification

Isolation should be checked as an invariant, not assumed from code review.
Before publishing readiness, the zygote verifies its task count, descriptor
inventory, and shared writable mappings through Linux inspection interfaces.
It checks the task count again before each fork because preparation or later
library activity could otherwise make forking unsafe. A future diagnostic mode
may expose the resulting inheritance report to applications.

Integration tests should compare a zygote-launched no-op process with an
ordinarily spawned baseline, checking descriptors, environment, signal state,
working directory, process groups, mappings, and termination behavior. Tests
should also deliberately pass one shared descriptor to demonstrate that such
sharing occurs only when requested.

## Lifecycle and concurrency

The zygote should avoid asynchronous signal handlers for child management.
Blocking `SIGCHLD` and consuming it through `signalfd`, combined with
`waitid(..., WNOHANG)`, would keep launch requests and child transitions in one
ordinary event loop.

Controller requests and zygote events need request identifiers because exits
can arrive out of order. The controller should have one channel reader that
dispatches events to launch handles; concurrent readers on the control socket
would make ownership and shutdown behavior difficult to reason about.

An optional dormant-worker pool moves `fork` off the request-to-entry critical
path. Each worker is forked only after transparent or prepared initialization,
waits on a private `SOCK_SEQPACKET` channel, accepts exactly one validated
launch request and its descriptors, specializes itself, reports success, runs
the application, and exits. Pool misses preserve the direct-fork path rather
than queueing behind an idle-worker ceiling. Workers close the template control
channel and every sibling private channel, so they cannot keep the controller,
template, or one another alive.

`PreforkPoolConfig` sets minimum and maximum idle counts, a refill threshold,
and a delayed refill interval. Falling below the minimum refills immediately
after the assigned application has entered; threshold refills happen from the
event loop after the configured delay. `Zygote::trim_prefork_pool` changes the
current target explicitly for memory pressure and can restore any target up to
the configured maximum. Shutdown terminates and reaps dormant workers while
leaving assigned application children independent.

Dormant-worker dwell time is an isolation boundary. Bootstrap nonce storage is
erased before workers are forked, request arenas are created only after
assignment, and workers are single-use, so prior-launch arguments,
environments, descriptors, and sandbox policy cannot survive into another
launch. Process-global mutable state remains governed by the same documented
zygote-safety contract as direct forks.

Linux worker recovery is opt-in and bounded. A failed template is removed from
routing immediately; healthy shards continue serving while a background
replacement observes exponential backoff, reruns executable bootstrap and
prepared initialization, and becomes routable only after the readiness
handshake. Requests from the failed generation are never replayed. Requests
that had not reached `Started` receive an explicit indeterminate-outcome error,
while already-started children retain their pidfds.

`Launcher::health` and `Zygote::health` expose dependency-neutral atomic
snapshots: ready workers, active and idle child counts, pool hits and misses,
refill counts and time, launch outcomes, worker restarts, request-to-entry
latency, and shutdown state. Counters have fixed cardinality and never include
arguments, environment values, sandbox policy contents, or other request data.

Dropping a launch handle must not accidentally kill the child. Cancellation
should be explicit. Dropping the zygote controller should have a documented
policy for the zygote and running children rather than depending on descriptor
closure timing.

Explicit `Zygote::shutdown` stops accepting launches and terminates the zygote
immediately without waiting for active children. Existing `Launcher` handles
become closed, and later launch attempts return a shutdown error. Running
children remain alive.

On Linux, a `Child` retains its pidfd after shutdown and can still poll for
termination or signal the process safely. If the zygote did not report terminal
status before exiting, however, `Child::wait` cannot recover the full exit code
because the controller is not the child's parent. It returns a specific
detached-status error rather than inventing a successful status. Callers that
need exit codes must wait for their children before shutting down the zygote.

On native-spawn backends, each `Child` remains directly waitable because it is
an operating-system child of the controller. The portable contract promises
that shutdown leaves children running; retaining uncached exit status after
shutdown is a backend capability rather than a cross-platform guarantee.

### Controller shutdown

The default shutdown contract follows ordinary Linux process spawning:

- graceful controller shutdown sends a zygote-shutdown request and closes the
  control channel;
- unexpected controller termination is detected when the kernel closes the
  inherited control-channel endpoint;
- the zygote stops accepting launches, closes its descriptors, and exits
  promptly;
- the zygote does not signal, cancel, or wait for running children during this
  shutdown; and
- Linux reparents those children, and any descendants they have created, to
  the applicable child subreaper or PID 1.

The control-channel descriptor must be closed in every launched child so a
descendant cannot accidentally keep the zygote alive after the controller has
gone away.

No default launch option may install `PR_SET_PDEATHSIG` relative to the zygote,
place children in a process group that the zygote kills on exit, or otherwise
couple child lifetime to the zygote. Terminating a running child tree is an
explicit opt-in operation, not shutdown cleanup.

This contract controls process lifetime, not I/O lifetime. If a child uses
captured pipes owned by the controller, controller exit closes those endpoints;
the child can then observe EOF, receive `SIGPIPE`, or block according to normal
pipe semantics. Callers that need fully detached descendants must provide
independent stdio descriptors.

External supervisors remain outside this guarantee. For example, systemd,
containers, or job objects implemented through cgroups may terminate every
process in the containing unit regardless of the parent-child policy here.

## Security model

The inherited socket is a capability: only a process holding the descriptor can
issue requests. No named listener or network transport is planned.

The protocol is still an input boundary and must be treated as hostile to
memory safety and resource availability. The runtime must bound packets,
strings, vectors, descriptors, concurrent children, and queued requests.

Sandboxing is opt-in rather than an ambient property of zygote launches.
Without a policy, forked children begin with the memory, credentials,
namespaces, and resource domains held by the template. With a policy, every
requested guarantee, including seccomp installation, is installed and verified
before `Started`; unsupported, malformed, or partially applied policy
terminates the child and reports its setup stage. Seccomp programs must allow
the private post-install `write(3, ...)` acknowledgement and `close(3)`
cleanup. No internal setup descriptor remains open when application code
begins. Sandboxing does not change the documented address-space and
constructor-state inheritance limits of `fork`.

## Failure model

Failures must identify their phase:

- initial target spawn;
- zygote bootstrap or version handshake;
- request validation;
- `fork()`;
- child specialization;
- application exit or signal;
- status transport; or
- zygote termination.

A launch is not successful merely because `fork()` returned a PID. The child
must complete specialization, install seccomp when requested, and then
acknowledge readiness before the controller emits `Started`. Filters that
cannot permit the private `write(3, ...)` acknowledgement and `close(3)`
cleanup are rejected before launch. Any setup or installation failure is
reported as a structured error and the zygote reaps the child.

If the zygote dies, every pending request becomes failed. Existing children may
continue briefly unless parent-death policy terminates them; the initial API
should choose one deterministic behavior.

## Performance expectations

The design is worthwhile only if it measurably improves end-to-end launch
latency over `std::process::Command` and direct `posix_spawn`. Benchmarks should
separately measure:

- controller request-to-entry latency;
- request-to-exit latency for a no-op application;
- sequential and concurrent launch throughput;
- zygote and per-child proportional set size;
- copy-on-write faults for representative initialized state; and
- latency as argument, environment, and descriptor counts grow.

The benchmark baseline must include a small dynamically linked Rust executable.
Measurements should include cold and warm filesystem-cache conditions and state
the kernel, libc, linker, CPU, and mitigation configuration.

The committed process benchmarks use `metabench` with Criterion for the full
latency and scaling matrices and representative allocation/performance-counter
cases. Run them with `--criterion --allocations --perf`; Gungraun is omitted
because process creation, syscalls, scheduler behavior, and I/O dominate these
workloads.

The optimization experiments retained the following measured designs:

- controller event readers retain 4 KiB for common packets and allocate the
  exact datagram size only for larger events;
- launch requests are encoded once after exact-size validation, and the native
  arena erases the initialized request range while teardown still erases all
  retained storage;
- active children are kept densely, so reaping work scales with live children
  rather than table capacity;
- Unix output capture uses one polling collector per child instead of one
  blocking thread per stream; Windows keeps two readers because anonymous
  pipes do not provide the same readiness primitive; and
- inherited Windows environment entries and overrides use an ordered merge
  with ordinal case-insensitive key comparison.

The forced legacy descriptor benchmarks did not show a stable latency benefit
that would justify replacing the exhaustive numeric fallback with an
inventory that could omit externally inherited descriptors. Concurrent launch
measurements likewise did not justify a more complex waiter slab or shared
output service: process creation remains dominant, while the existing router
preserves straightforward cancellation and shutdown ownership. Those
prototypes were therefore not adopted. Preforking remains opt-in because its
pool-hit improvement is workload- and scheduler-dependent; the benchmark
matrix reports pool hits, misses, refill time, and proportional memory for
idle counts 0, 1, 2, 4, 8, and 16.

## Prior art

- AFL and AFL++ establish a linked target-side forkserver before the workload
  and use control/status pipes to request and observe forks.
- Chromium's Linux zygote receives spawn requests and forks a pre-initialized
  renderer image.
- Android's Zygote and unspecialized app-process pool combine on-demand forks,
  process specialization, and optional pre-forked children.
- `libzygote` is a small C library with a Unix-socket controller, environment
  and working-directory transfer, `SCM_RIGHTS` stdio passing, and a `run`
  function loaded in each child.
- Python's `multiprocessing` forkserver centralizes forks in a known
  single-threaded helper process.

These systems validate the process model but do not provide a directly reusable
Rust API with the desired launch contract.

## Resolved initial decisions

1. Direct invocation runs the application normally. A bootstrap marker with an
   invalid channel fails closed.
2. Transparent mode invokes the application's ordinary `main` with conventional
   arguments. Prepared mode additionally exposes typed, logically immutable
   state through its specialized adapter.
3. Prepared mode is part of the initial release.
4. `zygote_rt` owns the shared wire protocol, and `zygote_control` depends on
   that minimal protocol surface.
5. One owning `Zygote` controls lifecycle; cloneable `Launcher` handles issue
   concurrent commands.
6. Every accelerated Linux child has a required pidfd.
7. Explicit shutdown exits the zygote immediately and leaves children running,
   even though uncached Linux exit status may then become unavailable.
8. Specialization includes argv, environment, working directory, descriptors,
   Unix credentials and groups, process group/session, umask, resource limits,
   and the cfg-gated sandbox policies described above.
9. The accelerated backend supports Linux 5.3+ on glibc and musl, using direct
   syscalls and runtime detection where appropriate.
