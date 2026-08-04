//! Children of the ViGEmBus devnode — the virtual pads the bus is exposing
//! right now.
//!
//! Two constraints shape this module:
//!
//! - **The bus devnode is found by its service name** (`ViGEmBus`), the same
//!   key the driver-health collector reads its service info under — never by
//!   matching pad VID/PIDs across the machine. A real DualShock or Xbox pad on
//!   a physical USB port carries the very same ids as a ViGEm persona; it hangs
//!   off a hub devnode, so a walk rooted at the bus can never count it.
//! - **Read-only cfgmgr32.** Locate/child/sibling/device-id return numbers and
//!   strings; nothing here opens a device, so the check cannot disturb a pad a
//!   running splitter is driving.
//!
//! Every failure path returns "no devices" rather than an error, in the
//! [`crate::collect`] tradition: a machine without the bus produces an empty —
//! and correct — report.

use crate::parse::parse_multi_sz;

/// Devnodes *registered* to the named service. Registration is not presence:
/// an uninstalled or stopped bus can leave a registered devnode behind, so
/// every id returned here must still pass the presence check in
/// [`child_instance_ids`] before it is reported or named in advice.
///
/// `CM_GETIDLIST_DONOTGENERATE` matters: without it, asking for a service's
/// devices can fabricate a phantom devnode for a service that has none, and a
/// phantom bus would then be "present with zero pads" instead of absent.
pub fn bus_instance_ids(service: &str) -> Vec<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_GETIDLIST_DONOTGENERATE,
        CM_GETIDLIST_FILTER_SERVICE, CR_SUCCESS,
    };

    let filter: Vec<u16> = service.encode_utf16().chain(std::iter::once(0)).collect();
    let flags = CM_GETIDLIST_FILTER_SERVICE | CM_GETIDLIST_DONOTGENERATE;
    let mut len: u32 = 0;
    // SAFETY: `filter` is a NUL-terminated UTF-16 service name that outlives
    // the call; `len` is a valid out-parameter.
    let cr = unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), flags) };
    if cr != CR_SUCCESS || len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` is `len` UTF-16 units, exactly what the size call asked for.
    let cr = unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), buf.as_mut_ptr(), len, flags) };
    if cr != CR_SUCCESS {
        return Vec::new();
    }
    parse_multi_sz(&buf)
}

/// Instance ids of every direct child of `instance_id`, in tree order.
///
/// `None` means the devnode is not *present* right now (registered leftover of
/// an uninstalled or stopped bus) — the caller must not report it or name it
/// in `pnputil /restart-device` advice. `Some(vec![])` is a present bus with
/// no pads: the healthy idle state.
pub fn child_instance_ids(instance_id: &str) -> Option<Vec<String>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Child, CM_Get_Sibling, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };

    let wide: Vec<u16> = instance_id
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut bus = 0u32;
    // SAFETY: `wide` is a NUL-terminated UTF-16 instance id that outlives the
    // call; `bus` is a valid out-parameter. NORMAL restricts to present
    // devices, so a stopped or removed bus reports no children rather than
    // stale ones.
    let cr = unsafe { CM_Locate_DevNodeW(&mut bus, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    if cr != CR_SUCCESS {
        return None;
    }
    let mut out = Vec::new();
    let mut child = 0u32;
    // SAFETY: `child` is a valid out-parameter and `bus` a devinst we located.
    if unsafe { CM_Get_Child(&mut child, bus, 0) } != CR_SUCCESS {
        // CR_NO_SUCH_DEVNODE: a bus with no children — the healthy idle state.
        return Some(out);
    }
    loop {
        if let Some(id) = device_id_of(child) {
            out.push(id);
        }
        let mut next = 0u32;
        // SAFETY: `next` is a valid out-parameter and `child` came from the
        // previous CM_Get_Child/CM_Get_Sibling step.
        if unsafe { CM_Get_Sibling(&mut next, child, 0) } != CR_SUCCESS {
            break;
        }
        child = next;
    }
    Some(out)
}

fn device_id_of(devinst: u32) -> Option<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_IDW, CM_Get_Device_ID_Size, CR_SUCCESS,
    };

    let mut len = 0u32;
    // SAFETY: `len` is a valid out-parameter and `devinst` came from the walk.
    if unsafe { CM_Get_Device_ID_Size(&mut len, devinst, 0) } != CR_SUCCESS || len == 0 {
        return None;
    }
    // The size call reports the length WITHOUT the terminating NUL.
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: `buf` holds len + 1 units, the documented buffer requirement.
    if unsafe { CM_Get_Device_IDW(devinst, buf.as_mut_ptr(), len + 1, 0) } != CR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}
