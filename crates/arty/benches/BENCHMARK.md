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

**async-std**: The custom spawner uses a oneshot channel to return results, which is ~14 µs faster than async-std's native `JoinHandle` in this benchmark.

## Key Takeaway

The `Spawner` abstraction has negligible overhead. For Tokio, it's essentially free since it uses the native `JoinHandle` directly. For custom spawners, the oneshot channel approach performs well.

## Running

```sh
cargo bench -p arty --bench spawner --features custom
```
