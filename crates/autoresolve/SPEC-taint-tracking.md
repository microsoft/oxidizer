# Taint-tracking for scoped resolver sharing

## Problem

When sibling scoped resolvers resolve the same type that only depends on
shared (ancestor) data, each child constructs its own copy independently.
The type should be resolved once and stored in the shared ancestor so all
siblings reuse it.

## Design constraints

1. **Thread safety.** Sibling scoped resolvers may run concurrently. Any
   data written into a `SharedTypeMap` ancestor could be read by another
   thread at any time. Resolution itself takes `&mut self`, so the local
   taint state does not need synchronization — but the ancestor write
   must go through `get_or_insert()` which already uses a `Mutex`.

2. **Multiple ancestor levels.** With nested scoping (app → request →
   task), a type resolved in a task scope may depend only on data from
   the app ancestor (`ancestors[1]`), not the request ancestor
   (`ancestors[0]`). Blindly storing in `ancestors[0]` places the value
   in the wrong scope — it would be dropped when the request scope ends
   instead of living for the full app lifetime, and it would not be
   shared across sibling *requests*.

## Proposed solution: ancestor-depth tracking

Replace the boolean taint flag with a **depth marker**: an
`Option<usize>` that records the shallowest (nearest) scope that
contributed to the resolution chain. The depth value represents the
storage tier:

- `None` — all inputs came from ancestors; the type can be promoted.
- `Some(depth)` — at least one input was found at this depth (0 =
  local, 1 = `ancestors[0]`, 2 = `ancestors[1]`, …).

Since resolution requires `&mut self`, this is a plain field on
`ScopedResolver` — no `Cell`, no `Atomic`, no synchronization needed
for the tracking itself.

### `ScopedResolver` changes

```rust
struct ScopedResolver<T> {
    ancestors: Vec<Arc<SharedTypeMap>>,
    types: TypeMap,
    depth: Option<usize>,  // NEW — tracks shallowest contributing scope
    base: PhantomData<T>,
}
```

### Resolution flow

1. Before resolving inputs for a new type, save the current `depth` and
   reset it to `None`.
2. Each dependency lookup updates `depth`:
   - Found in local `types` → `mark(0)`.
   - Found in `ancestors[i]` → `mark(i + 1)`.
   - (`mark` keeps the *minimum* depth seen so far.)
3. After resolving all inputs, read `depth` and decide where to store:
   - `Some(0)` — depends on local (scoped) data → store in local `types`.
   - `Some(n)` where `n >= 1` — the shallowest dependency lives in
     `ancestors[n - 1]` → store in `ancestors[n - 1]` via
     `get_or_insert()`.
   - `None` — no inputs (leaf type with no dependencies) → store in
     the deepest ancestor (`ancestors.last()`) to maximize sharing.
4. Restore the previous `depth`, then `mark()` the tier where the
   newly-resolved type *actually* ended up (so callers further up the
   chain see the correct depth).

```rust
fn mark(&mut self, tier: usize) {
    self.depth = Some(match self.depth {
        None => tier,
        Some(d) => d.min(tier),
    });
}
```

### Walk-through: two-level scoping (app → request)

Ancestors: `[request_shared]` (index 0)

| Resolution          | Inputs from                                     | Depth   | Stored in        |
|---------------------|-------------------------------------------------|---------|------------------|
| `CountedClient`     | `Validator` (ancestors[0]), `Clock` (ancestors[0]) | Some(1) | `ancestors[0]`   |
| `CorrelationVector` | `RequestContext` (local)                         | Some(0) | local            |
| `RequestHandler`    | `Client` (ancestors[0]), `CorrelationVector` (local) | Some(0) | local       |

### Walk-through: three-level scoping (app → request → task)

From a task scope, ancestors: `[request_shared, app_shared]`

| Resolution          | Inputs from                                      | Depth   | Stored in             |
|---------------------|--------------------------------------------------|---------|-----------------------|
| `Validator`         | `Scheduler` (ancestors[1])                        | Some(2) | `ancestors[1]` (app)  |
| `Client`            | `Validator` (ancestors[1]), `Clock` (ancestors[1]) | Some(2) | `ancestors[1]` (app)  |
| `CorrelationVector` | `RequestContext` (ancestors[0])                   | Some(1) | `ancestors[0]` (req)  |
| `TaskScopedClient`  | `RequestHandler` (ancestors[0]), `Task` (local)   | Some(0) | local                 |

### Reentrancy

Resolution is recursive: resolving type A may require constructing
dependency B, which may itself require constructing dependency C. The
save-restore in steps 1 and 4 forms an implicit call stack that
isolates each frame's depth tracking.

Trace of resolving `RequestHandler` (depends on `Client` and
`CorrelationVector`) where `CorrelationVector` is not yet resolved
and depends on `RequestContext` (local):

```
resolve(RequestHandler):                         // not found → construct
  saved₁ = depth, depth = None                   // step 1
│
│ resolve(Client):                               // found in ancestors[0]
│   mark(1)                                      // step 2 → depth = Some(1)
│
│ resolve(CorrelationVector):                    // not found → construct
│   saved₂ = Some(1), depth = None               // step 1 (inner)
│ │
│ │ resolve(RequestContext):                      // found locally
│ │   mark(0)                                    // step 2 → depth = Some(0)
│ │
│   depth = Some(0) → store CV locally           // step 3 (inner)
│   depth = saved₂ = Some(1), mark(0)            // step 4 (inner)
│                                                //   → depth = Some(min(1, 0)) = Some(0)
│
  depth = Some(0) → store RH locally             // step 3 ✓
  depth = saved₁, mark(0)                        // step 4
```

Each recursive `resolve()` saves the caller's accumulated depth,
works with a clean slate, and on completion restores the caller's
depth before marking where the result was actually stored. The outer
frame never sees intermediate state from the inner frame — it only
sees the final tier of the dependency it asked for.

### Thread safety analysis

- **Tracking state** (`depth: Option<usize>`): A plain field on
  `ScopedResolver`. Resolution takes `&mut self`, so only one thread
  can access the tracking state at a time. No synchronization needed.

- **Ancestor writes** (`get_or_insert`): Protected by the `Mutex`
  inside `SharedTypeMap`. If two siblings resolve the same type
  concurrently, both construct it, but `get_or_insert` keeps only the
  first and drops the second. The returned reference is valid for the
  lifetime of the `SharedTypeMap` (append-only invariant).

- **Ancestor reads** (`try_get`): Also go through the `Mutex`. A
  sibling that already stored a type will make it visible to others on
  subsequent lookups — no redundant construction on the next sibling.

### Trade-offs

- One `Option<usize>` read/write per dependency lookup — negligible
  overhead.
- No heap allocation or `TypeId` tracking.
- Types are promoted to the highest possible ancestor, maximizing
  sharing across siblings at every nesting level.
- The `get_or_insert` race on `SharedTypeMap` may cause one redundant
  construction per type per concurrent set of siblings — acceptable
  because it is bounded (at most once per type per scope creation wave)
  and the result is immediately dropped.
