//! Live service state from the Service Control Manager. `SC_MANAGER_CONNECT` +
//! `SERVICE_QUERY_STATUS` need no elevation.

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST;
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_STATUS_PROCESS,
};

use crate::report::ServiceState;

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { _ = CloseServiceHandle(self.0) };
    }
}

pub fn query_state(service_name: &str) -> ServiceState {
    let name: Vec<u16> = service_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let scm = match unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) } {
        Ok(h) => Handle(h),
        Err(_) => return ServiceState::Unknown,
    };
    let svc = match unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), SERVICE_QUERY_STATUS) } {
        Ok(h) => Handle(h),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => {
            return ServiceState::NotRegistered
        }
        Err(_) => return ServiceState::Unknown,
    };

    let mut buf = [0u8; std::mem::size_of::<SERVICE_STATUS_PROCESS>()];
    let mut needed = 0u32;
    if unsafe { QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(&mut buf), &mut needed) }
        .is_err()
    {
        return ServiceState::Unknown;
    }
    let status: SERVICE_STATUS_PROCESS = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
    match status.dwCurrentState.0 {
        1 => ServiceState::Stopped,
        2 => ServiceState::StartPending,
        3 => ServiceState::StopPending,
        4 => ServiceState::Running,
        5 => ServiceState::ContinuePending,
        6 => ServiceState::PausePending,
        7 => ServiceState::Paused,
        _ => ServiceState::Unknown,
    }
}
