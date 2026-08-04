//! File version resources via `GetFileVersionInfoW` / `VerQueryValueW`.

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};

use crate::parse::format_fixed_version;

#[derive(Debug, Default)]
pub struct FileVersionInfo {
    pub fixed_version: Option<String>,
    pub file_version_string: Option<String>,
    pub company: Option<String>,
    pub description: Option<String>,
}

pub fn query(path: &str) -> FileVersionInfo {
    let mut out = FileVersionInfo::default();
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let pcw = PCWSTR(wide.as_ptr());

    let size = unsafe { GetFileVersionInfoSizeW(pcw, None) };
    if size == 0 {
        return out;
    }
    let mut block = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(pcw, None, size, block.as_mut_ptr().cast()) }.is_err() {
        return out;
    }

    if let Some((ptr, len)) = query_value(&block, "\\") {
        if len as usize >= std::mem::size_of::<VS_FIXEDFILEINFO>() {
            let ffi: VS_FIXEDFILEINFO = unsafe { std::ptr::read_unaligned(ptr.cast()) };
            // 0xFEEF04BD guards against a malformed resource block.
            if ffi.dwSignature == 0xFEEF04BD {
                out.fixed_version = Some(format_fixed_version(
                    ffi.dwFileVersionMS,
                    ffi.dwFileVersionLS,
                ));
            }
        }
    }

    if let Some(lang_cp) = first_translation(&block) {
        let get = |field: &str| -> Option<String> {
            let sub = format!("\\StringFileInfo\\{lang_cp}\\{field}");
            let (ptr, len) = query_value(&block, &sub)?;
            if len == 0 {
                return None;
            }
            let units = unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize) };
            let s = String::from_utf16_lossy(
                &units
                    .iter()
                    .copied()
                    .take_while(|&c| c != 0)
                    .collect::<Vec<_>>(),
            );
            (!s.is_empty()).then_some(s)
        };
        out.file_version_string = get("FileVersion");
        out.company = get("CompanyName");
        out.description = get("FileDescription");
    }
    out
}

/// First entry of `\VarFileInfo\Translation` as "llllcccc" hex.
fn first_translation(block: &[u8]) -> Option<String> {
    let (ptr, len) = query_value(block, "\\VarFileInfo\\Translation")?;
    if (len as usize) < 4 {
        return None;
    }
    let pair = unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), 2) };
    Some(format!("{:04x}{:04x}", pair[0], pair[1]))
}

fn query_value(block: &[u8], subblock: &str) -> Option<(*const core::ffi::c_void, u32)> {
    let sub: Vec<u16> = subblock.encode_utf16().chain(std::iter::once(0)).collect();
    let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len: u32 = 0;
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(sub.as_ptr()),
            &mut ptr,
            &mut len,
        )
    };
    (ok.as_bool() && !ptr.is_null()).then_some((ptr.cast_const(), len))
}
