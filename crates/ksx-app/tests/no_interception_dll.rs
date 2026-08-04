//! `ksx.exe` must start on a machine that has never had `interception.dll`.
//!
//! # The failure this pins
//!
//! `interception-sys` links an **import library**, so every `interception_*`
//! call ksx made became a static entry in the PE import table — resolved by the
//! Windows loader *before `main`*. Without the DLL beside the exe the process
//! never started at all: a modal "The code execution cannot proceed because
//! interception.dll was not found" for `ksx --version`, `ksx devices`,
//! `ksx doctor`, everything. On a cabinet migrated to the WinUSB backend — M6's
//! whole purpose, whose exit criterion is *the Interception driver
//! uninstalled* — that made ksx unstartable.
//!
//! # Why two assertions and not one
//!
//! Running the exe from a directory without the DLL is the user-visible
//! behaviour, but it is only conclusive if the DLL is also absent from
//! `System32` and the `PATH` — which is true on CI and on the cabinet, and
//! false on a developer box that installed Interception. So the load-bearing
//! assertion is the structural one: parse the binary's import directory and
//! prove `interception.dll` is not in it. That answers the question on every
//! machine, and it fails loudly the moment somebody re-introduces a direct
//! `raw::interception_*` call.

#![cfg(windows)]

use std::path::{Path, PathBuf};

/// The binary under test, built by cargo for this integration test.
const KSX: &str = env!("CARGO_BIN_EXE_ksx");

// ---------------------------------------------------------------------------
// A minimal PE import-table reader
// ---------------------------------------------------------------------------

/// Names of the DLLs `exe` statically imports (the load-time import directory).
///
/// Deliberately hand-rolled: pulling a PE-parsing crate into the dependency
/// graph of a keyboard driver front-end to read four offsets is a worse trade
/// than 60 lines of documented arithmetic.
fn static_imports(exe: &Path) -> std::io::Result<Vec<String>> {
    let bytes = std::fs::read(exe)?;
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at =
        |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);

    assert_eq!(&bytes[0..2], b"MZ", "not a PE image: {}", exe.display());
    let pe = u32_at(0x3C) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0", "bad PE signature");

    let coff = pe + 4;
    let n_sections = u16_at(coff + 2) as usize;
    let opt_size = u16_at(coff + 16) as usize;
    let opt = coff + 20;
    // 0x10B = PE32, 0x20B = PE32+. The data-directory array starts 96 bytes into
    // the optional header for PE32 and 112 for PE32+.
    let dirs = match u16_at(opt) {
        0x10B => opt + 96,
        0x20B => opt + 112,
        other => panic!("unknown optional header magic {other:#x}"),
    };
    // Data directory index 1 = the import table.
    let import_rva = u32_at(dirs + 8) as usize;
    let import_size = u32_at(dirs + 12) as usize;
    if import_rva == 0 || import_size == 0 {
        return Ok(Vec::new()); // no static imports at all
    }

    // Section table follows the optional header; each entry is 40 bytes with the
    // virtual address at +12 and the raw file offset at +20.
    let sections = opt + opt_size;
    let to_offset = |rva: usize| -> Option<usize> {
        (0..n_sections).find_map(|i| {
            let s = sections + i * 40;
            let va = u32_at(s + 12) as usize;
            let vsize = u32_at(s + 8) as usize;
            let raw_size = u32_at(s + 16) as usize;
            let raw = u32_at(s + 20) as usize;
            let span = vsize.max(raw_size);
            (rva >= va && rva < va + span).then(|| raw + (rva - va))
        })
    };

    let mut names = Vec::new();
    let table = to_offset(import_rva).expect("import table lies inside a section");
    // IMAGE_IMPORT_DESCRIPTOR is 20 bytes; the array ends at an all-zero entry.
    // The DLL name RVA sits at +12.
    for i in 0.. {
        let d = table + i * 20;
        if d + 20 > bytes.len() || bytes[d..d + 20].iter().all(|b| *b == 0) {
            break;
        }
        let name_rva = u32_at(d + 12) as usize;
        let Some(at) = to_offset(name_rva) else {
            continue;
        };
        let end = bytes[at..]
            .iter()
            .position(|b| *b == 0)
            .map_or(bytes.len(), |n| at + n);
        names.push(String::from_utf8_lossy(&bytes[at..end]).to_ascii_lowercase());
    }
    Ok(names)
}

/// **The structural assertion.** No import descriptor for the DLL means the
/// loader has nothing to resolve, on any machine, whatever is in System32.
#[test]
fn the_binary_does_not_statically_import_interception_dll() {
    let imports = static_imports(Path::new(KSX)).expect("reading the ksx binary");
    assert!(
        !imports.is_empty(),
        "the import parser found nothing at all — it is broken, not the binary"
    );
    assert!(
        imports.iter().any(|d| d.starts_with("kernel32")),
        "sanity: every Windows exe imports kernel32; got {imports:?}"
    );
    assert!(
        !imports.iter().any(|d| d == "interception.dll"),
        "ksx.exe statically imports interception.dll, so the Windows loader requires the \
         file to exist before main() runs — `ksx --version` dies with a loader dialog on \
         any machine without it. Route the call through \
         `ksx_capture::interception_dll::api()` instead of `raw::interception_*`. \
         imports: {imports:?}"
    );
}

/// **The user-visible assertion.** Copy just the exe somewhere with no DLL
/// beside it and run the two commands a fresh machine runs first.
#[test]
fn version_and_devices_run_from_a_directory_with_no_interception_dll() {
    let dir = std::env::temp_dir().join(format!("ksx-no-dll-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let exe: PathBuf = dir.join("ksx.exe");
    std::fs::copy(KSX, &exe).expect("copying ksx.exe");
    assert!(
        !dir.join("interception.dll").exists(),
        "the point of this test is that the DLL is NOT here"
    );

    for args in [vec!["--version"], vec!["devices", "--json"]] {
        let out = std::process::Command::new(&exe)
            .args(&args)
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|e| panic!("running ksx {args:?}: {e}"));
        let stderr = String::from_utf8_lossy(&out.stderr);
        // STATUS_DLL_NOT_FOUND. The loader kills the process before main, so
        // there is no Rust-level message to look for — only this exit status.
        assert_ne!(
            out.status.code(),
            Some(0xC0000135_u32 as i32),
            "ksx {args:?} died in the Windows loader (STATUS_DLL_NOT_FOUND): {stderr}"
        );
        // `devices` exits 2 when nothing at all could be enumerated, which is a
        // legitimate answer on a machine with no I-PAC plugged in.
        assert!(
            matches!(out.status.code(), Some(0) | Some(2)),
            "ksx {args:?} exited {:?}\nstderr: {stderr}",
            out.status.code()
        );
        assert!(
            !stderr.contains("code execution cannot proceed"),
            "ksx {args:?} printed the loader's missing-DLL text: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
