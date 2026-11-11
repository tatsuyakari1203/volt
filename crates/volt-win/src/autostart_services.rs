//! [`AutoStartServices`] port on the Service Control Manager. Read-only: the services are
//! enumerated and their configuration read; none is started, stopped or changed.

use volt_core::{AutoStartService, AutoStartServices, PortError};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, WIN32_ERROR};
use windows::Win32::System::Services::{
    ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfigW, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_AUTO_START, SERVICE_QUERY_CONFIG, SERVICE_START_TYPE,
    SERVICE_STATE_ALL, SERVICE_WIN32,
};
use windows::core::{PCWSTR, PWSTR};

use crate::error::from_win32;
use crate::services::{ScHandle, query_delayed, win32_of};

/// The service type flag of a per-user service instance (`SERVICE_USERSERVICE_INSTANCE`): one
/// copy exists per signed-in user, and the template it comes from is listed instead.
const USER_SERVICE_INSTANCE: u32 = 0x80;
/// Bytes asked for per enumeration call. The services returned so far are copied out before the
/// next call, so a large machine just takes a few more calls.
const ENUM_BUFFER_BYTES: usize = 64 * 1024;
/// A bound against a pathological machine.
const MAX_SERVICES: usize = 4000;

/// Lists the services that start automatically, with the Service Control Manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinAutoStartServices;

impl WinAutoStartServices {
    /// A reader of auto-start services.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// A service as the enumeration names it.
struct Named {
    name: Vec<u16>,
    display_name: String,
    service_type: u32,
}

/// A NUL-terminated wide string the SCM returned, as text; empty for a null pointer.
fn wide_text(text: PWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the pointer is a NUL-terminated string inside the enumeration buffer, which is alive
    // and unmodified while this runs.
    unsafe { text.to_string() }.unwrap_or_default()
}

/// A NUL-terminated wide string as units including the terminator; empty for a null pointer.
fn wide_units(text: PWSTR) -> Vec<u16> {
    if text.is_null() {
        return vec![0];
    }
    // SAFETY: as in `wide_text`.
    let units = unsafe { text.as_wide() };
    let mut owned = units.to_vec();
    owned.push(0);
    owned
}

fn enumerate(scm: &ScHandle) -> Result<Vec<Named>, PortError> {
    let mut found = Vec::new();
    let mut resume = 0_u32;
    // `u64` storage keeps the buffer 8-byte aligned for `ENUM_SERVICE_STATUS_PROCESSW`.
    let mut storage = vec![0_u64; ENUM_BUFFER_BYTES / 8];
    loop {
        let (mut needed, mut returned) = (0_u32, 0_u32);
        // SAFETY: the slice covers exactly the `storage` allocation, which stays alive for the
        // call; the out-pointers are valid locals.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), storage.len() * 8)
        };
        // SAFETY: `scm` is open with SC_MANAGER_ENUMERATE_SERVICE; the buffer, the out-pointers and
        // the resume handle are valid for the call; the group name is null (all groups).
        let result = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(bytes),
                &raw mut needed,
                &raw mut returned,
                Some(&raw mut resume),
                PCWSTR::null(),
            )
        };
        let items = storage.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for index in 0..returned as usize {
            // SAFETY: the call filled `returned` consecutive entries at the start of the buffer
            // (it also does so when it answers "more data").
            let item = unsafe { &*items.add(index) };
            found.push(Named {
                name: wide_units(item.lpServiceName),
                display_name: wide_text(item.lpDisplayName),
                service_type: item.ServiceStatusProcess.dwServiceType.0,
            });
        }
        match result {
            Ok(()) => return Ok(found),
            Err(error) if WIN32_ERROR(win32_of(&error)) == ERROR_MORE_DATA => {
                if found.len() >= MAX_SERVICES {
                    return Ok(found);
                }
            }
            Err(error) => {
                return Err(from_win32(win32_of(&error), "listing the services"));
            }
        }
    }
}

/// The start type, command line and account of an open service.
struct Config {
    start_type: u32,
    image_path: String,
    account: String,
}

