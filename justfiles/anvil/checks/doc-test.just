# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier. Two cargo-test invocations cover both feature
# configurations: --all-features catches doctests that only compile
# with optional features enabled; the default-features run catches
# doctests that only compile when those features are off. The
# duplication is cheap because doctests are already a small fraction
# of total compile/test time.

# Run documentation tests for affected workspace packages.
[script("pwsh")]
anvil-doc-test: anvil-doc-test-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $docArgs = if ($pkg -contains '--workspace') {
        @('--workspace')
    } else {
        $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $byName = @{}
        foreach ($p in $meta.packages) { $byName[$p.name] = $p }

        $filteredArgs = [System.Collections.Generic.List[string]]::new()
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -ne '--package' -or ($i + 1) -ge $pkg.Count) { continue }
            $spec = $pkg[++$i]
            $name = ($spec -split '@', 2)[0]
            $p = $byName[$name]
            if ($p -and ($p.targets | Where-Object { $_.kind -contains 'lib' -and $_.doctest })) {
                $filteredArgs.Add('--package')
                $filteredArgs.Add($spec)
            }
        }
        @($filteredArgs)
    }
    if ($docArgs.Count -eq 0) {
        Write-Host 'anvil-doc-test: no affected packages with doctestable library targets; skipping'
        exit 0
    }
    & cargo test --doc @docArgs --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    & cargo test --doc @docArgs --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-doc-test` recipe.
[group("anvil-setup")]
anvil-doc-test-setup installer="install": anvil-tool-rustc-validate-prereqs

# Validate prerequisites for the `anvil-doc-test` recipe.
[group("anvil-setup")]
anvil-doc-test-validate-prereqs: anvil-tool-rustc-validate-prereqs
