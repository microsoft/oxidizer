# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
    Tests for the release skill's deterministic fact-gathering helper. Uses the
    synthetic-workspace
    fixture so the assertions are hermetic (no dependency on the real workspace).
#>

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:FactsScript = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\release-facts.ps1'

    function Invoke-ReleaseFacts {
        param([Parameter(Mandatory = $true)][string]$RepoRoot)
        Reset-ReleaseScriptCaches
        $json = & $script:FactsScript -RepoRoot $RepoRoot
        return ($json | ConvertFrom-Json)
    }
}

Describe 'release-facts.ps1' {
    BeforeAll {
        $script:WsRoot = Join-Path $TestDrive 'facts-ws'
        $spec = @{
            Packages = @(
                @{ Name = 'alpha';        Version = '0.1.0'; Deps = @(@{ Name = 'beta' }) }
                @{ Name = 'beta';         Version = '0.2.0' }
                @{
                    Name = 'exposer'
                    Version = '0.2.0'
                    Deps = @(@{ Name = 'beta' })
                    AllowedExternalTypes = @('beta::*', 'http::*', 'stale::*')
                }
                @{
                    Name = 'gamma_macros'
                    Version = '0.3.0'
                    ProcMacro = $true
                    Deps = @(@{ Name = 'beta' })
                    AllowedExternalTypes = @('gamma_macros::*')
                }
                @{
                    Name = 'macro_runtime'
                    Version = '0.3.0'
                    Deps = @(@{ Name = 'gamma_macros' })
                    AllowedExternalTypes = @('gamma_macros::derive_gamma')
                }
                @{
                    Name = 'wildcard_macro_user'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'gamma_macros' })
                    AllowedExternalTypes = @('*')
                }
                @{
                    Name = 'macro_intermediate'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'gamma_macros' })
                }
                @{
                    Name = 'transitive_wildcard_macro_user'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'macro_intermediate' })
                    AllowedExternalTypes = @('*')
                }
                @{
                    Name = 'renamed_macro_user'
                    Version = '0.1.0'
                    Deps = @(@{
                            Name = 'gamma_macros'
                            Rename = 'gamma_alias'
                        })
                    AllowedExternalTypes = @('gamma_macros::derive_gamma')
                }
                @{
                    Name = 'private_macro_user'
                    Version = '0.1.0'
                    Published = $false
                    Deps = @(@{ Name = 'gamma_macros' })
                    AllowedExternalTypes = @('gamma_macros::derive_gamma')
                }
                @{
                    Name = 'detached_macros'
                    Version = '0.1.0'
                    ProcMacro = $true
                    MacroRuntime = @('detached_runtime')
                }
                @{
                    Name = 'detached_runtime'
                    Version = '0.1.0'
                }
                @{
                    Name = 'devonly'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'beta'; Kind = 'dev' })
                }
                @{
                    Name = 'empty_exposer'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'beta' })
                    AllowedExternalTypes = @()
                }
                @{
                    Name = 'dual_dep'
                    Version = '0.1.0'
                    Deps = @(
                        @{ Name = 'beta' }
                        @{ Name = 'beta'; Kind = 'build' }
                    )
                }
                @{ Name = 'priv_pkg';     Version = '0.4.0'; Published = $false }
            )
        }
        $script:Ws = New-SyntheticWorkspace -Spec $spec -Path $script:WsRoot
        # Create an explicit version-bump commit for 'beta' so it has a real
        # baseline commit (parent 0.2.0 -> commit 0.5.0).
        $script:Ws.SetVersion('beta', '0.5.0')
        $script:Ws.AddCommit('bump beta to 0.5.0')

        # Tag 'beta' as released so everReleased distinguishes it from the
        # never-released crates (whose introducing commit also yields a baseline).
        & git -C $script:Ws.Path tag 'beta-v0.5.0' 2>&1 | Out-Null

        # Leave an uncommitted source edit on 'alpha' so it registers as modified.
        # The default suffix is a `// edit` comment, so alpha's own Rust source
        # change is doc/comment-only -- rustImplementationChanged must stay false.
        $script:Ws.ModifySource('alpha')

        # 'exposer' gets a real code addition, so its rustImplementationChanged
        # must be true, distinguishing an implementation edit from a comment one.
        $script:Ws.ModifySource('exposer', 'pub fn newly_added() -> i32 { 42 }')

        # Also modify the UNPUBLISHED 'priv_pkg'. This makes the "publish=false is
        # never surfaced" assertion meaningful: priv_pkg now has a real working-tree
        # change, so modified=false can only hold because the published filter
        # suppresses it -- not merely because nothing changed.
        $script:Ws.ModifySource('priv_pkg')

        $script:Facts = Invoke-ReleaseFacts -RepoRoot $script:WsRoot
        $script:ByFolder = @{}
        foreach ($p in $script:Facts.packages) { $script:ByFolder[$p.folder] = $p }
    }

    It 'emits every workspace package under crates/' {
        $script:Facts.schemaVersion | Should -Be 5
        $folders = @($script:Facts.packages | ForEach-Object { $_.folder }) | Sort-Object
        $folders | Should -Be @(
            'alpha',
            'beta',
            'detached_macros',
            'detached_runtime',
            'devonly',
            'dual_dep',
            'empty_exposer',
            'exposer',
            'gamma_macros',
            'macro_intermediate',
            'macro_runtime',
            'priv_pkg',
            'private_macro_user',
            'renamed_macro_user',
            'transitive_wildcard_macro_user',
            'wildcard_macro_user'
        )
    }

    It 'reports name, version and published flag' {
        $script:ByFolder['alpha'].name      | Should -Be 'alpha'
        $script:ByFolder['beta'].version    | Should -Be '0.5.0'
        $script:ByFolder['alpha'].published | Should -BeTrue
        $script:ByFolder['priv_pkg'].published | Should -BeFalse
    }

    It 'captures normal dependency edges (dev excluded)' {
        @($script:ByFolder['alpha'].deps) | Should -Contain 'beta'
        @($script:ByFolder['beta'].deps).Count | Should -Be 0
        @($script:ByFolder['devonly'].deps).Count | Should -Be 0
        @($script:ByFolder['dual_dep'].deps) | Should -Be @('beta')
    }

    It 'emits deterministic workspace exposure edges from external-type metadata' {
        $script:ByFolder['exposer'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['exposer'].exposedDeps) | Should -Be @('beta')
    }

    It 'fails closed for a direct dependency when exposure metadata is absent' {
        $script:ByFolder['alpha'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['alpha'].exposedDeps) | Should -Be @('beta')
    }

    It 'treats an explicit empty allowlist as no exposure for libraries' {
        $script:ByFolder['empty_exposer'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['empty_exposer'].exposedDeps).Count | Should -Be 0
    }

    It 'includes exposure properties for every package' {
        foreach ($p in $script:Facts.packages) {
            $p.PSObject.Properties.Name | Should -Contain 'exposedDeps'
            $p.PSObject.Properties.Name | Should -Contain 'exposureUnknown'
            $p.PSObject.Properties.Name | Should -Contain 'manifestOtherChanged'
        }
    }

    It 'flags proc-macro-only packages' {
        $script:ByFolder['gamma_macros'].procMacroOnly | Should -BeTrue
        $script:ByFolder['gamma_macros'].hasLibraryTarget | Should -BeFalse
        $script:ByFolder['beta'].procMacroOnly | Should -BeFalse
    }

    It 'does not treat proc-macro implementation dependencies as type exposure' {
        $script:ByFolder['gamma_macros'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['gamma_macros'].exposedDeps).Count | Should -Be 0
        @($script:ByFolder['gamma_macros'].macroImplementationClosure) |
            Should -Be @('beta')
    }

    It 'records public proc-macro edges separately from type exposure' {
        @($script:ByFolder['macro_runtime'].macroPublicDeps) |
            Should -Be @('gamma_macros')
        @($script:ByFolder['macro_runtime'].exposedDeps) |
            Should -Not -Contain 'gamma_macros'
    }

    It 'infers a generated-runtime partner from a public macro edge' {
        @($script:ByFolder['gamma_macros'].macroRuntimePartners) |
            Should -Be @('macro_runtime')
    }

    It 'does not infer macro publication from wildcard or unpublished consumers' {
        @($script:ByFolder['wildcard_macro_user'].macroPublicDeps).Count |
            Should -Be 0
        @($script:ByFolder['transitive_wildcard_macro_user'].macroPublicDeps).Count |
            Should -Be 0
        @($script:ByFolder['renamed_macro_user'].macroPublicDeps).Count |
            Should -Be 0
        @($script:ByFolder['gamma_macros'].macroRuntimePartners) |
            Should -Not -Contain 'private_macro_user'
        @($script:ByFolder['gamma_macros'].macroRuntimePartners) |
            Should -Not -Contain 'transitive_wildcard_macro_user'
        @($script:ByFolder['gamma_macros'].macroRuntimePartners) |
            Should -Not -Contain 'renamed_macro_user'
    }

    It 'retains an explicit exceptional runtime relationship' {
        @($script:ByFolder['detached_macros'].macroRuntimePartners) |
            Should -Be @('detached_runtime')
    }

    It 'resolves a baseline commit sha for a package with a prior version bump' {
        $script:ByFolder['beta'].hasBaseline | Should -BeTrue
        $script:ByFolder['beta'].baselineSha | Should -Match '^[0-9a-f]{40}$'
    }

    It 'includes a baselineSha property for every package (possibly null)' {
        foreach ($p in $script:Facts.packages) {
            $p.PSObject.Properties.Name | Should -Contain 'baselineSha'
        }
    }

    It 'distinguishes an ever-released crate from a never-released one via everReleased' {
        # beta is tagged 'beta-v0.5.0'; the others have no release tag. Every crate
        # has a baselineSha (its introducing commit counts as a bump), so
        # everReleased -- not hasBaseline -- is the real discriminator.
        $script:ByFolder['beta'].everReleased  | Should -BeTrue
        $script:ByFolder['alpha'].everReleased | Should -BeFalse
        $script:ByFolder['beta'].hasBaseline   | Should -BeTrue
        $script:ByFolder['alpha'].hasBaseline  | Should -BeTrue
    }

    It 'detects unreleased (working-tree) modifications' {
        $script:ByFolder['alpha'].modified | Should -BeTrue
        $script:ByFolder['alpha'].modifiedFileCount | Should -BeGreaterThan 0
        @($script:ByFolder['alpha'].modifiedFiles).Count |
            Should -Be $script:ByFolder['alpha'].modifiedFileCount
        $script:ByFolder['alpha'].modifiedFiles |
            Should -Contain 'crates/alpha/src/lib.rs'
        @($script:ByFolder['alpha'].manifestDependencyScopes).Count |
            Should -Be 0
        $script:ByFolder['beta'].modified | Should -BeFalse
    }

    It 'distinguishes a doc-comment-only edit from a real implementation edit' {
        # alpha's only source change is a `// edit` comment line.
        $script:ByFolder['alpha'].rustImplementationChanged | Should -BeFalse
        # exposer added a real `pub fn`.
        $script:ByFolder['exposer'].rustImplementationChanged | Should -BeTrue
        # An unmodified crate reports no implementation change.
        $script:ByFolder['beta'].rustImplementationChanged | Should -BeFalse
    }

    It 'handles dependency table variants without inheriting unrelated TOML sections' {
        $cases = @(
            @{
                Name = 'normal'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    $raw.Replace(
                        'beta.workspace = true',
                        'beta = { workspace = true, features = ["std"] }'
                    )
                }
                Expected = @('normal')
            },
            @{
                Name = 'padded normal header'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    $raw.Replace('[dependencies]', '[ dependencies ]').Replace(
                        'beta.workspace = true',
                        'beta = { workspace = true, features = ["std"] }'
                    )
                }
                Expected = @('normal')
            },
            @{
                Name = 'build removal'
                Deps = @(@{ Name = 'beta'; Kind = 'build' })
                Edit = {
                    param($raw)
                    $raw.Replace('beta.workspace = true', '')
                }
                Expected = @('build')
            },
            @{
                Name = 'dev'
                Deps = @(@{ Name = 'beta'; Kind = 'dev' })
                Edit = {
                    param($raw)
                    $raw.Replace(
                        'beta.workspace = true',
                        'beta = { workspace = true, features = ["testing"] }'
                    )
                }
                Expected = @('dev')
            },
            @{
                Name = 'target dev'
                Deps = @()
                Edit = {
                    param($raw)
                    "$raw`n`n[target.'cfg(windows)'.dev-dependencies]`nbeta.workspace = true"
                }
                Expected = @('dev')
            },
            @{
                Name = 'scope move'
                Deps = @(@{ Name = 'beta'; Kind = 'dev' })
                Edit = {
                    param($raw)
                    $raw.Replace('[dev-dependencies]', '[dependencies]')
                }
                Expected = @('normal', 'dev')
            },
            @{
                Name = 'target relocation'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    $raw.Replace(
                        '[dependencies]',
                        "[target.'cfg(unix)'.dependencies]"
                    )
                }
                Expected = @('normal')
            },
            @{
                Name = 'inline comment'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    $raw.Replace(
                        'beta.workspace = true',
                        'beta.workspace = true # explanation changed'
                    )
                }
                Expected = @()
            },
            @{
                Name = 'package features'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    "$raw`n`n[features]`ndefault = []`ntesting = [`"beta/testing`"]"
                }
                Expected = @('features')
            },
            @{
                Name = 'metadata'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    @"
$raw

[package.metadata.review.dependencies]
note = "not a Cargo dependency table"
"@
                }
                Expected = @()
            },
            @{
                Name = 'array table'
                Deps = @(@{ Name = 'beta' })
                Edit = {
                    param($raw)
                    @"
$raw

[[example]]
name = "demo"
path = "examples/demo.rs"
"@
                }
                Expected = @()
                OtherChanged = $true
                Example = $true
            }
        )

        foreach ($case in $cases) {
            $root = Join-Path $TestDrive "manifest-$($case.Name.Replace(' ', '-'))"
            $ws = New-SyntheticWorkspace -Path $root -Spec @{
                Packages = @(
                    @{ Name = 'alpha'; Version = '0.1.0'; Deps = $case.Deps }
                    @{ Name = 'beta'; Version = '0.1.0' }
                )
            }
            if ($case.Example) {
                $exampleDir = Join-Path $ws.Path 'crates\alpha\examples'
                New-Item -ItemType Directory -Path $exampleDir | Out-Null
                Set-Content `
                    -LiteralPath (Join-Path $exampleDir 'demo.rs') `
                    -Value 'fn main() {}' `
                    -NoNewline
            }
            $manifest = Join-Path $ws.Path 'crates\alpha\Cargo.toml'
            $raw = Get-Content -LiteralPath $manifest -Raw
            Set-Content `
                -LiteralPath $manifest `
                -Value (& $case.Edit $raw) `
                -NoNewline

            $facts = Invoke-ReleaseFacts -RepoRoot $ws.Path
            $alpha = $facts.packages | Where-Object folder -eq alpha
            @($alpha.manifestDependencyScopes) | Should -Be $case.Expected
            $alpha.manifestOtherChanged |
                Should -Be ([bool]($case.OtherChanged ?? $false))
        }
    }

    It 'detects case-only manifest changes ordinally' {
        $root = Join-Path $TestDrive 'manifest-case-only'
        $ws = New-SyntheticWorkspace -Path $root -Spec @{
            Packages = @(
                @{ Name = 'alpha'; Version = '0.1.0' }
            )
        }
        $manifest = Join-Path $ws.Path 'crates\alpha\Cargo.toml'
        $raw = Get-Content -LiteralPath $manifest -Raw
        Set-Content `
            -LiteralPath $manifest `
            -Value "$raw`n`n[features]`ndefault = []`nstd = []" `
            -NoNewline
        & git -C $ws.Path add . 2>&1 | Out-Null
        & git -C $ws.Path commit --amend --no-edit 2>&1 | Out-Null
        (Get-Content -LiteralPath $manifest -Raw).Replace('std = []', 'STD = []') |
            Set-Content -LiteralPath $manifest -NoNewline

        $facts = Invoke-ReleaseFacts -RepoRoot $ws.Path
        $alpha = $facts.packages | Where-Object folder -eq alpha
        @($alpha.manifestDependencyScopes) | Should -Be @('features')

        $targetRoot = Join-Path $TestDrive 'manifest-case-only-target'
        $targetWs = New-SyntheticWorkspace -Path $targetRoot -Spec @{
            Packages = @(
                @{ Name = 'alpha'; Version = '0.1.0' }
                @{ Name = 'beta'; Version = '0.1.0' }
            )
        }
        $targetManifest = Join-Path $targetWs.Path 'crates\alpha\Cargo.toml'
        $targetRaw = Get-Content -LiteralPath $targetManifest -Raw
        Set-Content `
            -LiteralPath $targetManifest `
            -Value "$targetRaw`n`n[target.'cfg(target_os = `"linux`")'.dependencies]`nbeta.workspace = true" `
            -NoNewline
        & git -C $targetWs.Path add . 2>&1 | Out-Null
        & git -C $targetWs.Path commit --amend --no-edit 2>&1 | Out-Null
        (Get-Content -LiteralPath $targetManifest -Raw).Replace(
            'target_os = "linux"',
            'target_os = "LINUX"'
        ) | Set-Content -LiteralPath $targetManifest -NoNewline

        $targetFacts = Invoke-ReleaseFacts -RepoRoot $targetWs.Path
        $targetAlpha = $targetFacts.packages | Where-Object folder -eq alpha
        @($targetAlpha.manifestDependencyScopes) | Should -Be @('normal')
    }

    It 'never surfaces publish=false packages as modified' {
        # priv_pkg HAS an uncommitted source edit (see BeforeAll), yet
        # Get-PackagesWithUnreleasedChanges skips it because it is unpublished. If
        # the published filter were removed, this assertion would fail.
        $script:ByFolder['priv_pkg'].modified | Should -BeFalse
        $script:ByFolder['priv_pkg'].workspaceModified | Should -BeTrue
        $script:ByFolder['priv_pkg'].modifiedFiles |
            Should -Contain 'crates/priv_pkg/src/lib.rs'
    }

    It 'fails loudly for an unresolvable base ref instead of reporting no baseline' {
        { & $script:FactsScript -RepoRoot $script:WsRoot -BaseRef 'refs/heads/no-such-ref-xyz' } |
            Should -Throw '*could not be resolved*'
    }
}

