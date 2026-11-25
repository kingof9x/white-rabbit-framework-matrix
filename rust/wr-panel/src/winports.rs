//! Serial ports on Windows: the list with each port's USB IDs and position (as pyserial makes it in
//! `serial/tools/list_ports_windows.py`), and opening a port to write to a panel.
//!
//! Only compiled on Windows (`#[cfg(windows)]` in `lib.rs`). The monitor hands `WindowsPorts` to
//! `Panels::new`; from then on `Hub` calls `list` (from the drawing threads while they look for a
//! panel, and from the main loop's `align`) and `open`, and every write and read goes through
//! `SerialPort` on whichever thread holds the port's lock.
//!
//! Listing uses SetupAPI: ask Windows for the device classes named "Ports" and "Modem", enumerate the
//! devices of each class that are present now, and for each read its COM name from the registry, its
//! instance ID (which holds the USB vendor/product IDs and interface number), its location paths
//! (turned into a USB position by `ports::location_from_paths`) and its friendly name.
//!
//! Opening uses `CreateFileW` on `\\.\COMn` with no sharing (Windows' COM ports are exclusive), sets
//! 8 data bits, no parity, one stop bit, DTR and RTS on (as pyserial does), and a 2 s write timeout
//! so a panel that stops taking data shows up as an error instead of a hang.

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

/// The string as UTF-16 with a closing 0, the form the `...W` Windows functions take.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A UTF-16 buffer filled by Windows, up to its first 0 (or all of it if there is none). Invalid
/// UTF-16 becomes U+FFFD instead of an error.
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// The device class GUIDs registered under a class name ("Ports", "Modem"). Empty on error. At most 8
/// are kept; `size` is how many Windows has, which can be more than fit.
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

/// A string property of a device (e.g. SPDRP_FRIENDLYNAME), or None if it has none. For multi-string
/// properties (hardware IDs, location paths) only the first string is returned, which is the one used.
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
            // The size in bytes, less one UTF-16 character, so the buffer always ends in a 0 for `from_wide`.
            (buf.len() * 2 - 2) as u32,
            null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&buf))
}

/// The device's "PortName" value from its registry key (e.g. "COM4", or "LPT1" for a printer port).
/// None if it has no key or no such value.
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
        // Closed before the result is looked at, so the key is never leaked on either path.
        RegCloseKey(key);
        (r == 0).then(|| from_wide(&buf))
    }
}

