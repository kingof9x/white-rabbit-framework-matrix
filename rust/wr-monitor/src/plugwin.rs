//! The Windows side of [`crate::plug`]: a hidden window that Windows tells about every USB device that
//! arrives or leaves (WM_DEVICECHANGE) and about the charger (WM_POWERBROADCAST), the devices'
//! properties from the configuration manager, and framework_tool --pd-info for the charger's port.
//! A port of the Windows parts of `led_mon/plug_events.py`.

use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::{Arc, OnceLock};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW, CM_Get_Parent,
    CM_Locate_DevNodeW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
};
use windows_sys::Win32::Foundation::{DEVPROPKEY, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use wr_core::{info, warn};
use wr_panel::winports::wide;

use crate::plug::{instance_id_from_interface, parse_pd_info, DeviceProps, PlugWatcher};

const GUID_DEVINTERFACE_USB_DEVICE: GUID = GUID::from_u128(0xA5DCBF10_6530_11D2_901F_00C04FB951ED);
/// Screens: one plugged into an HDMI or DisplayPort card arrives as a monitor device.
const GUID_DEVINTERFACE_MONITOR: GUID = GUID::from_u128(0xE6F07B5F_EE97_4A90_B076_33F57BF4EAA7);
const DEVPKEY_LOCATION_PATHS: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0xA45C254E_DF1C_4EFD_8020_67D146A850E0), pid: 37 };
const DEVPKEY_PHYSICAL_LOCATION: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x540B947E_8B40_45BC_A8A2_6A0B894CBDA2), pid: 9 };
const DEVPKEY_BUS_REPORTED_DESC: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x540B947E_8B40_45BC_A8A2_6A0B894CBDA2), pid: 4 };
const DEVPKEY_DEVICE_DESC: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0xA45C254E_DF1C_4EFD_8020_67D146A850E0), pid: 2 };

static WATCHER: OnceLock<Arc<PlugWatcher>> = OnceLock::new();

/// Whether the charger is plugged in, or None if Windows can't say.
pub fn ac_online() -> Option<bool> {
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: plain call filling our struct.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }
    match status.ACLineStatus {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// Which port the charger is in, from framework_tool --pd-info.
fn charger_port() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let tool = crate::thermal::find_framework_tool()?;
    let out = Command::new(tool)
        .arg("--pd-info")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    parse_pd_info(&String::from_utf8_lossy(&out.stdout))
}

