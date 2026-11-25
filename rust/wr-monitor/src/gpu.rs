//! Each GPU's use for the gpu-graph app, the way `led_mon/gpu.py` reads it on Windows: the
//! "GPU Engine" performance counters (as Task Manager), with the GPUs' names from DXGI matched by
//! the adapter LUID in the counter names.
//!
//! Windows has one "GPU Engine" counter instance per process, GPU and engine (3D, copy, video ...),
//! named like `pid_1234_luid_0x00000000_0x0000D1F2_phys_0_eng_0_engtype_3D`. `wr_core::apps::gpu`
//! adds them up per engine type and keeps the busiest type per GPU, which is what Task Manager shows.
//! DXGI (DirectX Graphics Infrastructure) lists the real graphics adapters with their names and LUIDs
//! (a LUID is a number Windows gives each adapter until the next restart).
//!
//! `reader` runs on the GPU reading thread (`background.rs`); `names` is also called from the control
//! thread for the layout editor. `formatted_array` and `wide` are shared with `counters.rs`.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
};

use wr_core::apps::gpu::{busiest_engines, unique_names, Luid};

use crate::background::OnceLog;

/// PDH's "the buffer is too small" status, returned by the sizing call of `formatted_array`.
const PDH_MORE_DATA: u32 = 0x800007D2;
/// Values over 100 are kept (several engines of one type can add up past it).
const PDH_FMT_NOCAP100: u32 = 0x8000;

/// A hardware GPU: its name, LUID, and its own and borrowed (shared) memory in bytes.
///
/// The memory sizes come from DXGI's adapter description and are used by the GPU memory reading.
pub struct Adapter {
    /// The adapter's description, e.g. "AMD Radeon RX 7700S".
    pub name: String,
    /// (high part, low part) of the adapter's LUID, matching the counter instance names.
    pub luid: Luid,
    /// The GPU's own video memory, in bytes.
    pub dedicated: f64,
    /// System memory the GPU may borrow, in bytes.
    pub shared: f64,
}

/// The hardware GPUs, the main one first.
///
/// Software adapters (the "Microsoft Basic Render Driver") are skipped. Fails only if DXGI itself
/// can't be created; an adapter whose description can't be read is skipped.
pub fn adapters() -> windows::core::Result<Vec<Adapter>> {
    // SAFETY: plain COM calls; the interfaces release themselves when dropped.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut found = Vec::new();
        let mut index = 0;
        // EnumAdapters1 returns an error (DXGI_ERROR_NOT_FOUND) after the last adapter, ending the loop.
        // DXGI lists the adapter driving the main screen first.
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            index += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            // The name is a fixed 128-unit UTF-16 array ending in a zero; take the part before it.
            let len = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len]).trim().to_string();
            found.push(Adapter {
                name,
                luid: (desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart),
                dedicated: desc.DedicatedVideoMemory as f64,
                shared: desc.SharedSystemMemory as f64,
            });
        }
        Ok(found)
    }
}

/// The hardware GPUs' names, the main one first, for the layout editor's GPU picker.
///
/// Unlike the reader these are the plain names (no " #2" numbering); empty if DXGI fails.
pub fn names() -> Vec<String> {
    adapters().map(|a| a.into_iter().map(|a| a.name).collect()).unwrap_or_default()
}

/// A Rust string as UTF-16 with a closing zero, the form Windows' "W" functions take.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The open counter query. Lives on the GPU thread.
///
/// The raw handles aren't `Send`, which is why the reader is built on the thread that uses it.
struct Counters {
    /// The PDH query handle.
    query: *mut core::ffi::c_void,
    /// The wildcard "GPU Engine(*)\Utilization Percentage" counter in it.
    counter: *mut core::ffi::c_void,
}

