//! Serial ports on Windows: the list with each port's USB IDs and position (as pyserial makes it in
//! `serial/tools/list_ports_windows.py`), and opening a port to write to a panel.

use std::io::{Error, Result, Write};
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Communication::{
    GetCommState, PurgeComm, SetCommState, SetCommTimeouts, COMMTIMEOUTS, DCB,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
use windows_sys::Win32::System::Registry::{RegCloseKey, RegQueryValueExW, KEY_READ};

use crate::ports::{location_from_paths, usb_ids, PortInfo};
use crate::protocol::BAUD_RATE;

use crate::hub::{Port, PortSystem};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn class_guids(name: &str) -> Vec<GUID> {
    let mut guids = [GUID::from_u128(0); 8];
    let mut size = 0u32;
    let name = wide(name);
    // SAFETY: the buffer holds 8 GUIDs and Windows is told so.
    let ok = unsafe { SetupDiClassGuidsFromNameW(name.as_ptr(), guids.as_mut_ptr(), guids.len() as u32, &mut size) };
    if ok == 0 {
        return Vec::new();
    }
    guids[..(size as usize).min(guids.len())].to_vec()
}

fn registry_string(set: HDEVINFO, info: &SP_DEVINFO_DATA, property: u32) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: the buffer's size in bytes is passed along with it.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            info,
            property,
            null_mut(),
            buf.as_mut_ptr() as *mut u8,
            (buf.len() * 2 - 2) as u32,
            null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&buf))
}

fn port_name(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> Option<String> {
    // SAFETY: plain SetupAPI / registry calls with buffers whose sizes are passed along.
    unsafe {
        let key = SetupDiOpenDevRegKey(set, info, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ);
        if key.is_null() || std::ptr::eq(key, INVALID_HANDLE_VALUE) {
            return None;
        }
        let mut buf = [0u16; 250];
        let mut len = (buf.len() * 2) as u32;
        let name = wide("PortName");
        let r = RegQueryValueExW(key, name.as_ptr(), null(), null_mut(), buf.as_mut_ptr() as *mut u8, &mut len);
        RegCloseKey(key);
        (r == 0).then(|| from_wide(&buf))
    }
}

fn instance_id(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> String {
    let mut buf = [0u16; 250];
    // SAFETY: the buffer's length in characters is passed along with it.
    let ok = unsafe { SetupDiGetDeviceInstanceIdW(set, info, buf.as_mut_ptr(), (buf.len() - 1) as u32, null_mut()) };
    if ok != 0 {
        from_wide(&buf)
    } else {
        registry_string(set, info, SPDRP_HARDWAREID).unwrap_or_default()
    }
}

/// Every serial port Windows knows about (only the ones present now).
pub fn list_ports() -> Vec<PortInfo> {
    let mut ports = Vec::new();
    let guids: Vec<GUID> = class_guids("Ports").into_iter().chain(class_guids("Modem")).collect();
    for guid in guids {
        // SAFETY: the device list is destroyed below; SP_DEVINFO_DATA has its size set.
        unsafe {
            let set = SetupDiGetClassDevsW(&guid, null(), null_mut(), DIGCF_PRESENT);
            if set == -1 {
                continue;
            }
            let mut index = 0;
            loop {
                let mut info: SP_DEVINFO_DATA = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(set, index, &mut info) == 0 {
                    break;
                }
                index += 1;
                let Some(device) = port_name(set, &info) else { continue };
                if device.starts_with("LPT") {
                    continue;
                }
                let hwid = instance_id(set, &info);
                let mut port = PortInfo { device, ..Default::default() };
                if hwid.to_uppercase().starts_with("USB") {
                    let (vid, pid, interface) = usb_ids(&hwid);
                    port.vid = vid;
                    port.pid = pid;
                    if let Some(paths) = registry_string(set, &info, SPDRP_LOCATION_PATHS) {
                        port.location = location_from_paths(&paths, interface);
                    }
                }
                port.description = registry_string(set, &info, SPDRP_FRIENDLYNAME).unwrap_or_default();
                ports.push(port);
            }
            SetupDiDestroyDeviceInfoList(set);
        }
    }
    ports
}

/// An open COM port.
pub struct SerialPort {
    handle: HANDLE,
}

// SAFETY: a COM port handle can be used from any thread; each SerialPort is used by one thread at a time.
unsafe impl Send for SerialPort {}

impl SerialPort {
    pub fn open(name: &str) -> Result<SerialPort> {
        let path = wide(&format!(r"\\.\{name}"));
        // SAFETY: plain Win32 calls; the handle is closed on drop.
        unsafe {
            let handle =
                CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING, 0, null_mut());
            if handle == INVALID_HANDLE_VALUE {
                return Err(Error::last_os_error());
            }
            let port = SerialPort { handle };
            let mut dcb: DCB = std::mem::zeroed();
            dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
            if GetCommState(handle, &mut dcb) == 0 {
                return Err(Error::last_os_error());
            }
            dcb.BaudRate = BAUD_RATE;
            dcb.ByteSize = 8;
            dcb.Parity = 0;
            dcb.StopBits = 0;
            // fBinary, DTR on, RTS on: what pyserial sets
            dcb._bitfield = 1 | (1 << 4) | (1 << 12);
            if SetCommState(handle, &dcb) == 0 {
                return Err(Error::last_os_error());
            }
            // A write that takes over 2 seconds means the panel is gone
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: 0,
                ReadTotalTimeoutMultiplier: 0,
                ReadTotalTimeoutConstant: 100,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 2000,
            };
            SetCommTimeouts(handle, &timeouts);
            PurgeComm(handle, 0x0004 | 0x0008); // PURGE_TXCLEAR | PURGE_RXCLEAR
            Ok(port)
        }
    }
}

impl Write for SerialPort {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut written = 0u32;
        // SAFETY: the buffer and its length are passed together.
        let ok = unsafe { WriteFile(self.handle, buf.as_ptr(), buf.len() as u32, &mut written, null_mut()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        if written == 0 && !buf.is_empty() {
            return Err(Error::new(std::io::ErrorKind::TimedOut, "the panel stopped taking data"));
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Port for SerialPort {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        Write::write_all(self, data)
    }

    fn read(&mut self, size: usize, timeout: Duration) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; size];
        let mut read = 0u32;
        // SAFETY: plain Win32 calls on our own handle; the buffer and its length are passed together.
        unsafe {
            // Wait up to `timeout` in all for `size` bytes, then give back what came
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: 0,
                ReadTotalTimeoutMultiplier: 0,
                ReadTotalTimeoutConstant: timeout.as_millis().clamp(1, u32::MAX as u128) as u32,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 2000,
            };
            SetCommTimeouts(self.handle, &timeouts);
            if ReadFile(self.handle, buf.as_mut_ptr(), size as u32, &mut read, null_mut()) == 0 {
                return Err(Error::last_os_error());
            }
        }
        buf.truncate(read as usize);
        Ok(buf)
    }

    fn clear_input(&mut self) -> Result<()> {
        // SAFETY: plain Win32 call on our own handle.
        if unsafe { PurgeComm(self.handle, 0x0008) } == 0 {
            // PURGE_RXCLEAR
            return Err(Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for SerialPort {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub struct WindowsPorts;

impl PortSystem for WindowsPorts {
    fn list(&self) -> Vec<PortInfo> {
        list_ports()
    }

    fn open(&self, name: &str) -> Result<Box<dyn Port>> {
        Ok(Box::new(SerialPort::open(name)?))
    }
}
