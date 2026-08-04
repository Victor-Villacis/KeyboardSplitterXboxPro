//! Pure parsers for data read off the machine — no syscalls, unit-tested with
//! fixtures so verdict/report logic stays testable off-cabinet.

/// FILETIME ticks (100 ns since 1601-01-01) between the Windows and Unix epochs.
const EPOCH_DIFF_TICKS: u64 = 116_444_736_000_000_000;
const TICKS_PER_SEC: u64 = 10_000_000;

/// Parse a REG_MULTI_SZ buffer (UTF-16, entries NUL-separated, list
/// double-NUL-terminated; tolerate missing terminators).
pub fn parse_multi_sz(raw: &[u16]) -> Vec<String> {
    raw.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Resolve a kernel service ImagePath to a Win32 path.
/// `\SystemRoot\System32\drivers\X.sys` and bare `System32\drivers\X.sys` both
/// occur in the wild; anything already absolute passes through.
pub fn resolve_image_path(image_path: &str, windir: &str) -> String {
    let windir = windir.trim_end_matches('\\');
    let lower = image_path.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("\\systemroot\\")
        .map(|_| &image_path["\\systemroot\\".len()..])
    {
        return format!("{windir}\\{rest}");
    }
    if lower.starts_with("system32\\") {
        return format!("{windir}\\{image_path}");
    }
    if let Some(rest) = lower
        .strip_prefix("\\??\\")
        .map(|_| &image_path["\\??\\".len()..])
    {
        return rest.to_string();
    }
    image_path.to_string()
}

/// `VS_FIXEDFILEINFO` version pair → "a.b.c.d".
pub fn format_fixed_version(ms: u32, ls: u32) -> String {
    format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff)
}

/// FILETIME ticks → RFC 3339 UTC (second precision). Returns `None` for values
/// before the Unix epoch (never legitimate for cert/policy timestamps).
pub fn filetime_to_rfc3339(ticks: u64) -> Option<String> {
    let unix_secs = ticks.checked_sub(EPOCH_DIFF_TICKS)? / TICKS_PER_SEC;
    let days = unix_secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = unix_secs % 86_400;
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    ))
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Current time as FILETIME ticks, for cert-expiry comparison.
pub fn now_as_filetime_ticks() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => EPOCH_DIFF_TICKS + d.as_secs() * TICKS_PER_SEC,
        Err(_) => EPOCH_DIFF_TICKS,
    }
}

/// Metadata scraped out of a binary CI policy (`.cip`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CipMeta {
    pub name: Option<String>,
    pub id: Option<String>,
}

/// Best-effort extraction of `PolicyInfo/Information/{Id,Name}` from a `.cip`.
///
/// The file is a PKCS#7 SignedData envelope around the binary policy; rather
/// than unwrap ASN.1 we scan for printable UTF-16LE runs. Secure Settings are
/// stored as length-prefixed UTF-16 tuples (Provider, Key, ValueName, Value),
/// so the runs appear in exactly that order.
pub fn parse_cip_meta(bytes: &[u8]) -> CipMeta {
    let runs = utf16le_printable_runs(bytes, 2);
    let mut meta = CipMeta::default();
    for w in runs.windows(4) {
        if w[0] == "PolicyInfo" && w[1] == "Information" {
            match w[2].as_str() {
                "Id" if meta.id.is_none() => meta.id = Some(w[3].clone()),
                "Name" if meta.name.is_none() => meta.name = Some(w[3].clone()),
                _ => {}
            }
        }
    }
    meta
}

