//! "Keep every shade when the panels are dim" (Effects tab): a port of `led_mon/dimming.py`.
//!
//! Normally the monitor dims the panels by drawing with smaller pixel values, so at low brightness many
//! shades become the same value. With this on, and White Rabbit firmware that has the GlobalCurrent
//! command (0x30), the pixel values are stretched to the full range and the panel is dimmed by lowering
//! the LED current instead. The light is the same; the shades are kept. Firmware without the command
//! doesn't answer the query, and is left as it was.

use std::time::Duration;

use wr_core::{info, Grid, HEIGHT, WIDTH};

use crate::hub::PortState;
use crate::protocol::{self, ANSWER_LEN, FULL_CURRENT};

/// How long to wait for the firmware's answer to the query.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(300);

/// What one open port's firmware can do, and the current it was last given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dimming {
    /// None until asked (once per open port).
    pub supported: Option<bool>,
    pub current: Option<u8>,
}

/// The grid stretched so the brightest value `foreground` becomes 255:
/// `np.clip(np.rint(grid * (255.0 / foreground)), 0, 255)`.
pub fn stretch(grid: &Grid, foreground: u8) -> Grid {
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
pub fn supports(port: &mut PortState) -> bool {
    if port.dimming.supported.is_none() && !port.is_held() {
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
pub fn prepare(port: &mut PortState, grid: &Grid, enabled: bool, foreground: i32) -> std::io::Result<Grid> {
    if !enabled || foreground <= 0 {
        if port.dimming.current.is_some_and(|c| c != FULL_CURRENT) {
            send_current(port, FULL_CURRENT)?;
        }
        return Ok(grid.clone());
    }
    if !supports(port) {
        return Ok(grid.clone());
    }
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

        let old_fw = hub.open("COM5").unwrap();
        let out = prepare(&mut old_fw.lock(), &g, true, 80).unwrap();
        assert_eq!(out.get(0, 0), 80);
        assert_eq!(old_fw.lock().dimming.supported, Some(false));
    }
}
