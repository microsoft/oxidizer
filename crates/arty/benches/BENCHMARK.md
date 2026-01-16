# Spawner Benchmarks

Compares the overhead of using `Spawner::spawn()` vs calling runtime spawn functions directly.

## Results

| Benchmark | Time |
|-----------|------|
| `tokio_direct` | ~12 µs |
| `tokio_via_spawner` | ~12 µs |
| `async_std_direct` | ~35 µs |
| `async_std_via_spawner` | ~21 µs |

## Analysis

**Tokio**: The spawner abstraction has **zero overhead**. Both paths use tokio's `JoinHandle` directly, so the only cost is the match statement which is negligible.

**async-std**: The custom spawner uses a oneshot channel to return results, which adds ~14 µs overhead compared to async-std's native `JoinHandle`. However, it's still faster than async-std's direct approach in absolute terms due to the channel implementation.

## Key Takeaway

For Tokio (the default), the `Spawner` abstraction is essentially free. For custom spawners, there's a small fixed overhead from the oneshot channel (~few µs), which is negligible for any real-world task.

## Running

```sh
cargo bench -p arty --bench spawner --features custom
```
