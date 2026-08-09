//! Stamp the customer launcher's Win32 icon and version information.
//!
//! The installer shortcuts point at `ksx-launcher.exe`, so relying on
//! `ksx.exe`'s resources would replace the product icon with Windows' generic
//! executable icon. This mirrors `ksx-app/build.rs`; the launcher deliberately
//! has no runtime dependency on the CLI crate.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/dist/ksx.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    #[cfg(windows)]
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon_with_id("../../assets/brand/dist/ksx.ico", "1");

    let version = env!("CARGO_PKG_VERSION");
    resources
        .set("ProductName", "ksx")
        .set("FileDescription", "ksx launcher")
        .set("OriginalFilename", "ksx-launcher.exe")
        .set("InternalName", "ksx-launcher")
        .set("CompanyName", "Victor Villacis")
        .set("LegalCopyright", "MIT OR Apache-2.0")
        .set("ProductVersion", version)
        .set("FileVersion", version);

    // Missing Windows SDK resource tooling must not make the program itself
    // unbuildable. The executable still launches correctly; only its cosmetic
    // resources are absent, matching ksx-app's existing policy.
    if let Err(error) = resources.compile() {
        println!(
            "cargo:warning=ksx-launcher.exe resources not stamped ({error}); \
             the icon and version tab will be missing"
        );
    }
}
