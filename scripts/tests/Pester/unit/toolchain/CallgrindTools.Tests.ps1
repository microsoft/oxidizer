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
        Initialize-CallgrindTools -RequiredVersion '0.19.4'
        $script:CargoCalls.Count | Should -Be 1
        $arguments = $script:CargoCalls[0]
        $arguments | Should -Contain 'gungraun-runner'
        $arguments | Should -Contain '--force'
        $versionIndex = [array]::IndexOf($arguments, '--version')
        $versionIndex | Should -BeGreaterOrEqual 0
        $arguments[$versionIndex + 1] | Should -Be '=0.19.4'
    }

    It 'falls back to source installation after binary installation fails' {
        $script:InstalledVersion = '0.19.5'
        Mock cargo {
            $script:CargoCalls += ,@($args)
            $global:LASTEXITCODE = if ($args[0] -eq 'binstall') { 1 } else { 0 }
        }
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -Installer binstall
        $script:CargoCalls.Count | Should -Be 2
        $script:CargoCalls[0][0] | Should -Be 'binstall'
        $script:CargoCalls[1][0] | Should -Be 'install'
    }

    It 'does not fall back to source after binary installation succeeds' {
        $script:InstalledVersion = '0.19.5'
        Initialize-CallgrindTools -RequiredVersion '0.19.4' -Installer binstall
        $script:CargoCalls.Count | Should -Be 1
        $script:CargoCalls[0][0] | Should -Be 'binstall'
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

    It 'rejects a <Version> runner during validation without installing anything' -TestCases @(
        @{ Version = '0.19.3' }
        @{ Version = '0.19.5' }
        @{ Version = $null }
    ) {
        param($Version)
        $script:InstalledVersion = $Version
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' -ValidateOnly } |
            Should -Throw '*gungraun-runner 0.19.4 is required*'
        Should -Invoke cargo -Times 0
    }

    It 'rejects missing Valgrind during validation' {
        $script:ValgrindAvailable = $false
        { Initialize-CallgrindTools -RequiredVersion '0.19.4' -ValidateOnly } |
            Should -Throw '*Valgrind is required*'
    }
}