/// The device's instance ID, e.g. `USB\VID_32AC&PID_0020&MI_00\7&1A2B3C&0&0000`, or its first hardware
/// ID if Windows won't give the instance ID; empty if neither.
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
/// Asks Windows afresh on every call (SetupAPI and registry reads), so callers fetch the list once and
/// reuse it. Errors for one class or device just leave it out.
pub fn list_ports() -> Vec<PortInfo> {
    let mut ports = Vec::new();
    let guids: Vec<GUID> = class_guids("Ports").into_iter().chain(class_guids("Modem")).collect();
    for guid in guids {
        // SAFETY: the device list is destroyed below; SP_DEVINFO_DATA has its size set.
        unsafe {
            let set = SetupDiGetClassDevsW(&guid, null(), null_mut(), DIGCF_PRESENT);
            // -1 is INVALID_HANDLE_VALUE for a device information set: this class can't be listed.
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
                // Printer ports are in the "Ports" class too.
                if device.starts_with("LPT") {
                    continue;
                }
                let hwid = instance_id(set, &info);
                let mut port = PortInfo { device, ..Default::default() };
                // Only USB devices have IDs and a USB position; a built-in serial port keeps just its name.
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
/// Closed when dropped (see `Drop`), which happens when the last user of the `SharedPort` lets go.
pub struct SerialPort {
    /// The Win32 file handle from `CreateFileW`; always valid while the `SerialPort` exists.
    handle: HANDLE,
}

// SAFETY: a COM port handle can be used from any thread; each SerialPort is used by one thread at a time.
unsafe impl Send for SerialPort {}

impl SerialPort {
    /// Opens "COMn" for reading and writing, exclusively. Fails with the OS error if the port doesn't
    /// exist, is already open (by this or another program: "access denied"), or can't be configured.
    pub fn open(name: &str) -> Result<SerialPort> {
        // The `\\.\` prefix is needed for COM10 and above, and works for every port.
        let path = wide(&format!(r"\\.\{name}"));
        // SAFETY: plain Win32 calls; the handle is closed on drop.
        unsafe {
            let handle =
                CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING, 0, null_mut());
            if handle == INVALID_HANDLE_VALUE {
                return Err(Error::last_os_error());
            }
            // Wrapped at once, so any early return below closes the handle through `Drop`.
            let port = SerialPort { handle };
            let mut dcb: DCB = std::mem::zeroed();
            dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
            if GetCommState(handle, &mut dcb) == 0 {
                return Err(Error::last_os_error());
            }
            dcb.BaudRate = BAUD_RATE;
            dcb.ByteSize = 8;
            // 0 = NOPARITY; StopBits 0 = ONESTOPBIT.
            dcb.Parity = 0;
            dcb.StopBits = 0;
            // fBinary, DTR on, RTS on: what pyserial sets
            // Bit 0 is fBinary (must be set on Windows), bits 4-5 fDtrControl and bits 12-13 fRtsControl,
            // where 1 means "on" (DTR_CONTROL_ENABLE, RTS_CONTROL_ENABLE). Every other flag is off: no flow
            // control, no character replacement.
            dcb._bitfield = 1 | (1 << 4) | (1 << 12);
            if SetCommState(handle, &dcb) == 0 {
                return Err(Error::last_os_error());
            }
            // A write that takes over 2 seconds means the panel is gone
            // Reads by default wait at most 100 ms in all (`read` sets its own timeout each time).
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: 0,
                ReadTotalTimeoutMultiplier: 0,
                ReadTotalTimeoutConstant: 100,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 2000,
            };
            // Failures of these two calls are ignored: the port still works, with Windows' default timeouts
            // or with old bytes left in its buffers.
            SetCommTimeouts(handle, &timeouts);
            PurgeComm(handle, 0x0004 | 0x0008); // PURGE_TXCLEAR | PURGE_RXCLEAR
            Ok(port)
        }
    }
}

impl Write for SerialPort {
    /// One `WriteFile` call, without overlapped I/O, so it blocks until the data is handed to the USB
    /// driver or the 2 s write timeout passes. A panel command (at most 38 bytes) goes out as one USB
    /// transfer, which is what the firmware needs.
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut written = 0u32;
        // SAFETY: the buffer and its length are passed together.
        let ok = unsafe { WriteFile(self.handle, buf.as_ptr(), buf.len() as u32, &mut written, null_mut()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        // The write timed out with nothing sent: the panel is gone or stuck. Reported as an error so the
        // port is marked broken (std's `write_all` would report it as `WriteZero` otherwise).
        if written == 0 && !buf.is_empty() {
            return Err(Error::new(std::io::ErrorKind::TimedOut, "the panel stopped taking data"));
        }
        Ok(written as usize)
    }

    /// Nothing to do: `WriteFile` returns once the data has been handed over.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Port for SerialPort {
    /// std's `write_all`, which repeats `write` until all of `data` is sent (in practice one call).
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        Write::write_all(self, data)
    }

    /// Waits up to `timeout` in all (at least 1 ms) for `size` bytes and returns what came, which may be
    /// fewer or none. The timeout is set on the port for every read, since each caller wants its own.
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

    /// Empties the driver's receive buffer (old answers nobody read).
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

/// The real `PortSystem`: lists ports with `list_ports` and opens them as `SerialPort`s.
pub struct WindowsPorts;

impl PortSystem for WindowsPorts {
    fn list(&self) -> Vec<PortInfo> {
        list_ports()
    }

    fn open(&self, name: &str) -> Result<Box<dyn Port>> {
        Ok(Box::new(SerialPort::open(name)?))
    }
}