/// All runs of >= `min_len` printable-ASCII UTF-16LE code units, scanned at
/// even offsets (the policy strings are 4-byte aligned in practice).
fn utf16le_printable_runs(bytes: &[u8], min_len: usize) -> Vec<String> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    for chunk in bytes.chunks_exact(2) {
        let c = u16::from_le_bytes([chunk[0], chunk[1]]);
        if (0x20..0x7f).contains(&c) {
            cur.push(c as u8 as char);
        } else {
            if cur.len() >= min_len {
                runs.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= min_len {
        runs.push(cur);
    }
    runs
}

/// Audit-vs-enforce heuristic: Microsoft names the rollout policies explicitly
/// ("… Audit Policy" / "… Enforced Policy"); the flags themselves are inside
/// the signed binary and need elevation (CiTool) to read authoritatively.
pub fn ci_mode_from_name(name: Option<&str>) -> crate::report::CiPolicyMode {
    use crate::report::CiPolicyMode;
    match name {
        Some(n) if n.to_ascii_lowercase().contains("audit") => CiPolicyMode::Audit,
        Some(n) if n.to_ascii_lowercase().contains("enforce") => CiPolicyMode::Enforce,
        _ => CiPolicyMode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::CiPolicyMode;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn multi_sz_standard() {
        let mut raw = wide("keyboard");
        raw.push(0);
        raw.extend(wide("kbdclass"));
        raw.extend([0, 0]);
        assert_eq!(parse_multi_sz(&raw), vec!["keyboard", "kbdclass"]);
    }

    #[test]
    fn multi_sz_empty_and_unterminated() {
        assert!(parse_multi_sz(&[]).is_empty());
        assert!(parse_multi_sz(&[0, 0]).is_empty());
        assert_eq!(parse_multi_sz(&wide("solo")), vec!["solo"]);
    }

    #[test]
    fn image_path_forms() {
        let w = "C:\\Windows";
        assert_eq!(
            resolve_image_path("\\SystemRoot\\System32\\drivers\\ViGEmBus.sys", w),
            "C:\\Windows\\System32\\drivers\\ViGEmBus.sys"
        );
        assert_eq!(
            resolve_image_path("System32\\drivers\\ScpVBus.sys", w),
            "C:\\Windows\\System32\\drivers\\ScpVBus.sys"
        );
        assert_eq!(
            resolve_image_path("\\??\\C:\\foo\\bar.sys", w),
            "C:\\foo\\bar.sys"
        );
        assert_eq!(
            resolve_image_path("C:\\abs\\x.sys", "C:\\Windows\\"),
            "C:\\abs\\x.sys"
        );
    }

    #[test]
    fn fixed_version_formats() {
        // ViGEmBus.sys live value: 1.21.442.0
        assert_eq!(format_fixed_version(0x0001_0015, 0x01BA_0000), "1.21.442.0");
        assert_eq!(format_fixed_version(0, 0), "0.0.0.0");
    }

    #[test]
    fn filetime_known_values() {
        assert_eq!(
            filetime_to_rfc3339(116_444_736_000_000_000).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        // Live WhqlOnlyEvaluation StatusEventTimestamp observed 2026-08-03 (local).
        assert_eq!(
            filetime_to_rfc3339(134_302_774_184_006_535).as_deref(),
            Some("2026-08-04T00:36:58Z")
        );
        // Interception cert NotAfter: 2012-10-21 (the whole saga in one number).
        assert_eq!(
            filetime_to_rfc3339(129_953_078_450_000_000).as_deref(),
            Some("2012-10-21T15:44:05Z")
        );
        assert_eq!(filetime_to_rfc3339(0), None);
    }

    /// Real bytes from this machine's
    /// `{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}.cip` settings section
    /// (System32\CodeIntegrity\CiPolicies\Active, offset 111172, 2026-08-03).
    const CIP_FIXTURE_HEX: &str = "0000000000000000000000000000000000000000000000000000000003000000120000004D006900630072006F0073006F00660074000000000000001C00000050006F006C00690063007900530065007400740069006E006700730000000000360000005300690067006E006500640050006F006C00690063007900530065007300730069006F006E004F006E006C0079004C006F0063006B0000000000000000000000010000001400000050006F006C0069006300790049006E0066006F00000000001600000049006E0066006F0072006D006100740069006F006E000000000000000400000049006400000000000300000018000000310030002E00320039003600310031002E0030002E003000000000001400000050006F006C0069006300790049006E0066006F00000000001600000049006E0066006F0072006D006100740069006F006E00000000000000080000004E0061006D00650000000000030000009E0000004D006900630072006F0073006F00660074002000570069006E0064006F00770073002000430072006F00730073002000430065007200740069006600690063006100740065007300200066006F007200200043006F0064006500200049006E007400650067007200690074007900200045007800630065007000740069006F006E007300200041007500640069007400200050006F006C006900630079000000000000000300000000000000";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cip_meta_from_real_fixture() {
        let meta = parse_cip_meta(&unhex(CIP_FIXTURE_HEX));
        assert_eq!(meta.id.as_deref(), Some("10.29611.0.0"));
        assert_eq!(
            meta.name.as_deref(),
            Some("Microsoft Windows Cross Certificates for Code Integrity Exceptions Audit Policy")
        );
    }

    #[test]
    fn cip_meta_absent_on_garbage() {
        assert_eq!(parse_cip_meta(&[0u8; 64]), CipMeta::default());
        assert_eq!(parse_cip_meta(b"not a policy at all"), CipMeta::default());
    }

    #[test]
    fn ci_mode_heuristic() {
        assert_eq!(
            ci_mode_from_name(Some("… Exceptions Audit Policy")),
            CiPolicyMode::Audit
        );
        assert_eq!(
            ci_mode_from_name(Some("… Exceptions Enforced Policy")),
            CiPolicyMode::Enforce
        );
        assert_eq!(
            ci_mode_from_name(Some("Windows Driver Policy")),
            CiPolicyMode::Unknown
        );
        assert_eq!(ci_mode_from_name(None), CiPolicyMode::Unknown);
    }
}
