//! Minimal safe wrappers over `RegGetValueW`/`RegOpenKeyExW`. Read-only,
//! best-effort: every failure is `None` (doctor must report on broken machines).

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD,
    RRF_RT_REG_MULTI_SZ, RRF_RT_REG_QWORD, RRF_RT_REG_SZ,
};

use crate::parse::parse_multi_sz;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn key_exists(subkey: &str) -> bool {
    let sub = wide(subkey);
    let mut hkey = HKEY::default();
    let err = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if err.is_ok() {
        unsafe { _ = RegCloseKey(hkey) };
        true
    } else {
        false
    }
}

/// REG_SZ / REG_EXPAND_SZ (auto-expanded by RegGetValueW) under HKLM.
pub fn read_string(subkey: &str, value: &str) -> Option<String> {
    let raw = read_raw(subkey, value, RRF_RT_REG_SZ.0)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

pub fn read_multi_string(subkey: &str, value: &str) -> Option<Vec<String>> {
    let raw = read_raw(subkey, value, RRF_RT_REG_MULTI_SZ.0)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(parse_multi_sz(&units))
}

pub fn read_u32(subkey: &str, value: &str) -> Option<u32> {
    let raw = read_raw(subkey, value, RRF_RT_REG_DWORD.0)?;
    Some(u32::from_le_bytes(raw.get(..4)?.try_into().ok()?))
}

pub fn read_u64(subkey: &str, value: &str) -> Option<u64> {
    let raw = read_raw(subkey, value, RRF_RT_REG_QWORD.0)?;
    Some(u64::from_le_bytes(raw.get(..8)?.try_into().ok()?))
}

fn read_raw(subkey: &str, value: &str, rrf_type: u32) -> Option<Vec<u8>> {
    use windows::Win32::System::Registry::REG_ROUTINE_FLAGS;
    let sub = wide(subkey);
    let val = wide(value);
    let flags = REG_ROUTINE_FLAGS(rrf_type);
    let mut len: u32 = 0;
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            flags,
            None,
            None,
            Some(&mut len),
        )
    };
    if err.is_err() || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if err.is_err() {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}
