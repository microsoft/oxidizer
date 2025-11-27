
$ErrorActionPreference = "Stop"

$jobs = 4
$build_timeout = 600
$timeout = 300


# Define test groups for mutants testing
$test_groups = @(
    @("butesbuf"),
    @("data_privacy", "data_privacy_macros"),
    @("fundle", "fundle_macros", "fundle_macros_impl"),
    @("ohno", "ohno_macros"),
    @("thread_aware", "thread_aware_macros", "thread_aware_macros_impl")
)

# Crates to skip from mutants testing
$skip = @("testing_aids")


$all_crates = Get-ChildItem -Directory $PSScriptRoot/../crates | ForEach-Object { $_.Name }
$not_included = $all_crates | Where-Object {
    $crate = $_
    $in_group = $false
    foreach ($group in $test_groups) {
        if ($group -contains $crate) {
            $in_group = $true
            break
        }
    }
    (-not $in_group -and $skip -notcontains $crate)
}

if ($not_included.Count -gt 0) {
    Write-Warning "The following crates are not included in any test group or skip list:`n$($not_included -join "`n")"
    Write-Warning "They will be tested individually."
}


function mutate_group($group) {
    $crates = $group -join ","
    Write-Host "Mutating group: $crates"

    $args = @(
        "--no-shuffle",
        "--baseline=skip",
        "--package=$crates",
        "--colors=never",
        "--jobs=$jobs",
        "--build-timeout=$build_timeout",
        "--timeout=$timeout",
        "-vV"
    )

    $mutate_command = "cargo mutants " + ($args -join " ")
    Write-Host "Running command: $mutate_command"

    Invoke-Expression $mutate_command
}

foreach ($group in $test_groups) {
    mutate_group $group
}
foreach ($crate in $not_included) {
    mutate_group @($crate)
}