# Runs all stand-alone example binaries in the workspace (or the specified package).
# Each example has a 30-second timeout to avoid misbehaving examples causing trouble.
# Supports both .rs files and subdirectories with main.rs files.
# Sets IS_TESTING=1 environment variable for each example run, to allow "test mode" differentiation.

param(
    [Parameter(Mandatory = $true)]
    [string]$Profile,

    [Parameter(Mandatory = $false)]
    [string]$Package = ""
)

$ErrorActionPreference = "Stop"
# We disable this because we manually handle exit codes and do not want to fail fast.
$PSNativeCommandUseErrorActionPreference = $false

# We will make the assumption that all the packages are in the "crates" folder
$packages_root = Join-Path $PSScriptRoot "../crates"

# Examples that are expected to panic or fail and should be excluded from run-examples.
$excluded_examples = @(
    # This is an interactive example that requires user input.
    "employees"
)

# Discover and run all stand-alone example binaries
$failures = @()
$total_count = 0
$success_count = 0
$timeout_seconds = 30

# Determine which packages to process
$packages_to_process = @()
if ($Package -eq "") {
    # Get all workspace members (assuming here they are just subdirectories).
    $workspace_members = Get-ChildItem -Path $packages_root -Directory | Where-Object { Test-Path (Join-Path $_.FullName "Cargo.toml") }
    $packages_to_process = $workspace_members | ForEach-Object { $_.Name }
}
else {
    $packages_to_process = @($Package)
}

Write-Host "Running examples for packages: $($packages_to_process -join ', ')"
Write-Host "Timeout per example: $timeout_seconds seconds"
Write-Host "Cargo profile: $Profile"
Write-Host ""

foreach ($pkg in $packages_to_process) {
    $examples_dir = Join-Path $packages_root $pkg "examples"

    # Skip packages without examples directory (early continue pattern)
    if (-not (Test-Path $examples_dir)) {
        Write-Host "No examples directory found for package '$pkg'" -ForegroundColor DarkGray
        continue
    }

    # Find .rs files directly in examples directory
    $example_files = Get-ChildItem -Path $examples_dir -Filter "*.rs" | Where-Object { $_.Name -ne "mod.rs" }

    # Find subdirectories with main.rs files
    $example_subdirs = Get-ChildItem -Path $examples_dir -Directory | Where-Object {
        Test-Path (Join-Path $_.FullName "main.rs")
    }

    # Process .rs files
    foreach ($example_file in $example_files) {
        $example_name = $example_file.BaseName

        # Skip excluded examples
        if ($excluded_examples -contains $example_name) {
            Write-Host "Skipping excluded example '$example_name' in package '$pkg'" -ForegroundColor DarkGray
            continue
        }

        $total_count++

        Write-Host "Running example '$example_name' in package '$pkg'..." -ForegroundColor Cyan

        try {
            # Run the example with a timeout to prevent hanging
            $job = Start-Job -ScriptBlock {
                param($pkg, $example_name, $profile)
                $env:IS_TESTING = "1"
                & cargo run --package $pkg --example $example_name --profile $profile --all-features --locked 2>&1
                return $LASTEXITCODE
            } -ArgumentList $pkg, $example_name, $Profile

            $completed = Wait-Job -Job $job -Timeout $timeout_seconds

            if ($completed) {
                $result = Receive-Job -Job $job
                $exit_code = $result[-1]  # Last item should be the exit code
                $output = $result[0..($result.Length - 2)] -join "`n"  # All output except exit code

                if ($exit_code -eq 0) {
                    Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                    $success_count++
                }
                else {
                    Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
                    if ($output.Trim() -ne "") {
                        Write-Host "Output:" -ForegroundColor Yellow
                        Write-Host $output -ForegroundColor DarkYellow
                    }
                    $failures += "$pkg::$example_name (exit code $exit_code)"
                }
            }
            else {
                Write-Host "✗ Example '$example_name' in package '$pkg' timed out after $timeout_seconds seconds" -ForegroundColor Red
                $failures += "$pkg::$example_name (timeout)"
                Stop-Job -Job $job
            }

            Remove-Job -Job $job -Force

        }
        catch {
            Write-Host "✗ Example '$example_name' in package '$pkg' failed with exception: $($_.Exception.Message)" -ForegroundColor Red
            $failures += "$pkg::$example_name (exception: $($_.Exception.Message))"
        }
    }

    # Process subdirectory examples
    foreach ($example_subdir in $example_subdirs) {
        $example_name = $example_subdir.Name

        # Skip excluded examples
        if ($excluded_examples -contains $example_name) {
            Write-Host "Skipping excluded example '$example_name' in package '$pkg'" -ForegroundColor DarkGray
            continue
        }

        $total_count++

        Write-Host "Running example '$example_name' in package '$pkg'..." -ForegroundColor Cyan

        try {
            # Run the example with a timeout to prevent hanging
            $job = Start-Job -ScriptBlock {
                param($pkg, $example_name, $profile)
                $env:IS_TESTING = "1"
                & cargo run --package $pkg --example $example_name --profile $profile --all-features --locked 2>&1
                return $LASTEXITCODE
            } -ArgumentList $pkg, $example_name, $Profile

            $completed = Wait-Job -Job $job -Timeout $timeout_seconds

            if ($completed) {
                $result = Receive-Job -Job $job
                $exit_code = $result[-1]  # Last item should be the exit code
                $output = $result[0..($result.Length - 2)] -join "`n"  # All output except exit code

                if ($exit_code -eq 0) {
                    Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                    $success_count++
                }
                else {
                    Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
                    if ($output.Trim() -ne "") {
                        Write-Host "Output:" -ForegroundColor Yellow
                        Write-Host $output -ForegroundColor DarkYellow
                    }
                    $failures += "$pkg::$example_name (exit code $exit_code)"
                }
            }
            else {
                Write-Host "✗ Example '$example_name' in package '$pkg' timed out after $timeout_seconds seconds" -ForegroundColor Red
                $failures += "$pkg::$example_name (timeout)"
                Stop-Job -Job $job
            }

            Remove-Job -Job $job -Force

        }
        catch {
            Write-Host "✗ Example '$example_name' in package '$pkg' failed with exception: $($_.Exception.Message)" -ForegroundColor Red
            $failures += "$pkg::$example_name (exception: $($_.Exception.Message))"
        }
    }
}

Write-Host ""
Write-Host "Summary:" -ForegroundColor White
Write-Host "  Total examples: $total_count" -ForegroundColor White
Write-Host "  Successful: $success_count" -ForegroundColor Green
Write-Host "  Failed: $($failures.Count)" -ForegroundColor $(if ($failures.Count -eq 0) { "Green" } else { "Red" })

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Failed examples:" -ForegroundColor Red
    foreach ($failure in $failures) {
        Write-Host "  - $failure" -ForegroundColor Red
    }
    exit 1
}
