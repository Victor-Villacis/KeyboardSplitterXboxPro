//! Stamp the Win32 resources into `ksx.exe`: the icon group and VERSIONINFO.
//!
//! Until this file existed, `ksx.exe` carried **no resources at all** — the
//! generic white-page icon in Explorer, in the taskbar, in the Open-With list
//! and in the UAC prompt, and a blank Details tab in its properties. The tray
//! could not do better than `LoadIconW(NULL, IDI_APPLICATION)` because there
//! was nothing in the binary to load.
//!
//! # The icon
//!
//! `assets/brand/dist/ksx.ico` — eight entries, and **not eight scalings of
//! one drawing**: 16/20/24/32 are the simplified mark, 48/64/128/256 the
//! detailed one (`tools/icongen`, `assets/brand/README.md`). Windows picks
//! the entry that fits the surface it is drawing, so the tray gets art drawn
//! for 16 px and the alt-tab switcher gets art drawn for 256 px.
//!
//! The group is stamped with resource id [`ICON_RESOURCE_ID`] = 1, which is
//! two contracts at once:
//!
//! - Explorer shows the icon group with the **numerically lowest** id, so 1
//!   is what makes this the file's face;
//! - `ksx_backend::daemon::tray` loads that same id out of its own module
//!   (`ksx-backend/src/daemon/tray.rs`, `ICON_RESOURCE_ID`). The two are the
//!   handshake; they are documented in terms of each other on both sides.
//!
//! # VERSIONINFO
//!
//! Version metadata is not decoration on this project. `ksx install-drivers`
//! reasons about Authenticode signatures and driver versions, `ksx doctor`
//! reports what is installed, and a support conversation that starts with
//! "right-click → Properties → Details" needs that tab to say something. The
//! version comes from `CARGO_PKG_VERSION` so it cannot drift from the crate.
//!
//! # When this actually runs
//!
//! Two conditions, checked separately because they are two different things:
//!
//! - the **target** must be Windows (`CARGO_CFG_TARGET_OS`) — a `.res` means
//!   nothing in an ELF binary, and ksx's lower crates do get
//!   `cargo check --target` runs;
//! - the **host** must be Windows (`cfg!(windows)` — a build script always
//!   runs natively). `winresource` is declared under
//!   `[target.'cfg(windows)'.build-dependencies]`, and Cargo resolves *that*
//!   table against the host, so the crate is not even linked into this script
//!   when it is built on Linux.
//!
//! So a Windows→Windows build gets the icon; a Linux→Windows cross-compile
//! produces a working, iconless `ksx.exe`. That is the same trade as the
//! `res.compile()` failure below, for the same reason: nothing about the
//! program's behaviour depends on the icon being there.

fn main() {
    // Committed artifacts, so this only fires when the brand is regenerated.
    println!("cargo:rerun-if-changed=../../assets/brand/dist/ksx.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // The TARGET's OS, not the host's: a build script always runs natively.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    #[cfg(windows)]
    windows_resources();
}

/// See [`ICON_RESOURCE_ID`] in `src/daemon/tray.rs` — the other half.
#[cfg(windows)]
const ICON_RESOURCE_ID: &str = "1";

#[cfg(windows)]
fn windows_resources() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon_with_id("../../assets/brand/dist/ksx.ico", ICON_RESOURCE_ID);

    let version = env!("CARGO_PKG_VERSION");
    res.set("ProductName", "ksx")
        .set(
            "FileDescription",
            "ksx — keyboard splitter for arcade cabinets",
        )
        .set("OriginalFilename", "ksx.exe")
        .set("InternalName", "ksx")
        .set("CompanyName", "Victor Villacis")
        .set("LegalCopyright", "MIT OR Apache-2.0")
        .set("ProductVersion", version)
        .set("FileVersion", version);

    // A resource-compiler failure must not be fatal. `rc.exe` lives in the
    // Windows SDK, and a machine with the MSVC toolchain but no SDK
    // component — or a cross-compile from a Linux CI runner — can still build
    // a perfectly working ksx; it just gets the old blank-icon binary. Making
    // that a hard error would trade a cosmetic loss for an unbuildable
    // project, so it is a warning the build log carries instead.
    if let Err(e) = res.compile() {
        println!("cargo:warning=ksx.exe resources not stamped ({e}); the icon and version tab will be missing");
    }
}