fn read_config(handle: &ScHandle, context: &str) -> Result<Config, PortError> {
    let mut needed = 0_u32;
    // SAFETY: `handle` is open with SERVICE_QUERY_CONFIG; a null buffer of size 0 asks for the
    // required size, and `needed` is a valid out-pointer.
    match unsafe { QueryServiceConfigW(handle.0, None, 0, &raw mut needed) } {
        Ok(()) => {}
        Err(error) if WIN32_ERROR(win32_of(&error)) == ERROR_INSUFFICIENT_BUFFER => {}
        Err(error) => return Err(from_win32(win32_of(&error), context)),
    }
    // `u64` storage keeps the buffer 8-byte aligned for `QUERY_SERVICE_CONFIGW`.
    let mut storage = vec![0_u64; (needed as usize).div_ceil(8).max(1)];
    let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    let capacity = u32::try_from(storage.len() * 8)
        .map_err(|_| PortError::Other("service configuration too large".into()))?;
    // SAFETY: `config` points to `capacity` writable, aligned bytes that stay alive for the call;
    // `needed` is a valid out-pointer.
    unsafe { QueryServiceConfigW(handle.0, Some(config), capacity, &raw mut needed) }
        .map_err(|error| from_win32(win32_of(&error), context))?;
    // SAFETY: the call succeeded, so the buffer holds an initialised QUERY_SERVICE_CONFIGW whose
    // string pointers point into the same buffer, which is still alive.
    let (start_type, image_path, account) = unsafe {
        (
            (*config).dwStartType.0,
            wide_text((*config).lpBinaryPathName),
            wide_text((*config).lpServiceStartName),
        )
    };
    Ok(Config {
        start_type,
        image_path,
        account,
    })
}

impl AutoStartServices for WinAutoStartServices {
    fn auto_start_services(&self) -> Result<Vec<AutoStartService>, PortError> {
        // SAFETY: null machine and database names select the local active database; nothing is
        // retained after the call.
        let scm = unsafe {
            OpenSCManagerW(
                PCWSTR::null(),
                PCWSTR::null(),
                SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
            )
        }
        .map_err(|error| from_win32(win32_of(&error), "open the service control manager"))?;
        let scm = ScHandle(scm);

        let mut services = Vec::new();
        for named in enumerate(&scm)? {
            if named.service_type & USER_SERVICE_INSTANCE != 0 {
                continue;
            }
            // SAFETY: `scm` is an open SCM handle and `named.name` is NUL-terminated UTF-16 that
            // outlives the call; only the right to read the configuration is asked for.
            let Ok(handle) =
                (unsafe { OpenServiceW(scm.0, PCWSTR(named.name.as_ptr()), SERVICE_QUERY_CONFIG) })
            else {
                // A service the user may not read is left out, as with the task list.
                continue;
            };
            let handle = ScHandle(handle);
            let name = String::from_utf16_lossy(&named.name[..named.name.len() - 1]);
            let context = format!("service {name}");
            let Ok(config) = read_config(&handle, &context) else {
                continue;
            };
            if SERVICE_START_TYPE(config.start_type) != SERVICE_AUTO_START {
                continue;
            }
            let delayed = query_delayed(&handle, &context).unwrap_or(false);
            services.push(AutoStartService {
                display_name: if named.display_name.is_empty() {
                    name.clone()
                } else {
                    named.display_name
                },
                name,
                image_path: config.image_path,
                delayed,
                account: config.account,
            });
        }
        Ok(services)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auto_start_services_of_this_machine_read_without_failing() {
        // Read-only: lists what the current user may read, and changes nothing.
        let services = WinAutoStartServices::new()
            .auto_start_services()
            .expect("reads");
        assert!(
            !services.is_empty(),
            "every Windows has auto-start services"
        );
        for service in &services {
            assert!(!service.name.is_empty());
            assert!(!service.display_name.is_empty());
        }
        // The Remote Procedure Call service is automatic on every Windows.
        assert!(
            services
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case("RpcSs"))
        );
    }
}
