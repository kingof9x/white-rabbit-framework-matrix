//! Where this computer is, from the Windows Location service (Settings > Privacy & security >
//! Location), as `led_mon/windows_location.py` finds it. The weather app uses it when its
//! "Location" setting is "Windows Location". When the service is off, White Rabbit isn't allowed
//! to use it, or no position is found, the weather falls back to the zip code or coordinates
//! typed in, or to the internet address.

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

#[derive(Clone, Debug, PartialEq)]
pub enum Unavailable {
    Denied,
    Off,
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
    position: Option<((f64, f64), Instant)>,
    failure: Option<(Unavailable, Instant)>,
}

impl Cache {
    /// The position, asking `lookup` only when the remembered one is old and no failure is recent.
    pub fn get(
        &mut self,
        now: Instant,
        lookup: impl FnOnce() -> Result<(f64, f64), Unavailable>,
    ) -> Result<(f64, f64), Unavailable> {
        if let Some((pos, at)) = self.position {
            if now.duration_since(at) < POSITION_MAX_AGE {
                return Ok(pos);
            }
        }
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

#[cfg(windows)]
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// (latitude, longitude) from Windows Location, or why it can't be used.
#[cfg(windows)]
pub fn position() -> Result<(f64, f64), Unavailable> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.get_or_insert_with(Cache::default).get(Instant::now(), lookup_with_timeout)
}

/// Asks Windows on a thread of its own, giving up after LOOKUP_TIMEOUT.
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

#[cfg(windows)]
fn lookup() -> Result<(f64, f64), Unavailable> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionStatus};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_is_reused_for_a_while() {
        let mut cache = Cache::default();
        let start = Instant::now();
        assert_eq!(cache.get(start, || Ok((51.5, -0.12))), Ok((51.5, -0.12)));
        assert_eq!(cache.get(start + Duration::from_secs(60), || panic!("asked again too soon")), Ok((51.5, -0.12)));
        assert_eq!(cache.get(start + POSITION_MAX_AGE, || Ok((48.8, 2.35))), Ok((48.8, 2.35)));
    }

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
