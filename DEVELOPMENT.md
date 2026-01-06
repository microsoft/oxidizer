# Development environment setup

The standard development environment is a Windows PC, with Windows Subsystem for Linux (WSL) used for Linux development.
After following this guide, you will be able to execute all the local PC development tooling associated with this repository.

> 💡 Development on a Linux or Mac PC is possible, though "at your own risk" - it is not expected that people verify compatibility with "Linux/Mac as primary workstation" scenarios when committing changes to the repo.

The following software must be present on the PC:

* [Latest version of LLVM](https://github.com/llvm/llvm-project/releases)
  * Select: ☑️ Add LLVM to the system PATH for the current user
* [Visual Studio 2022](https://visualstudio.microsoft.com/downloads/) with components:
  * Workload: Desktop development with C++
  * Individual Components: C++ Clang Compiler for Windows
* [Latest version of Visual Studio Code](https://code.visualstudio.com/Download) with extensions:
  * C/C++ (ms-vscode.cpptools)
  * rust-analyzer (rust-lang.rust-analyzer)
  * WSL (ms-vscode-remote.remote-wsl)

> 💡 Using an IDE other than Visual Studio Code is possible, though "at your own risk" - it is not expected that people verify compatibility with other IDEs when committing changes to the repo.

* [Latest version of Git](https://git-scm.com/downloads/win) with components:
  * Git LFS (Large File Storage)
* [Latest version of PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/installing-powershell-on-windows)

For detailed configuration of the above, use your own judgement - the defaults should work but customization is also generally fine.

This guide assumes a clean Windows PC in other regards.

# Windows environment setup

1. Install Rust using [Rustup](https://rustup.rs/), with all default settings.
1. Execute `cargo install just --locked` to install the Just utility (unless already installed).

After installing the Rust toolchain, we setup repository-specific tooling:

1. In a directory of your choosing, clone the `oxidizer` repo: `git clone https://github.com/microsoft/oxidizer.git`.

> 💡 Even though the Windows and Linux development environments are largely independent, they will both use the same working directory (created by the above `git clone` command). This allows you to build and test your changes immediately on both operating systems.

2. Switch to the `oxidizer` directory: `cd oxidizer`.
2. Execute `git config --local include.path ./.gitconfig` to attach the repo-specific Git configuration.
2. Execute `just install-tools` to install all necessary Rust toolchain versions and development tooling.
2. Open `.vscode/settings.template.jsonc` and save a copy as `.vscode/settings.json` to apply repo-specific settings for Visual Studio Code. Part of this file should be the same for everyone but the rest you can customize - refer to inline comments.

## Validate Windows environment

1. Execute `cargo build --all-features --all-targets` to build the workspace. Verify that the build is successful.
1. Execute `cargo test --all-features` to execute all tests in the workspace. Verify that all tests pass.
1. Validate that debugging works by opening `crates/tick/examples/basic.rs` and pressing the `Debug` link that appears above `main()`. This should successfully launch the example app under the debugger.

# Linux (WSL) environment setup

The Linux distribution we use for development is **Ubuntu 24.04**, running as a WSL virtual machine.

1. Install [Ubuntu 24.04.1 LTS](https://apps.microsoft.com/detail/9NZ3KLHXDJP5?hl=en-us&gl=US&ocid=pdpshare) from the Microsoft Store.
1. Open an Ubuntu terminal (e.g. from the Microsoft Store page, from the Start menu or in Windows Terminal).
    * You will be asked to create a user account the first time you run Ubuntu. This is a local account unique to the Linux VM and is not related to your account on the host machine.

All commands that follow are to be executed in the Ubuntu terminal.

Next, we upgrade, install and configure development prerequisites:

1. Execute `sudo apt update && sudo apt dist-upgrade -y` to upgrade everything that is already installed.
1. Execute `sudo apt install -y curl clang llvm libclang-dev gdb perl python3 python3-pip git git-lfs build-essential cmake pkg-config libssl-dev` to ensure that essential packages are installed.
1. [Install PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/install-ubuntu?view=powershell-7.5#installation-via-package-repository-the-package-repository).
1. Execute `git config --global credential.helper "/mnt/c/Program\ Files/Git/mingw64/bin/git-credential-manager.exe"` to set the correct Git authentication flow.
1. Execute `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` and install Rust with all default settings.
1. Reopen the terminal to apply changes.
1. Execute `cargo install just --locked` to install the Just utility (unless already installed).

Next, we setup repository-specific tooling on Linux:

1. Switch to the `oxidizer` directory you previously cloned on Windows, using a `/mnt/c` style path to access the Windows filesystem: `cd /mnt/c/Users/username/Desktop/oxidizer` (adjusting the path to match your chosen location).
1. Execute `just install-tools` to install all necessary Rust toolchain versions and development tooling.

## Optimize Linux build performance

> ⚠️ This chapter may conflict with other repos in the same WSL instance. Skip or undo if you experience problems.

After installing the Rust toolchains, we setup the build target directory for fast build times:

1. Execute `mkdir ~/target` to create a directory for Linux build outputs. While the repo directory itself is shared between Windows and Linux, we will use a dedicated directory for build outputs on Linux to improve Linux build performance.
1. Execute `nano ~/.bashrc` to open a text editor on this file.
1. Add `export CARGO_TARGET_DIR=~/target` near the end of the file.
1. Save & exit.
1. Reopen the terminal to apply changes.

## Validate Linux (WSL) environment

1. Execute `cargo build --all-features --all-targets` to build the workspace. Verify that the build is successful.
1. Execute `cargo test --all-features` to execute all tests in the workspace. Verify that all tests pass.

## Setup Visual Studio Code integration

Visual Studio Code is also our Linux IDE and requires some additional setup to work with the Linux workspace.

> ⚠️ While the IDE runs on the Windows desktop, all tooling runs in the Linux VM, including Visual Studio Code extensions. The below steps will instruct you to install the minimum set of required Visual Studio Code extensions for the Linux environment.

1. In an Ubuntu terminal, in the `oxidizer` directory, execute `code .` to open the project in Visual Studio Code.
1. Open the Extensions panel in Visual Studio Code.
1. Install following extensions by selecting "Install in WSL" for each:
    * C/C++ (ms-vscode.cpptools)
    * rust-analyzer (rust-lang.rust-analyzer)
1. Close Visual Studio Code and open it again via `code .` to complete extension setup.

Validate the setup by executing the following tasks from the task palette (F1):

1. `Tasks: Run Build Task`
1. `Tasks: Run Test Task`

Validate that debugging works by opening `crates/tick/examples/basic.rs` and pressing the `Debug` link that appears above `main()`. This should successfully launch the example app under the debugger.
