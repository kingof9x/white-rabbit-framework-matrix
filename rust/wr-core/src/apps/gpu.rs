//! GPU use from Windows' "GPU Engine" counters, added up the way Task Manager does: the
//! `busiest_engines` part of `led_mon/gpu.py`.

use std::collections::HashMap;

/// An adapter's LUID as (high part, low part), as it appears in the counter names.
pub type Luid = (u32, u32);

/// The LUID and engine type in a counter instance name such as
/// `pid_1234_luid_0x00000000_0x0000D1F2_phys_0_eng_0_engtype_3D`.
pub fn parse_instance(instance: &str) -> Option<(Luid, String)> {
    let lower = instance.to_ascii_lowercase();
    let start = lower.find("luid_0x")? + "luid_0x".len();
    let rest = &lower[start..];
    let (high, rest) = rest.split_once("_0x")?;
    let (low, rest) = rest.split_once("_phys_")?;
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
pub fn memory_share(dedicated_used: f64, shared_used: f64, dedicated_total: f64, shared_total: f64) -> Option<f64> {
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
pub fn busiest_engines(values: &[(String, f64)]) -> HashMap<Luid, f64> {
    let mut per_engine: HashMap<(Luid, String), f64> = HashMap::new();
    for (instance, value) in values {
        if let Some(key) = parse_instance(instance) {
            *per_engine.entry(key).or_insert(0.0) += value.max(0.0);
        }
    }
    let mut result: HashMap<Luid, f64> = HashMap::new();
    for ((luid, _), total) in per_engine {
        let entry = result.entry(luid).or_insert(0.0);
        *entry = entry.max(total).min(100.0);
    }
    result
}

/// Numbers repeated names (two identical GPUs) so each one can be picked: "X", "X #2".
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
pub fn pick(rows: &[(String, Option<f64>)], gpu: &str) -> Option<f64> {
    rows.iter().find(|(name, _)| name == gpu).or(rows.first()).and_then(|(_, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

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