fn property(node: u32, key: &DEVPROPKEY) -> Option<Vec<u8>> {
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: the first call asks for the size; the second fills a buffer of that size.
    unsafe {
        CM_Get_DevNode_PropertyW(node, key, &mut kind, null_mut(), &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if CM_Get_DevNode_PropertyW(node, key, &mut kind, buf.as_mut_ptr(), &mut size, 0) != 0 {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

/// The strings of a REG_MULTI_SZ / string property.
fn strings(raw: Option<Vec<u8>>) -> Vec<String> {
    let Some(raw) = raw else { return vec![] };
    let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn device_properties(instance_id: &str) -> DeviceProps {
    let mut out = DeviceProps::default();
    let id = wide(instance_id);
    let mut node = 0u32;
    // SAFETY: plain call with a null-terminated id.
    if unsafe { CM_Locate_DevNodeW(&mut node, id.as_ptr(), 0) } != 0 {
        return out;
    }
    out.name = strings(property(node, &DEVPKEY_BUS_REPORTED_DESC))
        .into_iter()
        .next()
        .or_else(|| strings(property(node, &DEVPKEY_DEVICE_DESC)).into_iter().next());
    // Walk up to the port: the device itself, then its parents (a hub in the port, ...). A screen's
    // parent is the graphics card, which says nothing about the port, so only the screen itself.
    let levels = if crate::plug::is_display(instance_id) { 1 } else { 6 };
    for _ in 0..levels {
        if out.location_paths.is_empty() {
            out.location_paths = strings(property(node, &DEVPKEY_LOCATION_PATHS));
        }
        if out.pld.is_none() {
            out.pld = property(node, &DEVPKEY_PHYSICAL_LOCATION);
        }
        if !out.location_paths.is_empty() && out.pld.is_some() {
            break;
        }
        let mut parent = 0u32;
        // SAFETY: plain call.
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != 0 {
            break;
        }
        node = parent;
    }
    out
}

fn present_devices(class: &GUID) -> Vec<String> {
    let mut size = 0u32;
    // SAFETY: the size call, then a buffer of that many UTF-16 units.
    unsafe {
        if CM_Get_Device_Interface_List_SizeW(&mut size, class, null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != 0
            || size == 0
        {
            return vec![];
        }
        let mut buf = vec![0u16; size as usize];
        if CM_Get_Device_Interface_ListW(class, null(), buf.as_mut_ptr(), size, CM_GET_DEVICE_INTERFACE_LIST_PRESENT)
            != 0
        {
            return vec![];
        }
        String::from_utf16_lossy(&buf)
            .split('\0')
            .filter(|s| !s.is_empty())
            .filter_map(instance_id_from_interface)
            .collect()
    }
}

/// The interface name in a WM_DEVICECHANGE message, for device interface messages.
unsafe fn interface_name(lparam: LPARAM) -> Option<String> {
    let header = &*(lparam as *const DEV_BROADCAST_HDR);
    if header.dbch_devicetype != DBT_DEVTYP_DEVICEINTERFACE {
        return None;
    }
    let iface = lparam as *const DEV_BROADCAST_DEVICEINTERFACE_W;
    let name = std::ptr::addr_of!((*iface).dbcc_name) as *const u16;
    let len = (0..).take_while(|&i| *name.add(i) != 0).count();
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(name, len)))
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(watcher) = WATCHER.get() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_DEVICECHANGE
            if lparam != 0 && (wparam as u32 == DBT_DEVICEARRIVAL || wparam as u32 == DBT_DEVICEREMOVECOMPLETE) =>
        {
            if let Some(id) = interface_name(lparam).as_deref().and_then(instance_id_from_interface) {
                let arrived = wparam as u32 == DBT_DEVICEARRIVAL;
                let watcher = watcher.clone();
                // Off the window's thread: reading the device's properties can take a moment
                std::thread::spawn(move || {
                    if arrived {
                        watcher.device_arrived(&id, &device_properties(&id));
                    } else {
                        watcher.device_removed(&id);
                    }
                });
            }
            1
        }
        WM_POWERBROADCAST => {
            match wparam as u32 {
                PBT_APMPOWERSTATUSCHANGE => {
                    let watcher = watcher.clone();
                    std::thread::spawn(move || watcher.power_changed(ac_online(), charger_port));
                }
                PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => watcher.resumed(),
                _ => {}
            }
            1
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Starts listening on its own thread.
pub fn start(watcher: Arc<PlugWatcher>) {
    if WATCHER.set(watcher.clone()).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("plug-events".into()).spawn(move || {
        watcher.set_ac_online(ac_online());
        for class in [&GUID_DEVINTERFACE_USB_DEVICE, &GUID_DEVINTERFACE_MONITOR] {
            for id in present_devices(class) {
                watcher.remember(&id, &device_properties(&id));
            }
        }
        let class = wide("WhiteRabbitPlugEvents");
        // SAFETY: standard window class and window; the window lives as long as the program.
        unsafe {
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(window_proc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let title = wide("White Rabbit plug events");
            // A hidden top-level window: power broadcasts don't reach message-only windows
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                warn!("Couldn't listen for things being plugged in (no window)");
                return;
            }
            for (class, what) in [(GUID_DEVINTERFACE_USB_DEVICE, "USB device"), (GUID_DEVINTERFACE_MONITOR, "screen")] {
                let mut filter: DEV_BROADCAST_DEVICEINTERFACE_W = std::mem::zeroed();
                filter.dbcc_size = std::mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32;
                filter.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE;
                filter.dbcc_classguid = class;
                let registration = RegisterDeviceNotificationW(
                    hwnd as HANDLE,
                    &filter as *const _ as *const core::ffi::c_void,
                    DEVICE_NOTIFY_WINDOW_HANDLE,
                );
                if registration.is_null() {
                    warn!("Couldn't register for {what} messages");
                }
            }
            info!("Listening for things being plugged in");
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    if let Err(e) = spawned {
        warn!("Couldn't listen for things being plugged in: {e}");
    }
}
