# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    $script:RepoRoot = Get-OxiRepoRoot
    $path = Join-Path $script:RepoRoot 'scripts\install-callgrind-tools.ps1'
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$null, [ref]$null)
    $function = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Initialize-CallgrindTools'
    }, $true)
    . ([scriptblock]::Create($function.Extent.Text))
    function gungraun-runner {}
    function cargo {}
}

Describe 'Callgrind tool setup' {
    BeforeEach {
        $script:InstalledVersion = '0.19.4'
        $script:ValgrindAvailable = $true
        $script:CargoCalls = @()
        Mock Get-Command {
            if ($Name -eq 'gungraun-runner' -and $script:InstalledVersion) { return $true }
            if ($Name -eq 'valgrind' -and $script:ValgrindAvailable) { return $true }
        } -ParameterFilter { $Name -in @('gungraun-runner', 'valgrind') }
        Mock gungraun-runner {
            $global:LASTEXITCODE = 0
            "gungraun-runner $script:InstalledVersion"
        }
        Mock cargo {
            $script:CargoCalls += ,@($args)
            $global:LASTEXITCODE = 0
        }
    }

    It 'keeps the exactly matching runner without installing anything' {
        Initialize-CallgrindTools -RequiredVersion '0.19.4'
        Should -Invoke cargo -Times 0
    }

    It 'replaces a <Version> runner with the exact required version' -TestCases @(
        @{ Version = '0.19.3' }
        @{ Version = '0.19.5' }
        @{ Version = $null }
    ) {
        param($Version)
        $script:InstalledVersion = $Version
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -CargoArguments @('+1.95')
        ($script:CargoCalls | ConvertTo-Json -Compress) |
            Should -Be '["+1.95","install","--locked","--force","--version","=0.19.4","gungraun-runner"]'
    }

    It 'falls back to source installation after binary installation fails' {
        $script:InstalledVersion = '0.19.5'
        Mock cargo {
            $script:CargoCalls += ,@($args)
            $global:LASTEXITCODE = if ($args[0] -eq 'binstall') { 1 } else { 0 }
        }
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -Installer binstall
        ($script:CargoCalls | ConvertTo-Json -Compress) | Should -Be (
            '[["binstall","--no-confirm","--locked","--force","--version","=0.19.4","gungraun-runner"],' +
            '["install","--locked","--force","--version","=0.19.4","gungraun-runner"]]'
        )
    }

    It 'uses the exact binary version without a source installation on success' {
        $script:InstalledVersion = '0.19.5'
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -Installer binstall
        ($script:CargoCalls | ConvertTo-Json -Compress) |
            Should -Be '["binstall","--no-confirm","--locked","--force","--version","=0.19.4","gungraun-runner"]'
    }

    It 'reports a failure to inspect the installed runner' {
        Mock gungraun-runner { $global:LASTEXITCODE = 1 }
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' } |
            Should -Throw '*Could not read the installed gungraun-runner version*'
        Should -Invoke cargo -Times 0
    }

    It 'reports an installation failure' {
        $script:InstalledVersion = $null
        Mock cargo { $global:LASTEXITCODE = 1 }
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' } |
            Should -Throw '*Could not install gungraun-runner 0.19.4*'
    }

    It 'validates provisioned tools without installing anything' {
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -ValidateOnly
        Should -Invoke cargo -Times 0
    }

    It 'rejects a mismatched runner during validation' {
        $script:InstalledVersion = '0.19.5'
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' -ValidateOnly } |
            Should -Throw '*gungraun-runner 0.19.4 is required*'
        Should -Invoke cargo -Times 0
    }

    It 'reports how to install missing Valgrind' {
        $script:ValgrindAvailable = $false
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' -ValidateOnly } |
            Should -Throw '*sudo apt-get install -y valgrind*'
    }

    It 'keeps the native tool cache pin equal to the workspace dependency' {
        $manifest = Get-Content (Join-Path $script:RepoRoot 'Cargo.toml') -Raw
        $version = [regex]::Match($manifest, '(?m)^gungraun\s*=\s*\{[^}]*version\s*=\s*"=([^"]+)"').Groups[1].Value
        $pins = Get-Content (Join-Path $script:RepoRoot 'justfiles\anvil\versions.just') -Raw
        $pin = [regex]::Match($pins, '(?m)^gungraun_runner_version\s*:=\s*"([^"]+)"').Groups[1].Value
        $version | Should -Not -BeNullOrEmpty
        $pin | Should -Be $version
    }
}

Describe 'Valgrind provisioning in GitHub Actions' {
    It 'selects <RunnerEnvironment> <OS> <Group>: <Selected>' -TestCases @(
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Linux'; Group = 'pr-test'; Selected = $true }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Linux'; Group = 'scheduled-test'; Selected = $true }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Linux'; Group = ''; Selected = $true }
        @{ RunnerEnvironment = 'self-hosted'; OS = 'Linux'; Group = 'pr-test'; Selected = $false }
        @{ RunnerEnvironment = 'self-hosted'; OS = 'Linux'; Group = 'scheduled-test'; Selected = $false }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Windows'; Group = 'pr-test'; Selected = $false }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'macOS'; Group = 'pr-test'; Selected = $false }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Linux'; Group = 'none'; Selected = $false }
        @{ RunnerEnvironment = 'github-hosted'; OS = 'Linux'; Group = 'pr-fast'; Selected = $false }
    ) {
        param($RunnerEnvironment, $OS, $Group, $Selected)
        $action = Get-Content (Join-Path $script:RepoRoot '.github\actions\anvil-setup\action.yml') -Raw
        $condition = [regex]::Match(
            $action,
            '(?m)^    - name: Install Valgrind \(Linux example groups\)\r?\n      if: ([^\r\n]+)'
        ).Groups[1].Value
        $condition | Should -Not -BeNullOrEmpty
        $runner = @{ environment = $RunnerEnvironment; os = $OS }
        $inputs = @{ group = $Group }
        $expression = $condition.Replace('runner.', '$runner.').Replace('inputs.', '$inputs.').
            Replace('&&', '-and').Replace('||', '-or').Replace('==', '-ceq')
        (& ([scriptblock]::Create($expression))) | Should -Be $Selected
    }
}