Describe 'release-facts.ps1 compile-fixture obligations' {
    BeforeAll {
        # A proc macro whose compile fixtures live in its runtime partner -- the
        # shape that let a rejected-input break ship as a patch, because in the
        # partner the fixture reads as an ordinary test-only edit.
        $script:FixWsRoot = Join-Path $TestDrive 'fixture-ws'
        $spec = @{
            Packages = @(
                @{
                    Name = 'gamma_macros'
                    Version = '0.3.0'
                    ProcMacro = $true
                    AllowedExternalTypes = @('gamma_macros::*')
                }
                @{
                    Name = 'macro_runtime'
                    Version = '0.3.0'
                    Deps = @(@{ Name = 'gamma_macros' })
                    AllowedExternalTypes = @('gamma_macros::derive_gamma')
                }
                @{
                    Name = 'detached_macros'
                    Version = '0.1.0'
                    ProcMacro = $true
                    MacroRuntime = @('detached_runtime')
                }
                @{ Name = 'detached_runtime'; Version = '0.1.0' }
            )
        }
        $script:FixWs = New-SyntheticWorkspace -Spec $spec -Path $script:FixWsRoot

        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/existing.rs', 'fn main() {}')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/existing.stderr', 'error: old')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/gone.rs', 'fn main() {}')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/gone.stderr', 'error: gone')
        $script:FixWs.AddCommit('add ui fixtures')

        # The version bump is the release baseline every obligation is diffed
        # against, and the fixtures above predate it.
        $script:FixWs.SetVersion('macro_runtime', '0.4.0')
        $script:FixWs.AddCommit('bump macro_runtime to 0.4.0')
        $script:FixBaselineRev = $script:FixWs.GitSha('HEAD')

        # Unreleased window: one expectation rewritten, one case newly rejected,
        # one fixture deleted, one case with no recorded expectation at all.
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/existing.stderr', 'error: new')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/reject_case.rs', 'fn main() {}')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/reject_case.stderr', 'error: rejected')
        $script:FixWs.WriteFile('crates/macro_runtime/tests/ui/plain_case.rs', 'fn main() {}')
        Remove-Item -LiteralPath (
            Join-Path $script:FixWsRoot 'crates\macro_runtime\tests\ui\gone.rs'
        )
        Remove-Item -LiteralPath (
            Join-Path $script:FixWsRoot 'crates\macro_runtime\tests\ui\gone.stderr'
        )

        $script:FixFacts = Invoke-ReleaseFacts -RepoRoot $script:FixWsRoot
        $script:FixByFolder = @{}
        foreach ($p in $script:FixFacts.packages) { $script:FixByFolder[$p.folder] = $p }
        $script:MacroFixtures = @(
            $script:FixByFolder['gamma_macros'].macroCompileFixtureChanges
        )
    }

    It 'collects fixture changes owned by a runtime partner onto the macro' {
        $script:FixByFolder['gamma_macros'].macroRuntimePartners |
            Should -Contain 'macro_runtime'
        @($script:MacroFixtures | ForEach-Object { $_.ownerPackage }) |
            Should -Not -Contain 'gamma_macros'
        @($script:MacroFixtures | ForEach-Object { $_.ownerPackage }) |
            Sort-Object -Unique |
            Should -Be @('macro_runtime')
        @($script:MacroFixtures | ForEach-Object { $_.scopeRole }) |
            Sort-Object -Unique |
            Should -Be @('runtimePartner')
    }

    It 'classifies added, modified and removed fixture paths' {
        $byPath = @{}
        foreach ($item in $script:MacroFixtures) { $byPath[$item.path] = $item }

        $byPath['crates/macro_runtime/tests/ui/existing.stderr'].status |
            Should -Be 'modified'
        $byPath['crates/macro_runtime/tests/ui/reject_case.rs'].status |
            Should -Be 'added'
        $byPath['crates/macro_runtime/tests/ui/reject_case.stderr'].status |
            Should -Be 'added'
        $byPath['crates/macro_runtime/tests/ui/gone.rs'].status |
            Should -Be 'removed'
        $byPath['crates/macro_runtime/tests/ui/plain_case.rs'].status |
            Should -Be 'added'
        # existing.rs itself never changed, so it is not an obligation.
        $byPath.ContainsKey('crates/macro_runtime/tests/ui/existing.rs') |
            Should -BeFalse
    }

    It 'derives expectedResult only where a recorded expectation exists' {
        $byPath = @{}
        foreach ($item in $script:MacroFixtures) { $byPath[$item.path] = $item }

        $byPath['crates/macro_runtime/tests/ui/reject_case.rs'].kind |
            Should -Be 'uiFixture'
        $byPath['crates/macro_runtime/tests/ui/reject_case.rs'].expectedResult |
            Should -Be 'fail'
        $byPath['crates/macro_runtime/tests/ui/reject_case.stderr'].kind |
            Should -Be 'uiExpectation'
        $byPath['crates/macro_runtime/tests/ui/reject_case.stderr'].expectedResult |
            Should -Be 'fail'
        # No sibling expectation on either side: the outcome is not mechanically
        # discoverable, so the fact refuses to guess.
        $byPath['crates/macro_runtime/tests/ui/plain_case.rs'].expectedResult |
            Should -BeNullOrEmpty
    }

    It 'records the owner package, published flag and baseline revision' {
        foreach ($item in $script:MacroFixtures) {
            $item.ownerPackage | Should -Be 'macro_runtime'
            $item.ownerPublished | Should -BeTrue
            $item.baselineRev | Should -Be $script:FixBaselineRev
        }
    }

    It 'emits obligations in a deterministic ordinal order' {
        $paths = @($script:MacroFixtures | ForEach-Object { $_.path })
        $sorted = [string[]]@($paths)
        [Array]::Sort($sorted, [StringComparer]::Ordinal)
        $paths | Should -Be $sorted

        $again = Invoke-ReleaseFacts -RepoRoot $script:FixWsRoot
        $againMacro = $again.packages | Where-Object folder -eq 'gamma_macros'
        @($againMacro.macroCompileFixtureChanges | ForEach-Object { $_.path }) |
            Should -Be $paths
    }

    It 'leaves unrelated packages without obligations' {
        @($script:FixByFolder['macro_runtime'].macroCompileFixtureChanges).Count |
            Should -Be 0
        @($script:FixByFolder['detached_macros'].macroCompileFixtureChanges).Count |
            Should -Be 0
    }
}

