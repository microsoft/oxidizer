# Fixing Nightly Clippy Lints with MCP Tools

You are a Rust developer working on a project that uses the `ms-nightly` toolchain and enforces strict Clippy lints.
Your task is to identify and fix all Clippy warnings and errors using the MCP (Microsoft Code Platform) tools.
For all requests, you shall use `ms-nightly` toolchain and MCP tools to ensure compatibility with the latest features and lints.

## Instructions

0. **Identify Crates in Your Workspace**
    - Before starting, ensure you have a clear understanding of the crates in your workspace. 
    - Create a list of all crates in the workspace by listing directories in `crates/` and `crates_internal/` directories.
    - Sort the crates in logical order by crates with least dependencies.
      You can get the list of dependencies by running `#cargo-info` with `verbose` on each crate.

1. **Fix Lints Crate by Crate**
    - For each crate in your workspace, perform the following steps before moving to the next crate.

2. **Run Clippy with Nightly Toolchain**
    - Use the `ms-nightly` toolchain to run Clippy on the current crate. Use the `#cargo-clippy` tool for this, targeting only the specific crate.

3. **Automatically Fix Lints with `fix`**
    - Use the `fix` parameter with `#cargo-clippy` to automatically apply suggested fixes for applicable lints in the current crate.
    - Review the changes made by the fix process to ensure correctness.

4. **Analyze Clippy Output**
    - Review the Clippy output for any remaining warnings or errors in the current crate.
    - Pay special attention to lints that are only available on `ms-nightly`.

5. **Manually Fix Remaining Lints**
    - For each remaining lint:
      - Read the lint message and suggested fix.
      - Update your code to resolve the issue.
      - Prefer code changes over suppressing lints with `#[allow(...)]`, unless suppression is justified and documented.
    - If a lint is unclear, consult the [Clippy documentation](https://rust-lang.github.io/rust-clippy/master/) for details.
    - If a lint suppression is not necessary anymore, remove the `#[expect(...)]` attribute from the code.

6. **Verify and Commit**
    - Re-run Clippy on the current crate to ensure all lints are fixed.
    - Run `#cargo-test` for the current crate to verify no regressions.
    - Commit your changes with a message referencing the lints fixed for that crate.

7. **Repeat for All Crates**
    - Repeat steps 2–6 for each crate in your workspace.

## Tips

- Always use the latest `ms-nightly` toolchain and MCP tools for best results.
- Use `all-features` and `all-targets` flags with MCP tools to ensure all lints are checked across all features and targets of the current crate.
- If you encounter unstable or experimental lints, document any workarounds or limitations.
  Use `#[expect(clippy::lint_name, reason = "")]` sparingly and only when absolutely necessary, providing a clear reason.
