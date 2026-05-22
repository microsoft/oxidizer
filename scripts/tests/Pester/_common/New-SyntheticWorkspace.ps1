# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Fixture builder for synthetic Cargo workspaces used by the release-script test
    suite. Dot-source from a Pester test; never run directly.

.DESCRIPTION
    Creates a temporary on-disk Cargo workspace under a chosen path, initialised as
    a Git repo with a single baseline commit. Exposes named topology presets
    (Linear2, Linear3, Linear4, Diamond4, Macros3, FanOut5, UpDown5, Mixed6,
    Detached) and a `-Spec` parameter for ad-hoc topologies.

    The returned object exposes mutation helpers (ModifySource, BumpVersion,
    SetPublishFalse, AddCommit, ...) so scenarios can build their pre-release
    git history declaratively.

    Synthetic workspaces use workspace inheritance for cross-crate dependencies
    (`bar.workspace = true`) which mirrors the real repo and side-steps a latent
    bug in `Update-CrateVersion` that mis-handles inline `path = "...", version
    = "..."` deps. The Update-CrateVersion bug is pinned by an integration test
    in Phase 5; fixtures intentionally use the production-shaped pattern.
#>

# --- TOPOLOGY PRESETS ---
#
# Each preset is a function returning a "spec" hashtable consumed by the
# generic builder. The Deps shape is:
#   @{ Name = '<dep>'; Kind = 'normal' | 'dev' | 'build' }
# Defaults: Kind = 'normal'. Per-crate defaults: Published = $true, Version = '0.1.0'.

function Get-PresetSpec {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('Linear2', 'Linear3', 'Linear4', 'Diamond4', 'Macros3',
                     'FanOut5', 'UpDown5', 'Mixed6', 'Detached')]
        [string]$Name
    )

    switch ($Name) {
        'Linear2' {
            return @{
                Crates = @(
                    @{ Name = 'downstream'; Version = '0.1.0'; Deps = @(@{ Name = 'upstream' }) }
                    @{ Name = 'upstream';   Version = '0.2.0' }
                )
            }
        }
        'Linear3' {
            return @{
                Crates = @(
                    @{ Name = 'a'; Version = '0.1.0'; Deps = @(@{ Name = 'b' }) }
                    @{ Name = 'b'; Version = '0.2.0'; Deps = @(@{ Name = 'c' }) }
                    @{ Name = 'c'; Version = '0.3.0' }
                )
            }
        }
        'Linear4' {
            return @{
                Crates = @(
                    @{ Name = 'a'; Version = '0.1.0'; Deps = @(@{ Name = 'b' }) }
                    @{ Name = 'b'; Version = '0.2.0'; Deps = @(@{ Name = 'c' }) }
                    @{ Name = 'c'; Version = '0.3.0'; Deps = @(@{ Name = 'd' }) }
                    @{ Name = 'd'; Version = '0.4.0' }
                )
            }
        }
        'Diamond4' {
            return @{
                Crates = @(
                    @{ Name = 'top';    Version = '0.1.0'; Deps = @(@{ Name = 'left' }, @{ Name = 'right' }) }
                    @{ Name = 'left';   Version = '0.2.0'; Deps = @(@{ Name = 'bottom' }) }
                    @{ Name = 'right';  Version = '0.3.0'; Deps = @(@{ Name = 'bottom' }) }
                    @{ Name = 'bottom'; Version = '0.4.0' }
                )
            }
        }
        'Macros3' {
            return @{
                Crates = @(
                    @{ Name = 'user';        Version = '0.1.0'; Deps = @(@{ Name = 'macros' }) }
                    @{ Name = 'macros';      Version = '0.2.0'; Deps = @(@{ Name = 'macros_impl' }) }
                    @{ Name = 'macros_impl'; Version = '0.3.0' }
                )
            }
        }
        'FanOut5' {
            return @{
                Crates = @(
                    @{ Name = 'user1';           Version = '0.1.0'; Deps = @(@{ Name = 'hub' }) }
                    @{ Name = 'user2';           Version = '0.2.0'; Deps = @(@{ Name = 'hub' }) }
                    @{ Name = 'user3';           Version = '0.3.0'; Deps = @(@{ Name = 'hub' }) }
                    @{ Name = 'hub';             Version = '0.4.0'; Deps = @(@{ Name = 'shared_upstream' }) }
                    @{ Name = 'shared_upstream'; Version = '0.5.0' }
                )
            }
        }
        'UpDown5' {
            return @{
                Crates = @(
                    @{ Name = 'downstream_x'; Version = '0.1.0'; Deps = @(@{ Name = 'target' }) }
                    @{ Name = 'downstream_y'; Version = '0.2.0'; Deps = @(@{ Name = 'target' }) }
                    @{ Name = 'target';       Version = '0.3.0'; Deps = @(@{ Name = 'upstream_a' }, @{ Name = 'upstream_b' }) }
                    @{ Name = 'upstream_a';   Version = '0.4.0' }
                    @{ Name = 'upstream_b';   Version = '0.5.0' }
                )
            }
        }
        'Mixed6' {
            return @{
                Crates = @(
                    @{ Name = 'target';       Version = '0.1.0'; Deps = @(
                        @{ Name = 'upstream_b' }
                        @{ Name = 'upstream_a'; Kind = 'dev' }
                    ) }
                    @{ Name = 'upstream_a';   Version = '0.3.0' }
                    @{ Name = 'upstream_b';   Version = '0.2.0' }
                    @{ Name = 'downstream_x'; Version = '0.4.0'; Deps = @(@{ Name = 'target' }) }
                    @{ Name = 'downstream_y'; Version = '0.5.0'; Deps = @(@{ Name = 'target' }, @{ Name = 'utility' }) }
                    @{ Name = 'utility';      Version = '0.6.0'; Published = $false }
                )
            }
        }
        'Detached' {
            return @{
                Crates = @(
                    @{ Name = 'alpha'; Version = '0.1.0'; Deps = @(@{ Name = 'beta' }) }
                    @{ Name = 'beta';  Version = '0.2.0' }
                    @{ Name = 'gamma'; Version = '0.3.0'; Deps = @(@{ Name = 'delta' }) }
                    @{ Name = 'delta'; Version = '0.4.0' }
                )
            }
        }
    }
}

