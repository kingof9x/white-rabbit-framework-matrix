//! Where this computer is, from the Windows Location service (Settings > Privacy & security >
//! Location), as `led_mon/windows_location.py` finds it. The weather app uses it when its
//! "Location" setting is "Windows Location". When the service is off, White Rabbit isn't allowed
//! to use it, or no position is found, the weather falls back to the zip code or coordinates
//! typed in, or to the internet address.
//!
//! `position` is called from the weather reading thread (`weather.rs`). Asking Windows can take many
//! seconds, so answers are cached (`Cache`), and each lookup runs on a short-lived
//! `windows-location` thread with a 15 s time limit. The cache logic is platform-neutral and tested.

#[cfg(windows)]
use std::sync::mpsc;
#[cfg(windows)]
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A position found is reused for this long (the weather refreshes every 30 seconds).
const POSITION_MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// After a failure, wait this long before asking Windows again.
const FAILURE_RETRY: Duration = Duration::from_secs(2 * 60);
/// Longest wait for Windows to find a position.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Why Windows Location can't give a position. The `Display` text is shown to the user and logged.
#[derive(Clone, Debug, PartialEq)]
pub enum Unavailable {
    /// The user (or a policy) hasn't allowed desktop apps to use the location.
    Denied,
    /// Location services are switched off for the whole PC.
    Off,
    /// Allowed and on, but no position was found (or Windows timed out); holds Windows' detail text.
    NoPosition(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::Denied => write!(
                f,
                "White Rabbit isn't allowed to use your location. In Windows Settings > Privacy & security > \
                 Location, turn on Location services and \"Let desktop apps access your location\"."
            ),
            Unavailable::Off => {
                write!(
                    f,
                    "Location services are off in Windows. Turn them on in Settings > Privacy & security > Location."
                )
            }
            Unavailable::NoPosition(detail) => {
                write!(f, "Windows couldn't find where this computer is right now (no Wi-Fi or GPS fix yet)")?;
                if detail.is_empty() {
                    Ok(())
                } else {
                    write!(f, " ({detail})")
                }
            }
        }
    }
}

/// Remembers the last position and the last failure, so Windows isn't asked every 30 seconds.
#[derive(Default)]
pub struct Cache {
    /// The last position found, (latitude, longitude) in degrees, and when.
    position: Option<((f64, f64), Instant)>,
    /// The last failure and when; cleared by the next success.
    failure: Option<(Unavailable, Instant)>,
}

impl Cache {
    /// The position, asking `lookup` only when the remembered one is old and no failure is recent.
    ///
    /// `now` is passed in (instead of read from the clock) so the tests can move time forward.
    /// Logs a success only when it follows a failure or is the first one, and a failure only when its
    /// kind changed, to keep the 30-second weather refresh from filling the log.
    pub fn get(
        &mut self,
        now: Instant,
        lookup: impl FnOnce() -> Result<(f64, f64), Unavailable>,
    ) -> Result<(f64, f64), Unavailable> {
        // A fresh enough position wins, without asking Windows.
        if let Some((pos, at)) = self.position {
            if now.duration_since(at) < POSITION_MAX_AGE {
                return Ok(pos);
            }
        }
        // A recent failure is repeated without asking Windows again.
        if let Some((failure, at)) = &self.failure {
            if now.duration_since(*at) < FAILURE_RETRY {
                return Err(failure.clone());
            }
        }
        match lookup() {
            Ok(pos) => {
                if self.position.is_none() || self.failure.is_some() {
                    wr_core::info!("Windows Location: {:.3}, {:.3}", pos.0, pos.1);
                }
                self.position = Some((pos, now));
                self.failure = None;
                Ok(pos)
            }
            Err(failure) => {
                if self
                    .failure
                    .as_ref()
                    // `discriminant` compares only the kind (Denied/Off/NoPosition), not the detail text.
                    .is_none_or(|(old, _)| std::mem::discriminant(old) != std::mem::discriminant(&failure))
                {
                    wr_core::warn!("Windows Location not used: {failure}");
                }
                self.failure = Some((failure.clone(), now));
                // A recent position is still better than none (e.g. Wi-Fi dropped for a moment)
                if let (Unavailable::NoPosition(_), Some((pos, at))) = (&failure, self.position) {
                    if now.duration_since(at) < POSITION_MAX_AGE * 4 {
                        return Ok(pos);
                    }
                }
                Err(failure)
            }
        }
    }
}

