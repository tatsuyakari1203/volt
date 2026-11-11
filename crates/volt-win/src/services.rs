//! [`Services`] port on the Service Control Manager.

use std::ffi::c_void;

use volt_core::{PortError, ServiceStartType, Services};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, WIN32_ERROR};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW, SC_HANDLE,
    SC_MANAGER_CONNECT, SERVICE_AUTO_START, SERVICE_BOOT_START, SERVICE_CHANGE_CONFIG,
    SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DEMAND_START,
    SERVICE_DISABLED, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_START_TYPE,
    SERVICE_SYSTEM_START,
};
use windows::Win32::System::Services::{ENUM_SERVICE_TYPE, SERVICE_ERROR};
use windows::core::{BOOL, PCWSTR};

use crate::error::from_win32;
use crate::wide::to_wide_nul;

/// The services port backed by the Service Control Manager.
///
/// Each call opens the SCM with `SC_MANAGER_CONNECT` and the service with only
/// the right it needs: `SERVICE_QUERY_CONFIG` to read, `SERVICE_CHANGE_CONFIG`
/// to change.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinServices;

impl WinServices {
    /// A services adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// An SCM or service handle, closed on drop.
pub(crate) struct ScHandle(pub(crate) SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful OpenSCManagerW or
        // OpenServiceW call, is owned exclusively by this value and is closed
        // exactly once, here.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_service(service: &str, access: u32) -> Result<ScHandle, PortError> {
    if service.is_empty() {
        return Err(PortError::InvalidInput("empty service name".into()));
    }
    let name = to_wide_nul(service, "service name")?;
    let context = format!("service {service}");

    // SAFETY: null machine and database names select the local active
    // database; no pointers are retained after the call.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(|e| from_win32(win32_of(&e), "open the service control manager"))?;
    let scm = ScHandle(scm);

    // SAFETY: `scm` is a valid open SCM handle and `name` is NUL-terminated
    // UTF-16 that outlives the call.
    unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), access) }
        .map(ScHandle)
        .map_err(|e| from_win32(win32_of(&e), &context))
}

/// Extracts the Win32 code from an error returned by the `windows` crate.
pub(crate) fn win32_of(error: &windows::core::Error) -> u32 {
    let hr = error.code().0.cast_unsigned();
    if hr >> 16 == 0x8007 { hr & 0xFFFF } else { hr }
}

/// Maps the raw SCM start type (plus the delayed flag) to the model.
pub(crate) fn start_type_from_raw(raw: u32, delayed: bool) -> Result<ServiceStartType, PortError> {
    match SERVICE_START_TYPE(raw) {
        SERVICE_BOOT_START => Ok(ServiceStartType::Boot),
        SERVICE_SYSTEM_START => Ok(ServiceStartType::System),
        SERVICE_AUTO_START if delayed => Ok(ServiceStartType::AutomaticDelayed),
        SERVICE_AUTO_START => Ok(ServiceStartType::Automatic),
        SERVICE_DEMAND_START => Ok(ServiceStartType::Manual),
        SERVICE_DISABLED => Ok(ServiceStartType::Disabled),
        _ => Err(PortError::Other(format!(
            "unknown service start type {raw}"
        ))),
    }
}

/// Maps the model to the raw SCM start type and whether delayed start applies.
/// `None` for the delayed flag means "irrelevant for this start type".
pub(crate) const fn start_type_to_raw(start_type: ServiceStartType) -> (u32, Option<bool>) {
    match start_type {
        ServiceStartType::Boot => (SERVICE_BOOT_START.0, None),
        ServiceStartType::System => (SERVICE_SYSTEM_START.0, None),
        ServiceStartType::Automatic => (SERVICE_AUTO_START.0, Some(false)),
        ServiceStartType::AutomaticDelayed => (SERVICE_AUTO_START.0, Some(true)),
        ServiceStartType::Manual => (SERVICE_DEMAND_START.0, None),
        ServiceStartType::Disabled => (SERVICE_DISABLED.0, None),
    }
}

impl Services for WinServices {
    fn start_type(&self, service: &str) -> Result<ServiceStartType, PortError> {
        let handle = open_service(service, SERVICE_QUERY_CONFIG)?;
        let context = format!("service {service}");

        // First call sizes the buffer (it fails with ERROR_INSUFFICIENT_BUFFER).
        let mut needed = 0_u32;
        // SAFETY: `handle` is open with SERVICE_QUERY_CONFIG; a null buffer
        // with size 0 is the documented way to ask for the required size and
        // `needed` is a valid out-pointer.
        let sizing = unsafe { QueryServiceConfigW(handle.0, None, 0, &raw mut needed) };
        match sizing {
            Ok(()) => {}
            Err(e) if WIN32_ERROR(win32_of(&e)) == ERROR_INSUFFICIENT_BUFFER => {}
            Err(e) => return Err(from_win32(win32_of(&e), &context)),
        }

        // u64 storage keeps the buffer 8-byte aligned for QUERY_SERVICE_CONFIGW.
        let mut storage = vec![0_u64; (needed as usize).div_ceil(8).max(1)];
        let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
        let capacity = u32::try_from(storage.len() * 8)
            .map_err(|_| PortError::Other("service configuration too large".into()))?;
        // SAFETY: `config` points to `capacity` writable bytes, aligned for
        // QUERY_SERVICE_CONFIGW, that stay alive for the call; `needed` is a
        // valid out-pointer.
        unsafe { QueryServiceConfigW(handle.0, Some(config), capacity, &raw mut needed) }
            .map_err(|e| from_win32(win32_of(&e), &context))?;
        // SAFETY: the call above succeeded, so the buffer holds an initialised
        // QUERY_SERVICE_CONFIGW; only its plain integer field is read.
        let raw = unsafe { (*config).dwStartType.0 };

        let delayed = if SERVICE_START_TYPE(raw) == SERVICE_AUTO_START {
            query_delayed(&handle, &context)?
        } else {
            false
        };
        start_type_from_raw(raw, delayed)
    }

