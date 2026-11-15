//! The whole drawing path without Windows: main loop -> queues -> drawing threads -> panel bytes.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wr_core::effects::EffectsSettings;
use wr_core::queue::FrameQueue;
use wr_panel::hub::{Port, PortSystem};
use wr_panel::overlay::EffectsOverlay;
use wr_panel::panel::{Panel, Panels};
use wr_panel::ports::{Pins, PortInfo};
use wr_panel::protocol;

use crate::monitor::{LayoutSource, Monitor, Shared};

type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct FakePort(String, Log);
impl Port for FakePort {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.1.lock().unwrap().push((self.0.clone(), data.to_vec()));
        Ok(())
    }
    fn read(&mut self, _: usize, _: Duration) -> io::Result<Vec<u8>> {
        Ok(vec![])
    }
    fn clear_input(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Two(Log);
impl PortSystem for Two {
    fn list(&self) -> Vec<PortInfo> {
        ["COM4", "COM5"]
            .iter()
            .zip(["1-3.2:x.0", "1-3.3:x.0"])
            .map(|(d, l)| PortInfo {
                device: d.to_string(),
                vid: Some(protocol::LED_MATRIX_VID),
                pid: Some(protocol::LED_MATRIX_PID),
                location: Some(l.into()),
                description: String::new(),
            })
            .collect()
    }
    fn open(&self, name: &str) -> io::Result<Box<dyn Port>> {
        Ok(Box::new(FakePort(name.into(), self.0.clone())))
    }
}

struct Fixed;
impl wr_core::render::Readings for Fixed {
    fn cpu_cores(&mut self) -> Vec<f64> {
        vec![0.5; 8]
    }
    fn cpu_history(&mut self) -> Vec<Option<f64>> {
        vec![Some(0.5); 20]
    }
    fn memory(&mut self) -> f64 {
        0.5
    }
    fn battery(&mut self) -> Option<(f64, bool)> {
        Some((0.5, false))
    }
    fn clock(&mut self, _: &wr_core::layout::App) -> (String, bool) {
        ("1234".into(), false)
    }
    fn now(&mut self) -> f64 {
        0.0
    }
}

#[test]
fn the_default_layout_lights_both_panels() {
    let fake = Arc::new(Two::default());
    let pins: Arc<Mutex<Pins>> = Arc::default();
    let panels = Panels::new(fake.clone(), pins.clone());
    let fx = EffectsOverlay::new(EffectsSettings::from_json(&serde_json::Value::Null));
    panels.set_overlay(fx.clone());
    let shared = Arc::new(Shared::default());
    *shared.effects.lock().unwrap() = Some(fx);
    *shared.panels.lock().unwrap() = Some(panels.clone());
    let queues = [Arc::new(FrameQueue::new()), Arc::new(FrameQueue::new())];
    let mut sides = [Panel::new(0, panels.clone()), Panel::new(1, panels.clone())];
    for p in sides.iter_mut() {
        p.connect(true);
    }
    let mut m = Monitor::new(
        shared,
        queues.clone(),
        pins,
        Box::new(Fixed),
        Box::new(|| 1.0),
        LayoutSource { path: None, modified: None },
    );
    for _ in 0..30 {
        m.step();
        for (q, p) in queues.iter().zip(sides.iter_mut()) {
            while let Some(item) = q.get(Duration::ZERO) {
                p.handle(item);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = fake.0.lock().unwrap();
    for port in ["COM4", "COM5"] {
        let lit = log
            .iter()
            .filter(|(n, b)| n == port && b.windows(3).any(|w| w == [0x32, 0xAC, protocol::STAGE_COL]))
            .count();
        let all: Vec<_> = log.iter().filter(|(n, _)| n == port).map(|(_, b)| b[..b.len().min(6)].to_vec()).collect();
        assert!(lit > 0, "{port} got no lit columns: {all:?}");
    }
    // The firmware reads one command per USB transfer: never two commands in one write
    for (port, bytes) in log.iter() {
        let starts = bytes.windows(2).filter(|w| *w == protocol::MAGIC).count();
        assert!(bytes.starts_with(&protocol::MAGIC) && starts == 1, "{port}: joined commands {bytes:?}");
    }
}
