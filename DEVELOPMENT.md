# Development environment setup

The standard development environment is a Windows PC, with Windows Subsystem for Linux (WSL) used for Linux development.
After following this guide, you will be able to execute all the local PC development tooling associated with this repository.

> 💡 Development on a Linux or Mac system is also possible, though the setup process is not documented here.

To run all repository tooling locally, you will need the following prerequisites:

* [Latest version of LLVM](https://github.com/llvm/llvm-project/releases)
* [Visual Studio Build Tools for C++](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with components:
  * Workload: Desktop development with C++
* [Latest version of Git](https://git-scm.com/downloads/win) with components:
  * Git LFS (Large File Storage)
* [Latest version of PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/installing-powershell-on-windows)

For detailed configuration of the above, use your own judgement - the defaults should work but customization is also generally fine.

The following directories must be in PATH:

* `c:\Program Files\LLVM\bin`

This guide assumes a clean Windows PC in other regards.

# Windows environment setup

1. If you had to install any of the above prerequisites, restart your PC to ensure a complete installation.
1. Install Rust using [Rustup](https://rustup.rs/), with all default settings.
1. Execute `cargo install just --locked` to install the Just utility (unless already installed).

After installing the Rust toolchain, we setup repository-specific tooling:

1. In a directory of your choosing, clone the `oxidizer` repo: `git clone https://github.com/microsoft/oxidizer.git`.

> 💡 Even though the Windows and Linux development environments are largely independent, they will both use the same working directory (created by the above `git clone` command). This allows you to build and test your changes immediately on both operating systems.

2. Switch to the `oxidizer` directory: `cd oxidizer`.
2. Execute `just anvil-setup` to install all necessary Rust toolchain versions and development tooling.
2. Execute `Install-Module -Name Pester -RequiredVersion 5.7.1 -Force -Scope CurrentUser -SkipPublisherCheck` to install the release-script test dependency.

## Validate Windows environment

1. Execute `just anvil-build` to build the workspace. Verify that the build is successful.
1. Execute `just anvil-pr-test` to execute the test and coverage group. Verify that it succeeds.
1. Execute `just test-scripts` to run the Pester suite for the release-related PowerShell scripts. Verify that all tests pass.

# Linux (WSL) environment setup

The Linux distribution we use for development is **Ubuntu**, running as a WSL virtual machine.

1. Install [Ubuntu](https://apps.microsoft.com/detail/9PDXGNCFSCZV?hl=en-us&gl=US&ocid=pdpshare) from the Microsoft Store.
1. Open an Ubuntu terminal (e.g. from the Microsoft Store page, from the Start menu or in Windows Terminal).
    * If you get an error about a required feature not being installed, try first restarting the PC to complete the feature installation.
1. You will be asked to create a user account the first time you run Ubuntu. This is a local account unique to the Linux VM and is not related to your account on the host machine.

All commands that follow are to be executed in the Ubuntu terminal.

Next, we upgrade, install and configure development prerequisites:

1. Execute `sudo apt update && sudo apt dist-upgrade -y` to upgrade everything that is already installed.
1. Execute `sudo apt install -y curl clang llvm libclang-dev gdb perl python3 python3-pip git git-lfs build-essential cmake pkg-config libssl-dev valgrind` to ensure that essential packages are installed.
1. [Install PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/install-ubuntu?view=powershell-7.5#installation-via-package-repository-the-package-repository).
1. Execute `git config --global credential.helper "/mnt/c/Program\ Files/Git/ucrt64/bin/git-credential-manager.exe"` to set the correct Git authentication flow.
1. Execute `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` and install Rust with all default settings.
1. Reopen the terminal to apply changes.
1. Execute `cargo install just --locked` to install the Just utility (unless already installed).

Next, we setup repository-specific tooling on Linux:

1. Switch to the `oxidizer` directory you previously cloned on Windows, using a `/mnt/c` style path to access the Windows filesystem: `cd /mnt/c/Users/username/Desktop/oxidizer` (adjusting the path to match your chosen location).
1. Execute `just anvil-setup` to install all necessary Rust toolchain versions and development tooling.
1. Execute `Install-Module -Name Pester -RequiredVersion 5.7.1 -Force -Scope CurrentUser -SkipPublisherCheck` to install the release-script test dependency.

## Optimize Linux build performance

> ⚠️ This chapter may conflict with other repos in the same WSL instance. Skip or undo if you experience problems.

After installing the Rust toolchains, we setup the build target directory for fast build times:

1. Execute `mkdir ~/target` to create a directory for Linux build outputs. While the repo directory itself is shared between Windows and Linux, we will use a dedicated directory for build outputs on Linux to improve Linux build performance.
1. Execute `nano ~/.bashrc` to open a text editor on this file.
1. Add `export CARGO_TARGET_DIR=~/target` near the end of the file.
1. Save & exit.
1. Reopen the terminal to apply changes.

## Validate Linux (WSL) environment

1. Execute `just anvil-build` to build the workspace. Verify that the build is successful.
1. Execute `just anvil-pr-test` to execute the test and coverage group. Verify that it succeeds.
1. Execute `just test-scripts` to run the Pester suite for the release-related PowerShell scripts. Verify that all tests pass.

# Before submitting a pull request

Run all essential CI checks locally with a single command:

```sh
just anvil-pr
```

This runs the same generated validation tier used by the Anvil pull request
workflow. Use `just anvil-pr-fast` when partial, faster feedback is sufficient.

PR titles must follow [Conventional Commits](https://www.conventionalcommits.org/) format (e.g., `feat: add validation method`, `fix: resolve memory leak`).