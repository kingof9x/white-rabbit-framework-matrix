//! GPU use from Windows' "GPU Engine" counters, added up the way Task Manager does: the
//! `busiest_engines` part of `led_mon/gpu.py`.
//!
//! Where this sits: `wr-monitor`'s `gpu.rs` and `counters.rs` read Windows performance counters (PDH)
//! on a background thread about once a second. Windows reports one counter per process per GPU
//! engine, named like `pid_1234_luid_0x..._0x..._phys_0_eng_0_engtype_3D`. The functions here only
//! take those names and numbers apart and add them up, so they can be tested without Windows; the
//! result (one number per GPU) feeds the gpu-graph app's history.
//!
//! How Task Manager counts: for each GPU, the use of every process on the same engine type (3D,
//! Copy, VideoDecode, ...) is added up, and the busiest engine type is the GPU's use. Engine types
//! are not added to each other, because they run in parallel on separate hardware.

use std::collections::HashMap;

/// An adapter's LUID as (high part, low part), as it appears in the counter names.
/// A LUID ("locally unique identifier") is the number Windows gives each graphics adapter until the
/// next restart. It is used as the key that ties the per-process counters to one GPU.
pub type Luid = (u32, u32);

/// The LUID and engine type in a counter instance name such as
/// `pid_1234_luid_0x00000000_0x0000D1F2_phys_0_eng_0_engtype_3D`.
/// Returns None for any name that does not have this shape (for example the "_Total" instance), so
/// callers can just skip it. Matching ignores upper/lower case. The returned engine type is lower
/// case (for example "3d") and keeps anything after "engtype_".
pub fn parse_instance(instance: &str) -> Option<(Luid, String)> {
    let lower = instance.to_ascii_lowercase();
    let start = lower.find("luid_0x")? + "luid_0x".len();
    let rest = &lower[start..];
    // After "luid_0x" come the high part, "_0x", the low part, then "_phys_" and the
    // physical adapter number. Both LUID parts are hexadecimal.
    let (high, rest) = rest.split_once("_0x")?;
    let (low, rest) = rest.split_once("_phys_")?;
    // The physical adapter number must be at least one digit, as Python's regular expression
    // required; otherwise the name is not a GPU engine counter.
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let engine_at = rest.find("engtype_")? + "engtype_".len();
    let high = u32::from_str_radix(high, 16).ok()?;
    let low = u32::from_str_radix(low, 16).ok()?;
    Some(((high, low), rest[engine_at..].to_string()))
}

/// The LUID in a "GPU Adapter Memory" counter instance name such as
/// `luid_0x00000000_0x0000D1F2_phys_0`.
/// Used by the GPU memory reading to match an adapter's memory counters to its LUID. None when the
/// name has no LUID (for example "Total") or the hex digits do not parse.
pub fn parse_adapter_luid(instance: &str) -> Option<Luid> {
    let lower = instance.to_ascii_lowercase();
    let start = lower.find("luid_0x")? + "luid_0x".len();
    let (high, rest) = lower[start..].split_once("_0x")?;
    let low = rest.split('_').next()?;
    Some((u32::from_str_radix(high, 16).ok()?, u32::from_str_radix(low, 16).ok()?))
}

/// How much of a GPU's memory is in use, 0.0-1.0, the way Task Manager shows it: a graphics card
/// with its own memory (1 GB or more) counts that; a built-in GPU, which borrows the computer's
/// memory, counts its own and the borrowed memory together. Bytes in and out.
/// Returns None when the total is 0 (nothing known about the adapter's memory), so the caller can
/// show no meter rather than a wrong one. The result is clamped to 0.0-1.0 because the counters are
/// read at slightly different moments and can briefly disagree.
pub fn memory_share(dedicated_used: f64, shared_used: f64, dedicated_total: f64, shared_total: f64) -> Option<f64> {
    // Built-in GPUs report a small "dedicated" memory (often 128-512 MB carved out by the BIOS);
    // 1 GB is the line between that and a real graphics card's own memory.
    const ONE_GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let (used, total) = if dedicated_total >= ONE_GB {
        (dedicated_used, dedicated_total)
    } else {
        (dedicated_used + shared_used, dedicated_total + shared_total)
    };
    (total > 0.0).then(|| (used / total).clamp(0.0, 1.0))
}

