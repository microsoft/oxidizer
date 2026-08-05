// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Locates the Spectre-mitigated MSVC libraries and adds them to the linker search path.

#[cfg(all(target_os = "windows", target_env = "msvc"))]
#[path = "src/architecture.rs"]
mod architecture;

fn main() {
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    add_spectre_link_search();
}

/// Windows requires additional steps to find the Spectre-mitigated CRT libraries.
///
/// See <https://learn.microsoft.com/cpp/build/reference/qspectre>.
#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn add_spectre_link_search() {
    use std::env;

    use cc::windows_registry;

    let target = env::var("TARGET").expect("Cargo must set TARGET when executing a package build script");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must set CARGO_CFG_TARGET_ARCH when executing a package build script");
    let arch = architecture::spectre_directory(&arch)
        .expect("Windows MSVC targets must use an architecture supported by Visual Studio Spectre libraries");

    let tool =
        windows_registry::find_tool(&target, "cl.exe").expect("the Windows MSVC target requires cl.exe from a Visual Studio installation");
    let spectre_libs = tool.path().join(format!(r"..\..\..\..\lib\spectre\{arch}"));

    if spectre_libs.exists() {
        println!("cargo:rustc-link-search=native={}", spectre_libs.display());
    } else {
        println!(
            "cargo:warning=No Spectre-mitigated libraries were found. \
             Use Visual Studio Installer to add them."
        );

        #[cfg(feature = "error")]
        assert!(
            env::var_os("MSVC_SPECTRE_LIBS_ALLOW_MISSING").is_some(),
            "No Spectre-mitigated libraries were found. \
             Use Visual Studio Installer to add them."
        );
    }
}
