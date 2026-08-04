//! Authenticode summary via `WinVerifyTrust` + the WTHelper signer-chain API.
//! Best-effort by construction: any failure collapses to `SignatureStatus::Unknown`
//! rather than failing the report.

use windows::core::GUID;
use windows::Win32::Foundation::{
    CERT_E_EXPIRED, HANDLE, HWND, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN,
    TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows::Win32::Security::Cryptography::{
    CertGetNameStringW, CERT_CONTEXT, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE,
};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};

use crate::parse::{filetime_to_rfc3339, now_as_filetime_ticks};
use crate::report::{SignatureInfo, SignatureStatus};

pub fn verify(path: &str) -> SignatureInfo {
    verify_impl(path, HANDLE::default())
}

/// Same check, but against an **already-open handle**.
///
/// `WINTRUST_FILE_INFO.hFile` is documented as taking precedence over the path
/// when it is set, so the Authenticode chain is read from the same file object
/// `SealedFile` is holding writers out of — no second `open()`, no window in
/// which the signed bytes could be swapped for other bytes. The path is still
/// passed because the trust provider uses it for catalog lookup and for the
/// text it puts in errors.
pub fn verify_handle(path: &str, raw_handle: isize) -> SignatureInfo {
    verify_impl(path, HANDLE(raw_handle as *mut core::ffi::c_void))
}

fn verify_impl(path: &str, file: HANDLE) -> SignatureInfo {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: windows::core::PCWSTR(wide.as_ptr()),
        hFile: file,
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        // Offline check: doctor must not hang without network.
        dwProvFlags: WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    };
    let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    let trust = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };

    let mut info = SignatureInfo::unknown();
    info.status = match trust as u32 {
        0 => SignatureStatus::Valid,
        c if c == CERT_E_EXPIRED.0 as u32 => SignatureStatus::Expired,
        c if c == TRUST_E_NOSIGNATURE.0 as u32
            || c == TRUST_E_SUBJECT_FORM_UNKNOWN.0 as u32
            || c == TRUST_E_PROVIDER_UNKNOWN.0 as u32 =>
        {
            SignatureStatus::Unsigned
        }
        _ => SignatureStatus::Untrusted,
    };

    // Signer details are available while the verification state is open,
    // even for failed-but-signed files.
    if !data.hWVTStateData.is_invalid() {
        unsafe { extract_signer(data.hWVTStateData, &mut info) };
    }

    // A trust-valid chain over an out-of-validity cert (timestamp
    // countersignature) is the legacy cross-signed state — flag it.
    if info.status == SignatureStatus::Valid && info.cert_expired == Some(true) {
        info.status = SignatureStatus::ValidExpiredCert;
    }

    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    info
}

unsafe fn extract_signer(state: HANDLE, info: &mut SignatureInfo) {
    let prov = WTHelperProvDataFromStateData(state);
    if prov.is_null() {
        return;
    }
    let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
    if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() {
        return;
    }
    let cert = (*(*sgnr).pasCertChain).pCert;
    if cert.is_null() {
        return;
    }
    info.signer = cert_name(cert, 0);
    info.issuer = cert_name(cert, CERT_NAME_ISSUER_FLAG);
    let cert_info = (*cert).pCertInfo;
    if !cert_info.is_null() {
        let na = (*cert_info).NotAfter;
        let ticks = ((na.dwHighDateTime as u64) << 32) | na.dwLowDateTime as u64;
        info.not_after_utc = filetime_to_rfc3339(ticks);
        info.cert_expired = Some(ticks < now_as_filetime_ticks());
    }
}

unsafe fn cert_name(cert: *const CERT_CONTEXT, flags: u32) -> Option<String> {
    let len = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, None);
    if len <= 1 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let written = CertGetNameStringW(
        cert,
        CERT_NAME_SIMPLE_DISPLAY_TYPE,
        flags,
        None,
        Some(&mut buf),
    );
    if written <= 1 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..written as usize - 1]))
}
