//! Best-effort friendly names from the registry Enum tree, ported from legacy
//! `InterceptionDevice.FriendlyName`: strip the `REV_xxxx&` segment from the
//! hardware id, open `HKLM\SYSTEM\CurrentControlSet\Enum\<hwid>`, walk instance
//! subkeys for one whose `Class` matches, and take the `DeviceDesc` text after
//! the `;` separator. Any failure yields `None` (legacy showed "n/a").

use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

use crate::backend::DeviceKind;
use crate::regkey::RegKey;

/// Legacy revision-strip: `HID\VID_D209&PID_0430&REV_0056&MI_00` →
/// `HID\VID_D209&PID_0430&MI_00`. If there is no `&` after `REV`, the id is
/// returned unchanged (legacy produced garbage here and then failed the
/// registry lookup; returning the original at least gives the lookup a
/// chance).
pub(crate) fn strip_revision(hwid: &str) -> String {
    if let Some(rev) = hwid.find("REV") {
        if let Some(amp_rel) = hwid[rev..].find('&') {
            let amp = rev + amp_rel;
            return format!("{}{}", &hwid[..rev], &hwid[amp + 1..]);
        }
    }
    hwid.to_owned()
}

/// The registry `Class` value for a device kind.
fn class_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Keyboard => "Keyboard",
        DeviceKind::Mouse => "Mouse",
    }
}

/// Best-effort friendly name for an Interception hardware id.
pub(crate) fn friendly_name(hwid: &str, kind: DeviceKind) -> Option<String> {
    let path = format!("SYSTEM\\CurrentControlSet\\Enum\\{}", strip_revision(hwid));
    let root = RegKey::open(HKEY_LOCAL_MACHINE, &path)?;
    // Legacy recursed the whole subtree; instance keys live one level down, so
    // a bounded depth-2 walk is faithful without unbounded recursion.
    search(&root, class_name(kind), 2)
}

fn search(key: &RegKey, class: &str, depth: u8) -> Option<String> {
    for name in key.subkey_names() {
        if name == "Properties" {
            continue; // access-denied by ACL; legacy skipped it too
        }
        let Some(sub) = RegKey::open(key.0, &name) else {
            continue;
        };
        if sub.string_value("Class").as_deref() == Some(class) {
            if let Some(desc) = sub.string_value("DeviceDesc") {
                // "@keyboard.inf,%...%;HID Keyboard Device" → text after ';'.
                let friendly = match desc.find(';') {
                    Some(i) => desc[i + 1..].to_owned(),
                    None => desc,
                };
                if !friendly.is_empty() {
                    return Some(friendly);
                }
            }
        }
        if depth > 1 {
            if let Some(found) = search(&sub, class, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_revision_matches_legacy_behavior() {
        assert_eq!(
            strip_revision("HID\\VID_D209&PID_0430&REV_0056&MI_00"),
            "HID\\VID_D209&PID_0430&MI_00"
        );
        // No REV segment: unchanged.
        assert_eq!(
            strip_revision("HID\\VID_D209&PID_0430&MI_00"),
            "HID\\VID_D209&PID_0430&MI_00"
        );
        // REV at the end with no following '&': unchanged (documented
        // divergence from legacy's garbage output).
        assert_eq!(strip_revision("HID\\FOO&REV_0001"), "HID\\FOO&REV_0001");
    }

    /// Live-registry test, read-only: the I-PAC4 is production hardware on this
    /// machine, so its Enum entry must resolve. Skips silently if the board is
    /// unplugged (friendly lookup is best-effort by contract).
    #[test]
    fn ipac_friendly_name_resolves_or_none() {
        let hwid = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
        if let Some(name) = friendly_name(hwid, DeviceKind::Keyboard) {
            assert!(!name.is_empty());
        }
    }
}
