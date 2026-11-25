//! Readings from Windows performance counters (the ones Task Manager shows): every physical disk's
//! bytes read and written, and how much memory each GPU uses.
//!
//! Performance counters are read through PDH (the Performance Data Helper library): open a "query",
//! add counters to it by path (like `\PhysicalDisk(_Total)\Disk Read Bytes/sec`), then call
//! `PdhCollectQueryData` each time a fresh sample is wanted and read the values out. A `(*)` in a path
//! is a wildcard: one counter that returns a value for every instance (every GPU, every disk ...).
//!
//! The readers here are made and called on a `background.rs` reading thread (see `sensors.rs`), never
//! on the main loop, because PDH calls can take a while. PDH handles aren't thread-safe to share, so
//! each reader keeps its own query for the life of that thread.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetRawCounterValue, PdhOpenQueryW, PDH_RAW_COUNTER,
};

use wr_core::apps::gpu::{memory_share, parse_adapter_luid, unique_names, Luid};

use crate::background::OnceLog;
use crate::gpu::{adapters, formatted_array, wide};

/// A PDH query or counter handle (an opaque pointer owned by PDH).
type Handle = *mut core::ffi::c_void;

/// A query with some counters in it. Lives on the thread that made it.
struct Query {
    /// The PDH query; never closed, as it lives as long as its reading thread (the whole run).
    query: Handle,
    /// The counters in the query, in the order their paths were given to `open`.
    counters: Vec<Handle>,
}

impl Query {
    /// Opens a query with one counter per path. Fails with a readable message (including PDH's status
    /// code in hex) if the query can't be opened or any counter is missing, e.g. on a Windows with
    /// performance counters disabled or damaged.
    fn open(paths: &[&str]) -> Result<Query, String> {
        let mut query = std::ptr::null_mut();
        // SAFETY: out-pointers to locals; the paths are NUL-terminated.
        unsafe {
            let status = PdhOpenQueryW(std::ptr::null(), 0, &mut query);
            if status != 0 {
                return Err(format!("PdhOpenQuery failed ({status:#x})"));
            }
            let mut counters = Vec::new();
            for path in paths {
                let mut counter = std::ptr::null_mut();
                // The English name works whatever language Windows is in
                let status = PdhAddEnglishCounterW(query, wide(path).as_ptr(), 0, &mut counter);
                if status != 0 {
                    return Err(format!("the counter {path} isn't there ({status:#x})"));
                }
                counters.push(counter);
            }
            // A first sample right away: rate and percent counters need two samples, so the next
            // collect already gives a usable value.
            PdhCollectQueryData(query);
            Ok(Query { query, counters })
        }
    }

