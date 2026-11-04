# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
    $script:Just = (Get-Command just -ErrorAction Stop).Source
    $script:Pwsh = (Get-Command pwsh -ErrorAction Stop).Source
    $script:Shell = Join-Path $TestDrive 'release-shell.ps1'
    @'
param([string]$Command)
$ErrorActionPreference = 'Stop'
function cargo {
    [System.IO.File]::WriteAllText(
        $env:RELEASE_TEST_OUTPUT,
        (ConvertTo-Json -InputObject @($args) -Compress))
}
& ([scriptblock]::Create($Command))
'@ | Set-Content -LiteralPath $script:Shell
}

Describe 'GitHub release argument forwarding' {
    It 'passes <Name> arguments to Cargo without evaluating them' -ForEach @(
        @{
            Name = 'ordinary'
            Repository = 'microsoft/oxidizer'
            Tag = 'data_privacy-v0.13.1'
        }
        @{
            Name = 'command substitution'
            Repository = 'owner/$(throw "repository evaluated")'
            Tag = 'crate-$(throw "tag evaluated")-v1.2.3'
        }
        @{
            Name = 'quotes and shell delimiters'
            Repository = 'owner/repo"; throw "repository evaluated"; #'
            Tag = 'crate''"`;$(throw "tag evaluated")-v1.2.3'
        }
    ) {
        $output = Join-Path $TestDrive "$([System.Guid]::NewGuid()).json"
        $startInfo = [System.Diagnostics.ProcessStartInfo]::new($script:Just)
        $startInfo.UseShellExecute = $false
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $startInfo.WorkingDirectory = $script:RepoRoot
        $startInfo.Environment['RELEASE_TEST_OUTPUT'] = $output
        [void]$startInfo.Environment.Remove('GH_TOKEN')
        [void]$startInfo.Environment.Remove('GITHUB_TOKEN')

        foreach ($argument in @(
            '--justfile', (Join-Path $script:RepoRoot 'justfile'),
            '--no-deps', '--clear-shell-args',
            '--shell', $script:Pwsh,
            '--shell-arg=-NoProfile', '--shell-arg=-NonInteractive',
            '--shell-arg=-File', "--shell-arg=$script:Shell",
            'publish-gh-release', '--repository', $Repository, '--tag', $Tag
        )) {
            $startInfo.ArgumentList.Add($argument)
        }

        $process = [System.Diagnostics.Process]::Start($startInfo)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(30000)) {
                $process.Kill($true)
                throw 'Release argument forwarding timed out.'
            }
            $process.ExitCode | Should -Be 0 -Because ($stdout.Result + $stderr.Result)
        } finally {
            $process.Dispose()
        }

        $actual = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
        $actual.Count | Should -Be 6
        ConvertTo-Json -InputObject @($actual[1..5]) -Compress |
            Should -Be (ConvertTo-Json -InputObject @(
                '-Zscript', 'scripts/publish-gh-release.rs', '--repo', $Repository, $Tag
            ) -Compress)
    }
}
