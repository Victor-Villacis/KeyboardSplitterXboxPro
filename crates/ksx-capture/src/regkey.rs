//! Minimal read-only registry handle, shared by [`crate::friendly`] (legacy
//! friendly names) and [`crate::winusb::enumerate`] (which driver a USB
//! interface is bound to).
//!
//! Read-only by construction: `KEY_READ` only, no write entry points exist.
//! That is a safety property, not an omission — nothing in ksx may ever change
//! a device's binding (`docs/RECOVERY.md` §2: the WinUSB rebind is a supervised
//! manual operation with a recovery keyboard on hand).

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, KEY_READ, RRF_RT_REG_SZ,
};

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Owning, read-only registry key handle.
pub(crate) struct RegKey(pub(crate) HKEY);

impl RegKey {
    pub(crate) fn open(parent: HKEY, path: &str) -> Option<Self> {
        let wide = to_wide(path);
        let mut h: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated wide string, out-pointer to HKEY.
        let status = unsafe { RegOpenKeyExW(parent, wide.as_ptr(), 0, KEY_READ, &mut h) };
        (status == ERROR_SUCCESS && !h.is_null()).then_some(Self(h))
    }

    pub(crate) fn subkey_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut index = 0u32;
        loop {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: buffer/length pair as documented; other outputs unused.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                break; // ERROR_NO_MORE_ITEMS or genuine failure — done either way
            }
            names.push(String::from_utf16_lossy(&buf[..len as usize]));
            index += 1;
        }
        names
    }

    pub(crate) fn string_value(&self, name: &str) -> Option<String> {
        let wide = to_wide(name);
        let mut size = 0u32;
        // SAFETY: size query (null data pointer) per RegGetValueW contract.
        let status = unsafe {
            RegGetValueW(
                self.0,
                std::ptr::null(),
                wide.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: buffer sized from the query above; size is in bytes.
        let status = unsafe {
            RegGetValueW(
                self.0,
                std::ptr::null(),
                wide.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let chars = (size as usize) / 2;
        let end = buf[..chars.min(buf.len())]
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(chars.min(buf.len()));
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: handle owned by us, closed exactly once.
        unsafe { RegCloseKey(self.0) };
    }
}