/// Each GPU's use in percent: for every engine type the use of all processes added up, and the
/// busiest engine type counts (at most 100).
/// `values` is (counter instance name, percent) pairs as read from Windows. Names that are not GPU
/// engine counters are ignored, negative values count as 0 (a guard against odd counter readings),
/// and a GPU's total is capped at 100 since several processes' shares can add up to a little more.
pub fn busiest_engines(values: &[(String, f64)]) -> HashMap<Luid, f64> {
    // First pass: add up all processes per (GPU, engine type).
    let mut per_engine: HashMap<(Luid, String), f64> = HashMap::new();
    for (instance, value) in values {
        if let Some(key) = parse_instance(instance) {
            *per_engine.entry(key).or_insert(0.0) += value.max(0.0);
        }
    }
    // Second pass: per GPU keep the busiest engine type.
    let mut result: HashMap<Luid, f64> = HashMap::new();
    for ((luid, _), total) in per_engine {
        let entry = result.entry(luid).or_insert(0.0);
        *entry = entry.max(total).min(100.0);
    }
    result
}

/// Numbers repeated names (two identical GPUs) so each one can be picked: "X", "X #2".
/// The names are what the user picks in the gpu-graph app's settings, so two of the same card would
/// otherwise be impossible to tell apart. The first one keeps its plain name; the order is that of
/// the list given.
pub fn unique_names(names: &[String]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    names
        .iter()
        .map(|name| {
            let n = seen.entry(name).or_insert(0);
            *n += 1;
            if *n == 1 {
                name.clone()
            } else {
                format!("{name} #{n}")
            }
        })
        .collect()
}

/// The named GPU's use (the first GPU when the name is blank or not found), or None without GPUs.
/// `rows` are (unique GPU name, use) pairs; the use is None when it could not be read. `gpu` is the
/// name stored in the app's settings. Falling back to the first GPU means a layout made on another
/// computer (with a different GPU name) still shows something.
pub fn pick(rows: &[(String, Option<f64>)], gpu: &str) -> Option<f64> {
    rows.iter().find(|(name, _)| name == gpu).or(rows.first()).and_then(|(_, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two processes on the same 3D engine add up (55), but the video engine is busier (70) and wins;
    /// a GPU whose total goes over 100 is capped at 100; a name that is not a GPU counter is ignored.
    #[test]
    fn adds_processes_and_takes_the_busiest_engine() {
        let values = vec![
            ("pid_1_luid_0x00000000_0x0000D1F2_phys_0_eng_0_engtype_3D".to_string(), 30.0),
            ("pid_2_luid_0x00000000_0x0000D1F2_phys_0_eng_0_engtype_3D".to_string(), 25.0),
            ("pid_2_luid_0x00000000_0x0000D1F2_phys_0_eng_3_engtype_VideoDecode".to_string(), 70.0),
            ("pid_3_luid_0x00000001_0x0000AAAA_phys_0_eng_0_engtype_Copy".to_string(), 80.0),
            ("pid_3_luid_0x00000001_0x0000AAAA_phys_0_eng_0_engtype_Copy".to_string(), 80.0),
            ("not a gpu".to_string(), 99.0),
        ];
        let result = busiest_engines(&values);
        assert_eq!(result[&(0, 0xd1f2)], 70.0);
        assert_eq!(result[&(1, 0xaaaa)], 100.0);
        assert_eq!(result.len(), 2);
    }

    /// LUIDs are read from adapter memory counter names, and memory use is worked out differently for a
    /// graphics card with its own memory and for a built-in GPU.
    #[test]
    fn adapter_memory() {
        assert_eq!(parse_adapter_luid("luid_0x00000000_0x0000D1F2_phys_0"), Some((0, 0xd1f2)));
        assert_eq!(parse_adapter_luid("Total"), None);
        let gb = 1024.0 * 1024.0 * 1024.0;
        // A graphics card with 8 GB: only its own memory counts
        assert_eq!(memory_share(2.0 * gb, 1.0 * gb, 8.0 * gb, 16.0 * gb), Some(0.25));
        // A built-in GPU with 512 MB of its own: own and borrowed memory together
        assert_eq!(memory_share(0.5 * gb, 3.5 * gb, 0.5 * gb, 15.5 * gb), Some(0.25));
        assert_eq!(memory_share(0.0, 0.0, 0.0, 0.0), None);
    }

    /// Repeated GPU names get "#2"; picking by name works, and a blank name or no match falls back to
    /// the first GPU (or None when there are no GPUs).
    #[test]
    fn names_and_picking() {
        let names = unique_names(&["A".into(), "A".into(), "B".into()]);
        assert_eq!(names, vec!["A", "A #2", "B"]);
        let rows = vec![("A".to_string(), Some(0.5)), ("B".to_string(), Some(0.25))];
        assert_eq!(pick(&rows, "B"), Some(0.25));
        assert_eq!(pick(&rows, ""), Some(0.5));
        assert_eq!(pick(&[], ""), None);
    }
}
