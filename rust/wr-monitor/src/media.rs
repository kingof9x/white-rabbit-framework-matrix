//! What is playing, for the now-playing app: Windows' media controls (the ones the volume pop-up
//! shows), read through WinRT as `led_mon/now_playing.py` does, about once a second while the app shows.
//! The small visualizer under the cover isn't fed yet (no sound capture here), so it stays dark.

use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapBounds, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode,
};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::IRandomAccessStreamReference;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use wr_core::apps::now_playing::{Media, NowPlaying, COVER};

use crate::background::OnceLog;

fn is_playing(session: &Session) -> bool {
    session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).map(|s| s == Status::Playing).unwrap_or(false)
}

/// The session Windows calls current if it is playing, otherwise any that is playing (a paused
/// browser tab can be "current" while Spotify plays), otherwise the current one.
fn pick_session(manager: &Manager) -> Option<Session> {
    let current = manager.GetCurrentSession().ok();
    if let Some(c) = &current {
        if is_playing(c) {
            return current;
        }
    }
    let sessions: Vec<Session> = manager.GetSessions().map(|v| v.into_iter().collect()).unwrap_or_default();
    if let Some(s) = sessions.iter().find(|s| is_playing(s)) {
        return Some(s.clone());
    }
    current.or_else(|| sessions.into_iter().next())
}

/// (media without its cover, the app playing, the cover's picture if there is one).
fn read() -> windows::core::Result<(Media, String, Option<IRandomAccessStreamReference>)> {
    let manager = Manager::RequestAsync()?.join()?;
    let Some(session) = pick_session(&manager) else { return Ok((Media::default(), String::new(), None)) };
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let media = Media {
        title: props.Title().map(|s| s.to_string()).unwrap_or_default(),
        artist: props.Artist().map(|s| s.to_string()).unwrap_or_default(),
        playing: is_playing(&session),
        cover: None,
    };
    let app = session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default();
    Ok((media, app, props.Thumbnail().ok()))
}

/// numpy's percentile (linear between the two nearest values) of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let at = p / 100.0 * (sorted.len() - 1) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo as f64)
}

/// The cover as 18 x 18 grey values, cropped to a square and stretched so the darkest part is off
/// and the lightest full (as `cover_from_image`).
fn read_cover(picture: &IRandomAccessStreamReference) -> windows::core::Result<Option<Vec<f64>>> {
    let stream = picture.OpenReadAsync()?.join()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    if w == 0 || h == 0 {
        return Ok(None);
    }
    // Scale so the shorter side is 18, then cut the middle square
    let scale = COVER as f64 / w.min(h) as f64;
    let (sw, sh) = (((w as f64 * scale).round() as u32).max(18), ((h as f64 * scale).round() as u32).max(18));
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(sw)?;
    transform.SetScaledHeight(sh)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    transform.SetBounds(BitmapBounds { X: (sw - 18) / 2, Y: (sh - 18) / 2, Width: 18, Height: 18 })?;
    let pixels = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Ignore,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?
        .DetachPixelData()?;
    if pixels.len() < COVER * COVER * 4 {
        return Ok(None);
    }
    // PIL's "L": 299/587/114 parts of red, green and blue
    let grey: Vec<f64> = pixels
        .chunks_exact(4)
        .take(COVER * COVER)
        .map(|p| ((p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000) as f64 / 255.0)
        .collect();
    let mut sorted = grey.clone();
    sorted.sort_by(f64::total_cmp);
    let (low, high) = (percentile(&sorted, 5.0), percentile(&sorted, 98.0));
    if high - low < 0.05 {
        return Ok(Some(grey.iter().map(|v| v.clamp(0.0, 1.0)).collect()));
    }
    Ok(Some(grey.iter().map(|v| ((v - low) / (high - low)).clamp(0.0, 1.0)).collect()))
}

/// Asks Windows what is playing. Made and called on the now-playing thread.
pub fn reader() -> Box<dyn FnMut() -> NowPlaying> {
    // SAFETY: joins this thread to the multithreaded apartment, once, before any WinRT call.
    if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        wr_core::warn!("Now playing: can't start WinRT ({e})");
    }
    let mut last = NowPlaying::default();
    let mut cover_key: Option<(String, String)> = None;
    let mut seen: Option<(String, bool, String)> = None;
    let mut log = OnceLog::default();
    Box::new(move || {
        let (mut media, app, picture) = match read() {
            Ok(r) => {
                log.ok("Now playing: reading what is playing");
                r
            }
            Err(e) => {
                log.fail(format!("Now playing: could not ask Windows what is playing: {e}"));
                (Media::default(), String::new(), None)
            }
        };
        let key = (media.title.clone(), media.artist.clone());
        if let Some(picture) = picture {
            if cover_key.as_ref() == Some(&key) {
                media.cover = last.media.cover.clone();
            } else {
                media.cover = read_cover(&picture).unwrap_or_else(|e| {
                    wr_core::info!("Now playing: no cover: {e}");
                    None
                });
                cover_key = Some(key);
            }
        }
        let now_seen = (app.clone(), media.playing, media.song());
        if seen.as_ref() != Some(&now_seen) {
            if app.is_empty() && media.song().is_empty() {
                wr_core::info!("Now playing: Windows says nothing is playing");
            } else {
                let state = if media.playing { "playing" } else { "paused or stopped" };
                let app = if app.is_empty() { "an app" } else { app.as_str() };
                let song = media.song();
                wr_core::info!(
                    "Now playing: {app} is {state}: {}",
                    if song.is_empty() { "(no song name)" } else { &song }
                );
            }
            seen = Some(now_seen);
        }
        if media.cover.is_some() {
            last.last_cover = media.cover.clone();
        }
        last.media = media;
        last.clone()
    })
}