Describe 'release-facts.ps1 external dependency exposure' {
    BeforeAll {
        # `syn` is inherited from [workspace.dependencies] by four crates that
        # differ only in what they expose, so one root-manifest edit produces
        # every outcome the lane has to tell apart.
        $script:ExtWsRoot = Join-Path $TestDrive 'external-ws'
        $spec = @{
            ExternalDependencies = @{
                syn   = '2.0.111'
                serde = '1.0.200'
            }
            Packages = @(
                @{
                    Name = 'exposing_impl'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'syn'; External = $true })
                    AllowedExternalTypes = @('syn::error::*')
                }
                @{
                    Name = 'private_user'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'syn'; External = $true })
                    AllowedExternalTypes = @('serde::Serialize')
                }
                @{
                    Name = 'syn_macros'
                    Version = '0.1.0'
                    ProcMacro = $true
                    Deps = @(@{ Name = 'syn'; External = $true })
                }
                @{
                    Name = 'unknown_exposure'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'syn'; External = $true })
                }
                @{
                    Name = 'dev_only_user'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'syn'; External = $true; Kind = 'dev' })
                    AllowedExternalTypes = @()
                }
                @{
                    Name = 'inline_user'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'serde'; External = $true; Version = '1.0.200' })
                    AllowedExternalTypes = @('serde::Serialize')
                }
            )
        }
        $script:ExtWs = New-SyntheticWorkspace -Spec $spec -Path $script:ExtWsRoot
        $script:ExtWs.AddCommit('baseline release state')

        # Unreleased window: the workspace-inherited requirement crosses a
        # compatibility line, and one crate pins its own inline requirement.
        $script:ExtWs.SetWorkspaceDependencyVersion('syn', '3.0.2')
        $script:ExtWs.SetPackageDependencyVersion('inline_user', 'serde', '2.0.0')

        $script:ExtFacts = Invoke-ReleaseFacts -RepoRoot $script:ExtWsRoot
        $script:ExtByFolder = @{}
        foreach ($p in $script:ExtFacts.packages) { $script:ExtByFolder[$p.folder] = $p }
    }

    It 'detects a workspace-inherited requirement change against the package baseline' {
        $changes = @($script:ExtByFolder['exposing_impl'].externalDepChanges)
        $changes.Count | Should -Be 1
        $changes[0].name | Should -Be 'syn'
        $changes[0].baselineReq | Should -Be '^2.0.111'
        $changes[0].currentReq | Should -Be '^3.0.2'
        $changes[0].breaking | Should -BeTrue
        $changes[0].kinds | Should -Be @('normal')
    }

    It 'detects a directly declared requirement change' {
        $changes = @($script:ExtByFolder['inline_user'].externalDepChanges)
        $changes.Count | Should -Be 1
        $changes[0].name | Should -Be 'serde'
        $changes[0].baselineReq | Should -Be '^1.0.200'
        $changes[0].currentReq | Should -Be '^2.0.0'
        $changes[0].breaking | Should -BeTrue
    }

    It 'reports exposure only where the allowlist admits the dependency' {
        $script:ExtByFolder['exposing_impl'].externalExposedDeps |
            Should -Be @('syn')
        @($script:ExtByFolder['private_user'].externalExposedDeps) |
            Should -Not -Contain 'syn'
    }

    It 'never exposes a foreign type identity through a proc macro' {
        @($script:ExtByFolder['syn_macros'].externalDepChanges).Count |
            Should -BeGreaterThan 0
        @($script:ExtByFolder['syn_macros'].externalExposedDeps).Count |
            Should -Be 0
    }

    It 'fails closed when the crate declares no exposure metadata' {
        $script:ExtByFolder['unknown_exposure'].externalExposedDeps |
            Should -Be @('syn')
    }

    It 'ignores dev-only external dependencies' {
        @($script:ExtByFolder['dev_only_user'].externalDepChanges).Count |
            Should -Be 0
        @($script:ExtByFolder['dev_only_user'].externalExposedDeps).Count |
            Should -Be 0
    }

    It 'promotes a package whose only change is the inherited requirement' {
        # Nothing under crates/private_user/ was touched, so without the
        # promotion the crate would never reach review at all.
        $script:ExtByFolder['private_user'].modifiedFileCount | Should -Be 0
        $script:ExtByFolder['private_user'].modified | Should -BeTrue
        $script:ExtByFolder['private_user'].workspaceModified | Should -BeTrue
        $script:ExtByFolder['private_user'].manifestDependencyScopes |
            Should -Contain 'normal'
    }

    It 'emits changes in a deterministic ordinal order' {
        foreach ($fact in $script:ExtFacts.packages) {
            $names = @($fact.externalDepChanges | ForEach-Object { $_.name })
            $sorted = [string[]]@($names)
            [Array]::Sort($sorted, [StringComparer]::Ordinal)
            $names | Should -Be $sorted
        }
    }

    It 'reports the same facts on a repeated run' {
        $again = Invoke-ReleaseFacts -RepoRoot $script:ExtWsRoot
        ($again.packages | ConvertTo-Json -Depth 8) |
            Should -Be ($script:ExtFacts.packages | ConvertTo-Json -Depth 8)
    }
}