    /// Takes a new sample of every counter in the query. False if PDH failed.
    fn collect(&mut self) -> bool {
        // SAFETY: the query is open.
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    /// The counter's raw value: for a "bytes/sec" counter, the bytes counted since Windows started.
    fn raw(&self, i: usize) -> Option<i64> {
        let mut value: PDH_RAW_COUNTER = unsafe { std::mem::zeroed() };
        // SAFETY: the counter is open; the value is a local.
        let status = unsafe { PdhGetRawCounterValue(self.counters[i], std::ptr::null_mut(), &mut value) };
        // CStatus is the counter's own status (0 = valid data); FirstValue is the raw 64-bit count.
        (status == 0 && value.CStatus == 0).then_some(value.FirstValue)
    }

    /// Every instance of wildcard counter `i`, as (instance name, value).
    fn all(&self, i: usize) -> Vec<(String, f64)> {
        // SAFETY: the counter is open and its query was just collected.
        unsafe { formatted_array(self.counters[i]) }
    }
}

/// Bytes read and written on all physical disks together (every SSD, whether or not it has a drive
/// letter), counted since Windows started. None when the counters can't be read; the monitor then
/// falls back to adding up the drive letters.
///
/// The raw totals are returned (not PDH's per-second rate) so the caller can work out the speed
/// itself over whatever interval it reads at. Opening is tried once; after a failure the reader keeps
/// returning None without retrying, and the failure is logged once.
pub fn disk_reader() -> Box<dyn FnMut() -> Option<(u64, u64)>> {
    let mut query: Option<Query> = None;
    let mut failed = false;
    Box::new(move || {
        if query.is_none() && !failed {
            match Query::open(&[
                r"\PhysicalDisk(_Total)\Disk Read Bytes/sec",
                r"\PhysicalDisk(_Total)\Disk Write Bytes/sec",
            ]) {
                Ok(q) => query = Some(q),
                Err(e) => {
                    wr_core::warn!("Reading all disks isn't available, using the drive letters: {e}");
                    failed = true;
                }
            }
        }
        let q = query.as_mut()?;
        if !q.collect() {
            return None;
        }
        // `max(0)` guards the conversion: a negative raw count would otherwise wrap to a huge number.
        Some((q.raw(0)?.max(0) as u64, q.raw(1)?.max(0) as u64))
    })
}

/// Each GPU's memory use, 0.0-1.0, by name (the names match the GPU graph's).
///
/// The "GPU Adapter Memory" counters give bytes used per adapter instance; DXGI (see `gpu::adapters`)
/// gives each GPU's name, LUID and memory sizes. They are matched by LUID. A GPU's share is None when
/// its total memory is unknown. Returns an empty list if the counters can't be read.
pub fn gpu_memory_reader() -> Box<dyn FnMut() -> Vec<(String, Option<f64>)>> {
    let mut query: Option<Query> = None;
    let mut failed = false;
    // (name, LUID, dedicated bytes, shared bytes) for each GPU, refreshed every 60 s.
    let mut gpus: Vec<(String, Luid, f64, f64)> = Vec::new();
    let mut gpus_read: Option<Instant> = None;
    let mut log = OnceLog::default();
    Box::new(move || {
        if query.is_none() && !failed {
            match Query::open(&[r"\GPU Adapter Memory(*)\Dedicated Usage", r"\GPU Adapter Memory(*)\Shared Usage"]) {
                Ok(q) => query = Some(q),
                Err(e) => {
                    wr_core::warn!("GPU memory isn't available: {e}");
                    failed = true;
                }
            }
        }
        let Some(q) = query.as_mut() else { return Vec::new() };
        // List the GPUs on the first call and then once a minute, in case an external GPU comes or goes.
        if gpus_read.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            gpus_read = Some(Instant::now());
            match adapters() {
                Ok(found) => {
                    log.ok("Listing the GPUs for their memory");
                    let names = unique_names(&found.iter().map(|a| a.name.clone()).collect::<Vec<_>>());
                    gpus = names.into_iter().zip(found).map(|(n, a)| (n, a.luid, a.dedicated, a.shared)).collect();
                }
                Err(e) => log.fail(format!("Can't list the GPUs: {e}")),
            }
        }
        if !q.collect() {
            return Vec::new();
        }
        // A GPU can appear as several counter instances (one per physical part); add them up by LUID.
        let add_up = |values: Vec<(String, f64)>| {
            let mut per: HashMap<Luid, f64> = HashMap::new();
            for (instance, bytes) in values {
                if let Some(luid) = parse_adapter_luid(&instance) {
                    *per.entry(luid).or_insert(0.0) += bytes.max(0.0);
                }
            }
            per
        };
        let (dedicated, shared) = (add_up(q.all(0)), add_up(q.all(1)));
        // One entry per known GPU, in DXGI's order; a GPU with no counter data counts as 0 bytes used.
        gpus.iter()
            .map(|(name, luid, dedicated_total, shared_total)| {
                let used = |m: &HashMap<Luid, f64>| m.get(luid).copied().unwrap_or(0.0);
                (name.clone(), memory_share(used(&dedicated), used(&shared), *dedicated_total, *shared_total))
            })
            .collect()
    })
}
