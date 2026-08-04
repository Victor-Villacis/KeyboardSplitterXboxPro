//! Check-and-use with no gap in between: [`SealedFile`].
//!
//! # The hole this closes
//!
//! `ksx install-drivers` hashes a bundled kernel-driver installer, checks its
//! Authenticode chain, and then — if both pins hold — runs it, elevated. Doing
//! those three things through the *path* means three separate `open()`s, and
//! anyone who can write the file between the second and the third gets their
//! bytes executed with an administrator token. That is a local
//! privilege-escalation chain, and no amount of hashing fixes it: the thing
//! that was hashed and the thing that ran were never proven to be the same
//! bytes.
//!
//! `SealedFile` opens the file **once** and keeps that handle alive across
//! verification *and* execution.
//!
//! # The guarantee, stated precisely
//!
//! While a `SealedFile` is alive on Windows, its handle was opened with
//! `dwShareMode = FILE_SHARE_READ` — i.e. it denies `FILE_SHARE_WRITE` and
//! `FILE_SHARE_DELETE`. The kernel therefore refuses, for as long as the handle
//! lives:
//!
//! - any other open that requests write access (nobody can rewrite the bytes),
//! - any delete or rename of the file (nobody can swap it out from under us).
//!
//! A subsequent `CreateProcess` on the same file still succeeds, because the
//! image loader asks for read+execute sharing, which `FILE_SHARE_READ` permits
//! (`FILE_EXECUTE` is classified as read access by the kernel's share-access
//! check). So: **the bytes hashed are the bytes executed**, with the file
//! locked against modification for the whole window.
//!
//! Two things this does *not* claim:
//!
//! - It does not defend a path whose *parent directory* an attacker controls:
//!   they cannot touch the sealed file, but they could re-point a junction so
//!   a later path lookup resolves elsewhere. [`SealedFile::exec_path`] answers
//!   that by re-deriving the path from the open handle
//!   (`GetFinalPathNameByHandleW`) so execution targets the sealed object, and
//!   `installer::locate` independently refuses candidate directories a standard
//!   user can write while elevated.
//! - Off Windows the share mode has no effect. There is no Authenticode there
//!   either, so verification can never succeed and nothing is ever executed;
//!   the type exists so the pure logic stays testable on a CI runner.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::sha256::Sha256;

/// `FILE_SHARE_READ` — readers welcome, writers and deleters refused.
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;

/// A file held open with writers locked out, so that what was checked is what
/// gets used. See the module docs for the exact guarantee.
pub struct SealedFile {
    file: File,
    requested: PathBuf,
    resolved: PathBuf,
    len: u64,
}

impl std::fmt::Debug for SealedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedFile")
            .field("requested", &self.requested)
            .field("resolved", &self.resolved)
            .field("len", &self.len)
            .finish()
    }
}

impl SealedFile {
    /// Open `path` for reading with writes and deletes denied to everyone else.
    ///
    /// Fails if another process already holds the file open for writing — which
    /// is itself worth refusing on: a driver installer someone is mid-write to
    /// is not a file to run.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = open_shared_read(path)?;
        let len = file.metadata()?.len();
        let resolved = final_path(&file).unwrap_or_else(|| path.to_path_buf());
        Ok(Self {
            file,
            requested: path.to_path_buf(),
            resolved,
            len,
        })
    }

    /// The path as the caller named it.
    pub fn path(&self) -> &Path {
        &self.requested
    }

    /// The path to hand to `CreateProcess`: re-derived from the open handle, so
    /// it names the sealed object rather than whatever the original path string
    /// resolves to now.
    pub fn exec_path(&self) -> &Path {
        &self.resolved
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// SHA-256 of the sealed bytes, read through the sealed handle.
    ///
    /// Cheap enough to call twice (once to build the plan, once immediately
    /// before execution) — and calling it twice is exactly what makes
    /// `installer::execute` self-sufficient rather than trusting its caller.
    pub fn digest(&mut self) -> std::io::Result<[u8; 32]> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = self.file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finish())
    }

    /// Authenticode verdict for the sealed handle.
    ///
    /// `WinVerifyTrust` is given `WINTRUST_FILE_INFO.hFile`, so it inspects the
    /// same open file object rather than re-resolving the path. `None` off
    /// Windows, where there is no Authenticode to ask about.
    pub fn signature(&self) -> Option<crate::report::SignatureInfo> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            Some(crate::win::signature::verify_handle(
                &self.resolved.display().to_string(),
                self.file.as_raw_handle() as isize,
            ))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
