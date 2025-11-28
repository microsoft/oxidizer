# cargo-ensure-no-cyclic-deps

A cargo subcommand that detects cyclic dependencies between crates in a workspace.

## Usage

Run this command in a cargo workspace:

```bash
cargo ensure-no-cyclic-deps
```

The command will:
- Analyze all workspace crates
- Check for cyclic dependencies (including dev-dependencies)
- Report any cycles found
- Exit with code 1 if cycles are detected, 0 otherwise

## Installation

```bash
cargo install --path .
```

Or from within the workspace:

```bash
cargo install cargo-ensure-no-cyclic-deps
```

## Example Output

When cycles are detected:

```
Error: Cyclic dependencies detected!

Cycle 1:
  crate_a -> crate_b -> crate_c -> crate_a

Cycle 2:
  crate_x -> crate_y -> crate_x
```

When no cycles are found:

```
No cyclic dependencies found.
```

## License

MIT