# --- INTERNAL: SPEC -> ON-DISK ---

function Write-CrateCargoToml {
    param(
        [Parameter(Mandatory = $true)][hashtable]$Crate,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $lines = @(
        '[package]'
        "name = `"$($Crate.Name)`""
        "version = `"$($Crate.Version)`""
        'edition = "2021"'
        'description = "synthetic test crate"'
        'license = "MIT"'
    )
    if ($Crate.ContainsKey('Published') -and $Crate.Published -eq $false) {
        $lines += 'publish = false'
    }
    if ($Crate.ContainsKey('AllowedExternalTypes') -and $null -ne $Crate.AllowedExternalTypes) {
        $lines += ''
        $lines += '[package.metadata.cargo_check_external_types]'
        $entries = ($Crate.AllowedExternalTypes | ForEach-Object { "`"$_`"" }) -join ', '
        $lines += "allowed_external_types = [$entries]"
    }

    $allDeps = @()
    if ($null -ne $Crate.Deps) { $allDeps = @($Crate.Deps) }
    $deps      = @($allDeps | Where-Object { $null -ne $_ -and $_.Kind -ne 'dev' -and $_.Kind -ne 'build' })
    $buildDeps = @($allDeps | Where-Object { $null -ne $_ -and $_.Kind -eq 'build' })
    $devDeps   = @($allDeps | Where-Object { $null -ne $_ -and $_.Kind -eq 'dev' })

    if ($deps.Count -gt 0) {
        $lines += ''
        $lines += '[dependencies]'
        foreach ($d in $deps) {
            $lines += "$($d.Name).workspace = true"
        }
    }
    if ($buildDeps.Count -gt 0) {
        $lines += ''
        $lines += '[build-dependencies]'
        foreach ($d in $buildDeps) {
            $lines += "$($d.Name).workspace = true"
        }
    }
    if ($devDeps.Count -gt 0) {
        $lines += ''
        $lines += '[dev-dependencies]'
        foreach ($d in $devDeps) {
            $lines += "$($d.Name).workspace = true"
        }
    }

    Set-Content -Path $Path -Value ($lines -join "`n") -NoNewline
}

function Write-RootCargoToml {
    param(
        [Parameter(Mandatory = $true)][hashtable]$Spec,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $lines = @(
        '[workspace]'
        'resolver = "2"'
        'members = ["crates/*"]'
        ''
        '[workspace.dependencies]'
    )
    foreach ($crate in $Spec.Crates) {
        $lines += "$($crate.Name) = { path = `"crates/$($crate.Name)`", version = `"$($crate.Version)`" }"
    }

    Set-Content -Path $Path -Value ($lines -join "`n") -NoNewline
}

function Initialize-GitRepo {
    param([string]$Path)

    Push-Location $Path
    try {
        & git init --quiet --initial-branch=main 2>&1 | Out-Null
        & git config user.email 'test@example.com' 2>&1 | Out-Null
        & git config user.name  'Test User' 2>&1 | Out-Null
        & git config commit.gpgsign false 2>&1 | Out-Null
        & git add -A 2>&1 | Out-Null
        & git -c core.autocrlf=false commit --quiet -m 'baseline' --allow-empty 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "git init/commit failed in $Path"
        }
    } finally {
        Pop-Location
    }
}

# --- PUBLIC API ---

function New-SyntheticWorkspace {
    [CmdletBinding(DefaultParameterSetName = 'Preset')]
    param(
        [Parameter(Mandatory = $true, ParameterSetName = 'Preset')]
        [ValidateSet('Linear2', 'Linear3', 'Linear4', 'Diamond4', 'Macros3',
                     'FanOut5', 'UpDown5', 'Mixed6', 'Detached')]
        [string]$Preset,

        [Parameter(Mandatory = $true, ParameterSetName = 'Spec')]
        [hashtable]$Spec,

        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if ($PSCmdlet.ParameterSetName -eq 'Preset') {
        $Spec = Get-PresetSpec -Name $Preset
    }

    if (-not (Test-Path $Path)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
    }

    Write-RootCargoToml -Spec $Spec -Path (Join-Path $Path 'Cargo.toml')

    foreach ($crate in $Spec.Crates) {
        $crateDir = Join-Path $Path "crates\$($crate.Name)"
        $srcDir   = Join-Path $crateDir 'src'
        New-Item -ItemType Directory -Path $srcDir -Force | Out-Null
        Write-CrateCargoToml -Crate $crate -Path (Join-Path $crateDir 'Cargo.toml')
        Set-Content -Path (Join-Path $srcDir 'lib.rs') -Value "// $($crate.Name)" -NoNewline
        Set-Content -Path (Join-Path $crateDir 'CHANGELOG.md') -Value "# Changelog`n`n## [Unreleased]" -NoNewline
    }

    Initialize-GitRepo -Path $Path

    $ws = [pscustomobject]@{
        Path = (Resolve-Path $Path).Path
        Spec = $Spec
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'ModifySource' -Value {
        param([string]$Crate, [string]$Suffix = "// edit")
        $libPath = Join-Path $this.Path "crates\$Crate\src\lib.rs"
        if (-not (Test-Path $libPath)) {
            throw "ModifySource: crate '$Crate' not found at '$libPath'"
        }
        Add-Content -Path $libPath -Value "`n$Suffix"
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'WriteFile' -Value {
        param([string]$RelPath, [string]$Content)
        $full = Join-Path $this.Path $RelPath.Replace('/', '\')
        $parent = Split-Path $full -Parent
        if (-not (Test-Path $parent)) {
            New-Item -ItemType Directory -Path $parent -Force | Out-Null
        }
        Set-Content -Path $full -Value $Content -NoNewline
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'BumpVersion' -Value {
        param([string]$Crate, [string]$NewVersion)
        $cratePath = Join-Path $this.Path "crates\$Crate\Cargo.toml"
        $rootPath  = Join-Path $this.Path 'Cargo.toml'
        if (-not (Test-Path $cratePath)) {
            throw "BumpVersion: crate '$Crate' not found at '$cratePath'"
        }

        $content = Get-Content $cratePath -Raw
        $content = [regex]::Replace($content, '(?m)^version\s*=\s*"[^"]+"', "version = `"$NewVersion`"")
        Set-Content -Path $cratePath -Value $content -NoNewline

        $rootContent = Get-Content $rootPath -Raw
        $rootContent = [regex]::Replace(
            $rootContent,
            "(?m)^$([regex]::Escape($Crate))\s*=\s*\{[^}]*version\s*=\s*`"[^`"]+`"",
            { param($m) [regex]::Replace($m.Value, 'version\s*=\s*"[^"]+"', "version = `"$NewVersion`"") }
        )
        Set-Content -Path $rootPath -Value $rootContent -NoNewline
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'SetPublishFalse' -Value {
        param([string]$Crate)
        $cratePath = Join-Path $this.Path "crates\$Crate\Cargo.toml"
        if (-not (Test-Path $cratePath)) {
            throw "SetPublishFalse: crate '$Crate' not found at '$cratePath'"
        }
        $content = Get-Content $cratePath -Raw
        if ($content -match '(?m)^publish\s*=') {
            $content = [regex]::Replace($content, '(?m)^publish\s*=\s*[^\r\n]+', 'publish = false')
        } else {
            $content = [regex]::Replace($content, '(?m)^(version\s*=\s*"[^"]+")', "`$1`npublish = false")
        }
        Set-Content -Path $cratePath -Value $content -NoNewline
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'AddCommit' -Value {
        param([string]$Message)
        Push-Location $this.Path
        try {
            & git add -A 2>&1 | Out-Null
            & git -c core.autocrlf=false commit --quiet -m $Message --allow-empty 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "AddCommit failed: $Message"
            }
        } finally {
            Pop-Location
        }
    }

    $ws | Add-Member -MemberType ScriptMethod -Name 'GitSha' -Value {
        param([string]$Ref = 'HEAD')
        Push-Location $this.Path
        try {
            return (& git rev-parse $Ref 2>&1).ToString().Trim()
        } finally {
            Pop-Location
        }
    }

    return $ws
}