fn open_shared_read(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
}

#[cfg(not(windows))]
fn open_shared_read(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// `GetFinalPathNameByHandleW`, stripped back to a plain drive path.
///
/// The `\\?\` prefix is correct but upsets installers that parse their own
/// image path (WiX bundles among them), so a `\\?\C:\…` answer is trimmed to
/// `C:\…`. UNC answers keep their prefix — mangling those would change meaning.
#[cfg(windows)]
fn final_path(file: &File) -> Option<PathBuf> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, GETFINALPATHNAMEBYHANDLE_FLAGS,
        VOLUME_NAME_DOS,
    };

    let handle = HANDLE(file.as_raw_handle());
    let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
    let mut buf = vec![0u16; 1024];
    // SAFETY: `handle` is a live file handle owned by `file`; the buffer is
    // sized by `len()`, which is what the wrapper passes as `cchFilePath`.
    let written = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, flags) };
    if written == 0 || written as usize >= buf.len() {
        return None;
    }
    let text = String::from_utf16_lossy(&buf[..written as usize]);
    Some(PathBuf::from(strip_dos_prefix(&text)))
}

#[cfg(not(windows))]
fn final_path(_file: &File) -> Option<PathBuf> {
    None
}

/// `\\?\C:\x` → `C:\x`; `\\?\UNC\srv\share` and everything else unchanged.
pub fn strip_dos_prefix(path: &str) -> &str {
    match path.strip_prefix(r"\\?\") {
        Some(rest)
            if rest.len() >= 3
                && rest.as_bytes()[1] == b':'
                && (rest.as_bytes()[2] == b'\\' || rest.as_bytes()[2] == b'/')
                && rest.as_bytes()[0].is_ascii_alphabetic() =>
        {
            rest
        }
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ksx-sealed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn digest_matches_the_plain_file_hash_and_is_repeatable() {
        let path = temp_file("digest.bin", b"abc");
        let mut sealed = SealedFile::open(&path).unwrap();
        let once = sealed.digest().unwrap();
        let twice = sealed.digest().unwrap();
        assert_eq!(once, twice, "digest() must rewind, not continue");
        assert_eq!(
            crate::sha256::hex_upper(&once),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
        assert_eq!(sealed.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_large_multi_buffer_file_hashes_correctly() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let path = temp_file("large.bin", &data);
        let mut sealed = SealedFile::open(&path).unwrap();
        let mut oneshot = Sha256::new();
        oneshot.update(&data);
        assert_eq!(sealed.digest().unwrap(), oneshot.finish());
        let _ = std::fs::remove_file(&path);
    }

    /// The whole point: while the seal is held, the bytes cannot be replaced.
    #[cfg(windows)]
    #[test]
    fn a_sealed_file_cannot_be_rewritten_or_deleted() {
        let path = temp_file("sealed.bin", b"original");
        let sealed = SealedFile::open(&path).unwrap();
        assert!(
            std::fs::write(&path, b"tampered").is_err(),
            "a sealed installer must not be rewritable — that is the TOCTOU hole"
        );
        assert!(
            std::fs::remove_file(&path).is_err(),
            "a sealed installer must not be deletable/renamable while it is being used"
        );
        drop(sealed);
        // ...and the lock is released with the handle, not leaked.
        std::fs::write(&path, b"tampered").unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(windows)]
    #[test]
    fn exec_path_resolves_to_the_real_object() {
        let path = temp_file("exec.bin", b"x");
        let sealed = SealedFile::open(&path).unwrap();
        let resolved = sealed.exec_path();
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "the \\\\?\\ prefix confuses installers that parse their own image path: {}",
            resolved.display()
        );
        assert_eq!(
            resolved.file_name(),
            Some(std::ffi::OsStr::new("exec.bin")),
            "{}",
            resolved.display()
        );
        drop(sealed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_missing_file_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("ksx-sealed-does-not-exist-4f2a.bin");
        assert!(SealedFile::open(&missing).is_err());
    }

    #[test]
    fn dos_prefix_stripping_leaves_unc_alone() {
        assert_eq!(strip_dos_prefix(r"\\?\C:\a\b.exe"), r"C:\a\b.exe");
        assert_eq!(
            strip_dos_prefix(r"\\?\UNC\server\share\b.exe"),
            r"\\?\UNC\server\share\b.exe"
        );
        assert_eq!(strip_dos_prefix(r"C:\a\b.exe"), r"C:\a\b.exe");
    }
}
