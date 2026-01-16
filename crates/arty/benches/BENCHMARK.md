# Spawner Benchmarks

Compares the overhead of using `Spawner::spawn()` vs calling runtime spawn functions directly.

## Results

| Benchmark | Time |
|-----------|------|
| `tokio_direct` | ~12 µs |
| `tokio_via_spawner` | ~12 µs |
| `smol_direct` | ~2 µs |
| `smol_via_spawner` | ~2.6 µs |

## Analysis

**Tokio**: The spawner abstraction has **zero overhead**. Both paths use tokio's `JoinHandle` directly, so the only cost is the match statement which is negligible.

**smol**: The custom spawner adds ~0.6 µs overhead from the oneshot channel. smol is significantly faster than tokio in this benchmark.

## Key Takeaway

The `Spawner` abstraction has negligible overhead. For Tokio, it's essentially free since it uses the native `JoinHandle` directly. For custom spawners, the oneshot channel approach performs well.

## Running

```sh
cargo bench -p arty --bench spawner --features custom
```