impl Counters {
    /// Opens the query and adds the counter; the error message says which step failed.
    fn open() -> Result<Counters, String> {
        let path = wide(r"\GPU Engine(*)\Utilization Percentage");
        let mut query = std::ptr::null_mut();
        let mut counter = std::ptr::null_mut();
        // SAFETY: out-pointers to locals; the path is NUL-terminated.
        unsafe {
            let status = PdhOpenQueryW(std::ptr::null(), 0, &mut query);
            if status != 0 {
                return Err(format!("PdhOpenQuery failed ({status:#x})"));
            }
            // The English name works whatever language Windows is in; the wildcard picks up new processes
            let status = PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter);
            if status != 0 {
                return Err(format!("the GPU Engine counters aren't there ({status:#x})"));
            }
            // First sample now: a percentage needs two samples, so the next read gives real values.
            PdhCollectQueryData(query);
        }
        Ok(Counters { query, counter })
    }

    /// (instance name, percent) since the last call. Empty while there is no GPU work yet.
    fn read(&mut self) -> Vec<(String, f64)> {
        // SAFETY: the query and counter were opened together and stay open.
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return Vec::new();
            }
            formatted_array(self.counter)
        }
    }
}

/// Every instance's value of a wildcard counter, as (instance name, value).
///
/// # Safety
/// `counter` must be an open counter whose query was just collected.
///
/// Returns an empty list on any PDH error. Instances whose value isn't valid (CStatus != 0) are left
/// out. Values are not capped at 100 (`PDH_FMT_NOCAP100`).
pub unsafe fn formatted_array(counter: *mut core::ffi::c_void) -> Vec<(String, f64)> {
    // SAFETY: the buffer is sized by the first call, as PDH asks; names point into it.
    unsafe {
        let format = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
        let (mut size, mut count) = (0u32, 0u32);
        // First call with no buffer: PDH answers PDH_MORE_DATA and writes the size it needs in bytes.
        // That size covers the item array plus the instance-name strings stored after it.
        let status = PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, std::ptr::null_mut());
        if status != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // Allocated in whole items (rounded up) so the buffer is correctly aligned and at least `size`
        // bytes long. PDH fills it; only the first `count` items are items, the rest holds the names.
        let mut buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> = Vec::with_capacity(size as usize / item_size + 1);
        let status = PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, buffer.as_mut_ptr());
        if status != 0 {
            return Vec::new();
        }
        // Sound because PDH has just written `count` items, and `count` items fit in the capacity.
        buffer.set_len(count as usize);
        buffer
            .iter()
            .filter(|item| !item.szName.is_null() && item.FmtValue.CStatus == 0)
            .map(|item| {
                let mut len = 0;
                // Each name is a zero-terminated UTF-16 string inside the same buffer: find its end.
                while *item.szName.add(len) != 0 {
                    len += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len));
                (name, item.FmtValue.Anonymous.doubleValue)
            })
            .collect()
    }
}

/// Reads every GPU's use, 0.0-1.0, once a call. Made on the GPU thread.
///
/// Returns (GPU name, use) for each hardware GPU; names are made unique (" #2") for identical cards.
/// If the counters can't be opened it logs once and returns an empty list from then on.
pub fn reader() -> Box<dyn FnMut() -> Vec<(String, Option<f64>)>> {
    let mut counters: Option<Counters> = None;
    let mut failed = false;
    let mut gpus: Vec<(String, Luid)> = Vec::new();
    let mut gpus_read: Option<Instant> = None;
    let mut log = OnceLog::default();
    Box::new(move || -> Vec<(String, Option<f64>)> {
        if counters.is_none() && !failed {
            match Counters::open() {
                Ok(c) => counters = Some(c),
                Err(e) => {
                    wr_core::warn!("GPU use isn't available: {e}");
                    failed = true;
                }
            }
        }
        let Some(c) = counters.as_mut() else { return Vec::new() };
        // Now and then again, in case an external GPU is plugged in
        if gpus_read.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            gpus_read = Some(Instant::now());
            match adapters() {
                Ok(found) => {
                    log.ok("Listing the GPUs");
                    let names = unique_names(&found.iter().map(|a| a.name.clone()).collect::<Vec<_>>());
                    gpus = names.into_iter().zip(found.into_iter().map(|a| a.luid)).collect();
                }
                Err(e) => log.fail(format!("Can't list the GPUs: {e}")),
            }
        }
        let usage = busiest_engines(&c.read());
        // `busiest_engines` gives percent capped at 100, so dividing by 100 gives 0.0-1.0.
        // A GPU with no engine counters (idle) counts as 0.
        gpus.iter().map(|(name, luid)| (name.clone(), Some(usage.get(luid).copied().unwrap_or(0.0) / 100.0))).collect()
    })
}