    fn set_start_type(&self, service: &str, start_type: ServiceStartType) -> Result<(), PortError> {
        let handle = open_service(service, SERVICE_CHANGE_CONFIG)?;
        let context = format!("service {service}");
        let (raw, delayed) = start_type_to_raw(start_type);

        // SAFETY: `handle` is open with SERVICE_CHANGE_CONFIG. Every
        // optional string argument is None (unchanged) and the type fields
        // are SERVICE_NO_CHANGE, so only the start type is modified.
        unsafe {
            ChangeServiceConfigW(
                handle.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(raw),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
        }
        .map_err(|e| from_win32(win32_of(&e), &context))?;

        // The delayed flag is only honoured for automatic start, so it is set
        // after the start type. If this second call fails the start type has
        // already changed; report it so the caller can revert.
        if let Some(delayed) = delayed {
            let mut info = SERVICE_DELAYED_AUTO_START_INFO {
                fDelayedAutostart: BOOL::from(delayed),
            };
            // SAFETY: `handle` is open with SERVICE_CHANGE_CONFIG and `info`
            // is a valid SERVICE_DELAYED_AUTO_START_INFO that outlives the
            // call, as required by SERVICE_CONFIG_DELAYED_AUTO_START_INFO.
            unsafe {
                ChangeServiceConfig2W(
                    handle.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    Some(std::ptr::from_mut(&mut info).cast::<c_void>()),
                )
            }
            .map_err(|e| {
                PortError::Other(format!(
                    "{context}: start type changed but the delayed-start flag could not be set: {}",
                    from_win32(win32_of(&e), "ChangeServiceConfig2")
                ))
            })?;
        }
        Ok(())
    }
}

/// Reads the delayed-auto-start flag of an automatic service.
pub(crate) fn query_delayed(handle: &ScHandle, context: &str) -> Result<bool, PortError> {
    let mut info = SERVICE_DELAYED_AUTO_START_INFO::default();
    let mut needed = 0_u32;
    let size = u32::try_from(size_of::<SERVICE_DELAYED_AUTO_START_INFO>())
        .map_err(|_| PortError::Other("struct size".into()))?;
    // SAFETY: `handle` is open with SERVICE_QUERY_CONFIG; the buffer is a
    // valid SERVICE_DELAYED_AUTO_START_INFO of exactly `size` bytes and
    // `needed` is a valid out-pointer.
    unsafe {
        QueryServiceConfig2W(
            handle.0,
            SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
            Some(std::slice::from_raw_parts_mut(
                std::ptr::from_mut(&mut info).cast::<u8>(),
                size as usize,
            )),
            &raw mut needed,
        )
    }
    .map_err(|e| from_win32(win32_of(&e), context))?;
    Ok(info.fDelayedAutostart.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_type_mapping_round_trips() {
        for start_type in [
            ServiceStartType::Boot,
            ServiceStartType::System,
            ServiceStartType::Automatic,
            ServiceStartType::AutomaticDelayed,
            ServiceStartType::Manual,
            ServiceStartType::Disabled,
        ] {
            let (raw, delayed) = start_type_to_raw(start_type);
            assert_eq!(
                start_type_from_raw(raw, delayed.unwrap_or(false)),
                Ok(start_type)
            );
        }
    }

    #[test]
    fn raw_values_match_the_documented_constants() {
        assert_eq!(start_type_to_raw(ServiceStartType::Boot).0, 0);
        assert_eq!(start_type_to_raw(ServiceStartType::System).0, 1);
        assert_eq!(start_type_to_raw(ServiceStartType::Automatic).0, 2);
        assert_eq!(start_type_to_raw(ServiceStartType::Manual).0, 3);
        assert_eq!(start_type_to_raw(ServiceStartType::Disabled).0, 4);
    }

    #[test]
    fn delayed_flag_only_matters_for_automatic() {
        assert_eq!(start_type_from_raw(3, true), Ok(ServiceStartType::Manual));
        assert_eq!(
            start_type_from_raw(2, true),
            Ok(ServiceStartType::AutomaticDelayed)
        );
        assert!(start_type_from_raw(99, false).is_err());
    }

    #[test]
    fn reads_the_start_type_of_a_service_every_windows_has() {
        // RpcSs (Remote Procedure Call) is always Automatic. Read-only.
        let start = WinServices::new().start_type("RpcSs").expect("read RpcSs");
        assert_eq!(start, ServiceStartType::Automatic);
        // Service names are case-insensitive.
        assert_eq!(WinServices::new().start_type("rpcss"), Ok(start));
    }

    #[test]
    fn reads_delayed_and_manual_services_without_error() {
        // Whatever their configuration, these exist on supported Windows and
        // must be readable by a standard user.
        for name in ["EventLog", "Dhcp", "wuauserv"] {
            WinServices::new().start_type(name).expect(name);
        }
    }

    #[test]
    fn unknown_service_is_not_found() {
        let services = WinServices::new();
        let name = "VoltTestNoSuchService";
        assert!(matches!(
            services.start_type(name),
            Err(PortError::NotFound(_))
        ));
        // The only set_start_type call the automated tests make: on a service
        // that does not exist, so no real service is ever changed.
        assert!(matches!(
            services.set_start_type(name, ServiceStartType::Disabled),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn empty_and_nul_service_names_are_invalid_input() {
        let services = WinServices::new();
        assert!(matches!(
            services.start_type(""),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            services.start_type("a\0b"),
            Err(PortError::InvalidInput(_))
        ));
    }
}
