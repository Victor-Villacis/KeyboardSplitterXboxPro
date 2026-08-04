//! Read-only PnP device-tree enumeration.
//!
//! One `CM_Get_Device_ID_ListW(CM_GETIDLIST_FILTER_PRESENT)` call plus registry
//! reads under `HKLM\SYSTEM\CurrentControlSet\Enum`. That combination is
//! deliberate:
//!
//! - **Present-only.** The `Enum` tree is a graveyard — this machine has 38
//!   keyboard-class nodes in the registry and one of them is the I-PAC. Counting
//!   from the registry alone would make the "is this the last keyboard?" refusal
//!   meaningless, and that refusal is the thing standing between a user and a
//!   panel they cannot type on.
//! - **Nothing is opened.** `CM_Get_Device_ID_List` returns strings.
//!   `SetupDiOpenDeviceInterface`, `CreateFile` on a device path, and every
//!   `winusb.sys` claim primitive are absent from this module on purpose: a
//!   survey must never be able to disturb the thing it is surveying.

use crate::parse::parse_multi_sz;
use crate::winusb::{DeviceNode, NodeStatus};

use super::registry;

const ENUM: &str = "SYSTEM\\CurrentControlSet\\Enum";

/// `DN_STARTED` — "is running", the flag `CM_Get_DevNode_Status` sets when a
/// function driver is loaded and started for the node. Spelled out because
/// `windows-sys` does not export the `DN_*` set.
const DN_STARTED: u32 = 0x0000_0008;

/// Every device instance id currently *present* on the machine.
///
/// Returns an empty vector on failure rather than an error: a survey that
/// cannot enumerate must read as "nothing found", which every caller already
/// handles conservatively (an empty keyboard list refuses every claim).
pub fn present_instance_ids() -> Vec<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_GETIDLIST_FILTER_PRESENT,
        CR_SUCCESS,
    };

    let mut len: u32 = 0;
    // SAFETY: a null filter with FILTER_PRESENT is the documented "every present
    // device" form; `len` is a valid out-parameter.
    let cr = unsafe {
        CM_Get_Device_ID_List_SizeW(&mut len, std::ptr::null(), CM_GETIDLIST_FILTER_PRESENT)
    };
    if cr != CR_SUCCESS || len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` is `len` UTF-16 units, exactly what the size call asked for.
    let cr = unsafe {
        CM_Get_Device_ID_ListW(
            std::ptr::null(),
            buf.as_mut_ptr(),
            len,
            CM_GETIDLIST_FILTER_PRESENT,
        )
    };
    if cr != CR_SUCCESS {
        return Vec::new();
    }
    parse_multi_sz(&buf)
}

/// What the PnP manager says about one device right now.
///
/// "Present" is not the same as "working": a paired Bluetooth keyboard that is
/// switched off is present with `CM_PROB_DEVICE_NOT_CONNECTED` (45), a disabled
/// device is present with `CM_PROB_DISABLED` (22), and neither will type the
/// command that undoes a claim. This is the difference, and it is why the
/// last-keyboard refusal asks for it (`crate::winusb::Survey::keyboard_count`).
///
/// Read-only: `CM_Locate_DevNodeW` + `CM_Get_DevNode_Status` return numbers.
/// Nothing here opens a device.
pub fn status_for(instance_id: &str) -> Option<NodeStatus> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };

    let wide: Vec<u16> = instance_id
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut devinst = 0u32;
    // SAFETY: `wide` is a NUL-terminated UTF-16 instance id that outlives the
    // call; `devinst` is a valid out-parameter. NORMAL means "present devices
    // only", so a device that has genuinely gone away fails here.
    let cr = unsafe { CM_Locate_DevNodeW(&mut devinst, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    if cr != CR_SUCCESS {
        // Not locatable as a present device: it cannot type for anyone.
        return Some(NodeStatus {
            started: false,
            problem: 0,
        });
    }
    let mut status = 0u32;
    let mut problem = 0u32;
    // SAFETY: two valid out-parameters and a devinst we just located.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    if cr != CR_SUCCESS {
        // The question could not be answered. Say so (`None`) rather than
        // inventing a fault: `DeviceNode::is_live` then leaves the node alone.
        return None;
    }
    Some(NodeStatus {
        started: status & DN_STARTED != 0,
        problem,
    })
}

/// Read one device's `Enum` properties into a [`DeviceNode`].
pub fn node_for(instance_id: &str) -> DeviceNode {
    let key = format!("{ENUM}\\{instance_id}");
    let node = DeviceNode::new(
        instance_id,
        registry::read_string(&key, "ClassGUID"),
        registry::read_string(&key, "Service"),
        registry::read_string(&key, "DeviceDesc"),
        registry::read_string(&key, "ParentIdPrefix"),
    );
    match status_for(instance_id) {
        Some(status) => node.with_status(status),
        None => node,
    }
}

/// Every present device, with its `Enum` properties.
pub fn present_nodes() -> Vec<DeviceNode> {
    present_instance_ids()
        .iter()
        .map(|id| node_for(id))
        .collect()
}