/// The process-wide cache. An `Option` because a `static` must be built without calling `default()`.
#[cfg(windows)]
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// (latitude, longitude) from Windows Location, or why it can't be used.
///
/// Holds the cache lock while Windows is asked (up to 15 s); only the weather thread calls this, so
/// no one else waits.
#[cfg(windows)]
pub fn position() -> Result<(f64, f64), Unavailable> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.get_or_insert_with(Cache::default).get(Instant::now(), lookup_with_timeout)
}

/// Asks Windows on a thread of its own, giving up after LOOKUP_TIMEOUT.
///
/// On a timeout the lookup thread is left to finish on its own; its answer goes nowhere because the
/// receiving end has been dropped (`send` then fails, which is ignored).
#[cfg(windows)]
fn lookup_with_timeout() -> Result<(f64, f64), Unavailable> {
    let (send, receive) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("windows-location".into()).spawn(move || {
        let _ = send.send(lookup());
    });
    if let Err(e) = spawned {
        return Err(Unavailable::NoPosition(e.to_string()));
    }
    receive
        .recv_timeout(LOOKUP_TIMEOUT)
        .unwrap_or_else(|_| Err(Unavailable::NoPosition(format!("no answer in {} seconds", LOOKUP_TIMEOUT.as_secs()))))
}

/// The actual WinRT calls: check access, check that location is on, then ask for a position.
/// Blocks until Windows answers. An "access denied" error code at any step means `Denied`.
#[cfg(windows)]
fn lookup() -> Result<(f64, f64), Unavailable> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionStatus};
    // The HRESULT for "access denied" (Win32 error 5 wrapped as an HRESULT).
    const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
    let denied_or = |e: windows::core::Error| {
        if e.code().0 == E_ACCESSDENIED {
            Unavailable::Denied
        } else {
            Unavailable::NoPosition(e.message().to_string())
        }
    };
    let access = Geolocator::RequestAccessAsync().and_then(|op| op.join()).map_err(denied_or)?;
    if access != GeolocationAccessStatus::Allowed {
        // Unspecified means Windows hasn't decided (the user hasn't answered); treat it like denied
        return Err(Unavailable::Denied);
    }
    let locator = Geolocator::new().map_err(denied_or)?;
    if locator.LocationStatus().map_err(denied_or)? == PositionStatus::Disabled {
        return Err(Unavailable::Off);
    }
    let found = locator.GetGeopositionAsync().and_then(|op| op.join()).map_err(denied_or)?;
    let point = found.Coordinate().and_then(|c| c.Point()).and_then(|p| p.Position()).map_err(denied_or)?;
    Ok((point.Latitude, point.Longitude))
}

/// Tests for the location cache's timing rules.
#[cfg(test)]
mod tests {
    use super::*;

    /// A found position is reused without asking for 15 minutes, then asked for again.
    #[test]
    fn a_position_is_reused_for_a_while() {
        let mut cache = Cache::default();
        let start = Instant::now();
        assert_eq!(cache.get(start, || Ok((51.5, -0.12))), Ok((51.5, -0.12)));
        assert_eq!(cache.get(start + Duration::from_secs(60), || panic!("asked again too soon")), Ok((51.5, -0.12)));
        assert_eq!(cache.get(start + POSITION_MAX_AGE, || Ok((48.8, 2.35))), Ok((48.8, 2.35)));
    }

    /// After a failure Windows isn't asked again for 2 minutes.
    #[test]
    fn a_failure_waits_before_asking_again() {
        let mut cache = Cache::default();
        let start = Instant::now();
        assert_eq!(cache.get(start, || Err(Unavailable::Denied)), Err(Unavailable::Denied));
        assert_eq!(
            cache.get(start + Duration::from_secs(30), || panic!("asked again too soon")),
            Err(Unavailable::Denied)
        );
        assert_eq!(cache.get(start + FAILURE_RETRY, || Ok((1.0, 2.0))), Ok((1.0, 2.0)));
    }

    /// A lost fix keeps using the last position (up to an hour old), but being denied does not.
    #[test]
    fn a_recent_position_covers_a_lost_fix() {
        let mut cache = Cache::default();
        let start = Instant::now();
        cache.get(start, || Ok((1.0, 2.0))).unwrap();
        let later = start + POSITION_MAX_AGE;
        assert_eq!(cache.get(later, || Err(Unavailable::NoPosition(String::new()))), Ok((1.0, 2.0)));
        // Being denied is not covered: the user turned it off
        let mut cache = Cache::default();
        cache.get(start, || Ok((1.0, 2.0))).unwrap();
        assert_eq!(cache.get(later, || Err(Unavailable::Denied)), Err(Unavailable::Denied));
    }
}
