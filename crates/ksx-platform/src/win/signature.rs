//! Authenticode summary via `WinVerifyTrust` + the WTHelper signer-chain API.
//! Best-effort by construction: any failure collapses to `SignatureStatus::Unknown`
//! rather than failing the report.
//!
//! # Why everything is read out of the WinVerifyTrust state
//!
//! The timestamp countersignature could also be reached with `CryptQueryObject`
//! plus `CryptMsgGetParam`, decoding the unauthenticated attributes by hand. It
//! is not, for two reasons:
//!
//! - `CryptQueryObject` takes a **path or a memory blob**, never a file handle.
//!   Using it would mean a second `open()` of the installer, which is precisely
//!   the time-of-check/time-of-use gap [`crate::sealed`] exists to close — the
//!   timestamp would then have been read from a file that is not provably the
//!   one that gets executed.
//! - Hand-decoding the countersignature would also mean hand-building its
//!   certificate chain to decide whether the timestamping authority is trusted.
//!   `WinVerifyTrust` has already done that, on the sealed handle, and exposes
//!   the answer through `CRYPT_PROVIDER_SGNR::pasCounterSigners`.
//!
//! So the whole verdict — hash aside — comes from one `WinVerifyTrust` call
//! against `WINTRUST_FILE_INFO.hFile`. One open, one check, no gap.

use windows::core::GUID;
use windows::Win32::Foundation::{
    CERT_E_EXPIRED, FILETIME, HANDLE, HWND, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN,
    TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows::Win32::Security::Cryptography::{
    CertGetNameStringW, CERT_CONTEXT, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE,
};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
    CRYPT_PROVIDER_DATA, CRYPT_PROVIDER_SGNR, SGNR_TYPE_TIMESTAMP,
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};

use crate::parse::{filetime_to_rfc3339, now_as_filetime_ticks};
use crate::report::{SignatureInfo, SignatureStatus, TimestampInfo};

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
    // The signing certificate's window. Both ends matter once timestamps are in
    // play: "was this file signed while the certificate was alive" is a question
    // about an interval, not about an expiry date.
    let (not_before, not_after) = {
        let cert_info = (*cert).pCertInfo;
        if cert_info.is_null() {
            (0, 0)
        } else {
            (ticks((*cert_info).NotBefore), ticks((*cert_info).NotAfter))
        }
    };
    if not_after != 0 {
        info.not_before_utc = filetime_to_rfc3339(not_before);
        info.not_after_utc = filetime_to_rfc3339(not_after);
        info.cert_expired = Some(not_after < now_as_filetime_ticks());
    }
    info.timestamp = extract_timestamp(prov, sgnr, not_before, not_after);
}

/// The countersignature that dates the signature, checked against the signing
/// certificate's window `[not_before, not_after]`.
///
/// A signature may carry several countersigners. The first one that survives
/// every check wins; if none does, the first one found is returned anyway so
/// the refusal can say *what* was wrong with it rather than "absent".
unsafe fn extract_timestamp(
    prov: *mut CRYPT_PROVIDER_DATA,
    sgnr: *const CRYPT_PROVIDER_SGNR,
    not_before: u64,
    not_after: u64,
) -> Option<TimestampInfo> {
    let mut first: Option<TimestampInfo> = None;
    for index in 0..(*sgnr).csCounterSigners {
        let counter = WTHelperGetProvSignerFromChain(prov, 0, true, index);
        if counter.is_null() {
            continue;
        }
        let candidate = read_timestamp(counter, not_before, not_after);
        if candidate.is_verified() {
            return Some(candidate);
        }
        if first.is_none() {
            first = Some(candidate);
        }
    }
    first
}

/// One countersigner → the four facts [`TimestampInfo`] needs. Nothing here
/// decides anything; the verdict is [`TimestampInfo::problem`]'s job.
unsafe fn read_timestamp(
    counter: *const CRYPT_PROVIDER_SGNR,
    not_before: u64,
    not_after: u64,
) -> TimestampInfo {
    let signed_at = ticks((*counter).sftVerifyAsOf);
    // `dwError == 0` is wintrust's own verdict on this countersignature: it
    // built and validated the timestamping authority's chain while verifying
    // the file, so this is the trusted-TSA answer rather than our guess at one.
    let chain_ok =
        (*counter).dwError == 0 && (*counter).csCertChain > 0 && !(*counter).pasCertChain.is_null();

    let mut authority = None;
    let mut within_timestamp_cert_validity = false;
    if chain_ok {
        let cert = (*(*counter).pasCertChain).pCert;
        if !cert.is_null() {
            authority = cert_name(cert, 0);
            let cert_info = (*cert).pCertInfo;
            if !cert_info.is_null() {
                let (tsa_from, tsa_to) =
                    (ticks((*cert_info).NotBefore), ticks((*cert_info).NotAfter));
                within_timestamp_cert_validity =
                    tsa_to != 0 && signed_at >= tsa_from && signed_at <= tsa_to;
            }
        }
    }

    TimestampInfo {
        signed_at_utc: filetime_to_rfc3339(signed_at),
        signed_at_ticks: signed_at,
        authority,
        is_timestamp_signer: (*counter).dwSignerType & SGNR_TYPE_TIMESTAMP != 0,
        chain_ok,
        // An unreadable certificate window (0, 0) fails this, which is the
        // point: not knowing the window is not the same as being inside it.
        within_signing_cert_validity: not_after != 0
            && signed_at >= not_before
            && signed_at <= not_after,
        within_timestamp_cert_validity,
    }
}

fn ticks(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
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
