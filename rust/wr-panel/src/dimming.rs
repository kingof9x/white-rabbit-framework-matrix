//! "Keep every shade when the panels are dim" (Effects tab): a port of `led_mon/dimming.py`.
//!
//! Normally the monitor dims the panels by drawing with smaller pixel values, so at low brightness many
//! shades become the same value. With this on, and White Rabbit firmware that has the GlobalCurrent
//! command (0x30), the pixel values are stretched to the full range and the panel is dimmed by lowering
//! the LED current instead. The light is the same; the shades are kept. Firmware without the command
//! doesn't answer the query, and is left as it was.
//!
//! Where it runs: `Panel::draw` in `panel.rs` calls `prepare` on the drawing thread, with the panel's
//! port already locked for the whole frame, so the current change and the frame's columns go out
//! together and the window's commands can't come in between. What has been learned about the panel
//! (does it support the command, which current was last sent) is kept in `PortState::dimming`, so it
//! lives exactly as long as the open port: a re-opened port is asked again.
//!
//! Example: the apps draw with a brightest value (`foreground`) of 80. Without this, a pixel of 40 is
//! sent as 40 and only values 0..80 can be shown (81 shades). With this, the current is set to 80 and
//! every pixel is multiplied by 255/80, so 40 is sent as 128 and all 256 values are usable.

use std::time::Duration;

use wr_core::{info, Grid, HEIGHT, WIDTH};

use crate::hub::PortState;
use crate::protocol::{self, ANSWER_LEN, FULL_CURRENT};

/// How long to wait for the firmware's answer to the query.
/// A missing answer is the normal "not supported" case, so this is kept short: it delays the first
/// frame on an older panel once, by this much.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(300);

/// What one open port's firmware can do, and the current it was last given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dimming {
    /// None until asked (once per open port).
    /// Some(true) if the firmware answered the GlobalCurrent query, Some(false) if it didn't.
    pub supported: Option<bool>,
    /// The LED current (0..=255) last sent to the panel, or None when unknown (never sent, or reset by the
    /// window holding the panel). Used to send the current only when it changes.
    pub current: Option<u8>,
}

/// The grid stretched so the brightest value `foreground` becomes 255:
/// `np.clip(np.rint(grid * (255.0 / foreground)), 0, 255)`.
/// `foreground` must be at least 1 (the caller clamps it); values above `foreground` are clipped to 255
/// and negative values to 0.
pub fn stretch(grid: &Grid, foreground: u8) -> Grid {
    // `round_ties_even` matches numpy's `rint` (2.5 -> 2, 3.5 -> 4), so the bytes equal Python's exactly.
    let scale = 255.0 / foreground as f64;
    let mut out = Grid::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            out.v[x][y] = (grid.v[x][y] as f64 * scale).round_ties_even().clamp(0.0, 255.0) as i32;
        }
    }
    out
}

/// True if the panel's firmware answers the GlobalCurrent query. Asked once per open port, and not
/// while the window holds the panel (its reply would go to the window).
/// The caller must have the port locked (it passes the locked state). Side effects on the first call:
/// empties the port's input, writes the query, waits up to `QUERY_TIMEOUT` and logs the result. Any
/// error (failed write or read) counts as "not supported" and is remembered; a failed write also marks
/// the port broken, so the next frame's write fails and the drawing thread opens the port again.
pub fn supports(port: &mut PortState) -> bool {
    if port.dimming.supported.is_none() && !port.is_held() {
        // Old bytes in the input are thrown away first, so only an answer to this query counts. The query
        // is written with `write_control` because `write` would drop it if the panel became held, but the
        // `is_held` check above already ruled that out under the same lock.
        let answer = port
            .clear_input()
            .and_then(|_| port.write_control(&protocol::global_current_query()))
            .and_then(|_| port.read(ANSWER_LEN, QUERY_TIMEOUT));
        let supported = matches!(&answer, Ok(a) if a.len() == ANSWER_LEN);
        port.dimming.supported = Some(supported);
        info!(
            "Panel {}: dimming by LED current {}",
            port.name(),
            if supported { "supported" } else { "not supported (older firmware)" }
        );
    }
    port.dimming.supported == Some(true)
}

/// Sends the LED current only if it differs from the last one sent (one 4-byte command), and records
/// it. Uses `write`, so it is dropped while held; `prepare` returns early then, so nothing wrong is
/// recorded.
fn send_current(port: &mut PortState, value: u8) -> std::io::Result<()> {
    if port.dimming.current != Some(value) {
        port.write(&protocol::global_current(value))?;
        port.dimming.current = Some(value);
    }
    Ok(())
}

