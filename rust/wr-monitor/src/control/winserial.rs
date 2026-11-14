//! Serial ports on Windows that can be read too, so the window gets the panels' replies.
//!
//! The same settings as `wr_panel::winports::SerialPort` (which only writes), plus reading with a
//! timeout and clearing what came in. When wr-panel's port learns to read, this can go.

use std::io::{Error, ErrorKind, Read, Result, Write};
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Devices::Communication::{
    GetCommState, PurgeComm, SetCommState, SetCommTimeouts, COMMTIMEOUTS, DCB, PURGE_RXCLEAR, PURGE_TXCLEAR,
};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};

use super::bridge::{RawPort, RawPorts};
use wr_panel::ports::PortInfo;
use wr_panel::protocol::BAUD_RATE;
use wr_panel::winports::{list_ports, wide};

/// A write that takes over 2 seconds means the panel is gone.
const WRITE_TIMEOUT_MS: u32 = 2000;

pub struct Port {
    handle: HANDLE,
}

// SAFETY: a COM port handle can be used from any thread; the Bridge locks each port around its use.
unsafe impl Send for Port {}

impl Port {
    pub fn open(name: &str) -> Result<Port> {
        let path = wide(&format!(r"\\.\{name}"));
        // SAFETY: plain Win32 calls; the handle is closed on drop.
        unsafe {
            let handle =
                CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING, 0, null_mut());
            if handle == INVALID_HANDLE_VALUE {
                return Err(Error::last_os_error());
            }
            let mut port = Port { handle };
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
            port.set_read_timeout(Duration::from_millis(100))?;
            PurgeComm(handle, PURGE_TXCLEAR | PURGE_RXCLEAR);
            Ok(port)
        }
    }
}

impl Read for Port {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut read = 0u32;
        // SAFETY: the buffer and its length are passed together.
        let ok = unsafe { ReadFile(self.handle, buf.as_mut_ptr(), buf.len() as u32, &mut read, null_mut()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        // Nothing within the timeout reads as 0 bytes, as pyserial gives b""
        Ok(read as usize)
    }
}

impl Write for Port {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut written = 0u32;
        // SAFETY: the buffer and its length are passed together.
        let ok = unsafe { WriteFile(self.handle, buf.as_ptr(), buf.len() as u32, &mut written, null_mut()) };
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        if written == 0 && !buf.is_empty() {
            return Err(Error::new(ErrorKind::TimedOut, "the panel stopped taking data"));
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl RawPort for Port {
    fn set_read_timeout(&mut self, timeout: Duration) -> Result<()> {
        // A read returns when all the bytes asked for came, or after the timeout with what came
        let timeouts = COMMTIMEOUTS {
            ReadIntervalTimeout: 0,
            ReadTotalTimeoutMultiplier: 0,
            ReadTotalTimeoutConstant: timeout.as_millis().clamp(1, u32::MAX as u128) as u32,
            WriteTotalTimeoutMultiplier: 0,
            WriteTotalTimeoutConstant: WRITE_TIMEOUT_MS,
        };
        // SAFETY: plain call on our own handle.
        if unsafe { SetCommTimeouts(self.handle, &timeouts) } == 0 {
            return Err(Error::last_os_error());
        }
        Ok(())
    }

    fn reset_input(&mut self) -> Result<()> {
        // SAFETY: plain call on our own handle.
        if unsafe { PurgeComm(self.handle, PURGE_RXCLEAR) } == 0 {
            return Err(Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub struct WindowsRawPorts;

impl RawPorts for WindowsRawPorts {
    fn list(&self) -> Vec<PortInfo> {
        list_ports()
    }

    fn open(&self, name: &str) -> Result<Box<dyn RawPort>> {
        Ok(Box::new(Port::open(name)?))
    }
}