/// The grid to send. With dimming by current on (`enabled`) and supported, stretched to the full
/// range, and the panel's current set to match the brightness (`foreground`). Otherwise the grid as
/// it was, and the panel's current back to full if it was lowered.
/// `foreground` is the brightest value the apps draw with now (follows the screen brightness, from
/// `Options::foreground`). An error is a failed write to the panel; the drawing thread then treats
/// the panel as lost.
pub fn prepare(port: &mut PortState, grid: &Grid, enabled: bool, foreground: i32) -> std::io::Result<Grid> {
    if port.is_held() {
        // The window has the panel: the monitor's writes are dropped, so nothing may be recorded as
        // sent either. (Recording a current that never reached the panel left it at full current with
        // stretched pixels, too bright, after the window gave it back.)
        return Ok(grid.clone());
    }
    // Turned off (or brightness 0): only if the current was lowered earlier is it put back to full,
    // so panels that were never dimmed this way get no extra command.
    if !enabled || foreground <= 0 {
        if port.dimming.current.is_some_and(|c| c != FULL_CURRENT) {
            send_current(port, FULL_CURRENT)?;
        }
        return Ok(grid.clone());
    }
    if !supports(port) {
        return Ok(grid.clone());
    }
    // The current takes the place of the brightness: current 80 with pixels stretched by 255/80 gives
    // about the same light as current 255 with pixels up to 80.
    let foreground = foreground.clamp(1, 255) as u8;
    send_current(port, foreground)?;
    Ok(stretch(grid, foreground))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::fake::*;
    use crate::hub::Hub;
    use std::sync::Arc;

    /// Proves the stretch rounds and clips exactly like the Python/numpy version, including half to even.
    #[test]
    fn stretch_like_numpy() {
        let mut g = Grid::new();
        g.set(0, 0, 160);
        g.set(1, 0, 35);
        g.set(2, 0, 1);
        g.set(3, 0, 300);
        let s = stretch(&g, 160);
        // 35 * 255 / 160 = 55.78 -> 56; 1 * 1.59375 -> 2; 300 -> clipped
        assert_eq!([s.get(0, 0), s.get(1, 0), s.get(2, 0), s.get(3, 0)], [255, 56, 2, 255]);
        // round half to even: 2 * 255 / 204 = 2.5 -> 2
        let mut h = Grid::new();
        h.set(0, 0, 2);
        assert_eq!(stretch(&h, 204).get(0, 0), 2);
    }

    /// Proves the whole cycle: new firmware gets the current and stretched pixels, an unchanged current is
    /// not sent again, turning it off restores full current, a hold resets the current and nothing is
    /// recorded while held, the current is sent again after release, and old firmware is left alone.
    #[test]
    fn dims_by_current_only_when_the_firmware_answers() {
        let fake = Arc::new(Fake::default());
        fake.answers.lock().unwrap().push("COM4".into());
        let hub = Hub::new(Arc::new(FakeSystem(fake.clone())));
        let mut g = Grid::new();
        g.set(0, 0, 80);

        let new_fw = hub.open("COM4").unwrap();
        let out = prepare(&mut new_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(out.get(0, 0), 255);
        assert_eq!(new_fw.lock().dimming.current, Some(80));
        // the same brightness again isn't sent again
        let before = fake.written.lock().unwrap().len();
        prepare(&mut new_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(fake.written.lock().unwrap().len(), before);
        // turned off: full current again, values as they were
        let out = prepare(&mut new_fw.lock(), &g, false, 80).unwrap();
        assert_eq!(out.get(0, 0), 80);
        assert_eq!(fake.written.lock().unwrap().last().unwrap().1, protocol::global_current(255));

        // The window borrows the panel (its current goes back to full) and gives it back: the dimmed
        // current is sent again, not taken as already sent
        hub.hold(&["COM4".into()], None);
        assert_eq!(fake.written.lock().unwrap().last().unwrap().1, protocol::global_current(255));
        prepare(&mut new_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(new_fw.lock().dimming.current, None, "nothing reaches a held panel");
        hub.release(None);
        prepare(&mut new_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(fake.written.lock().unwrap().last().unwrap().1, protocol::global_current(80));

        let old_fw = hub.open("COM5").unwrap();
        let out = prepare(&mut old_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(out.get(0, 0), 80);
        assert_eq!(old_fw.lock().dimming.supported, Some(false));
    }
}
