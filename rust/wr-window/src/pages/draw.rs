//! Draw (DrawTab in `draw_tab.py`): draw on both panels' 9 x 34 LEDs in shades of grey, each panel on
//! its own or both as one picture (when the panels sit next to each other), make an animation out of
//! several frames, import a PNG or GIF, and save drawings for the Picture app.
//!
//! The idea of a drawing editor comes from Deo's LED Matrix Manager (MIT licence); this is our own code.
//!
//! How the Slint callbacks (`DrawPage` in `ui/pages/draw.slint`) map to Rust, all connected in `wire`:
//! `press(x, y, right)`, `drag(x, y)` and `release()` are the mouse on the editor, already turned into
//! LED cells by the Slint side (`press`, `drag`, `release` here); the whole-frame buttons (`clear`,
//! `fill`, `swap`, `mirror`, `flip`, `shift`) go through `transform`; the frame strip (`add-blank`,
//! `duplicate`, `delete-frame`, `move-frame`) through `change_frames`; `undo` / `redo` through `restore`;
//! `play`, `import`, `live-changed`, `save`, `open`, `new-drawing`, `delete-drawing` and `show-folder`
//! are handled inline in `wire`. All of these run on the UI thread: the drawing is tiny (2 x 9 x 34
//! bytes per frame), so editing and drawing the editor picture are instant.
//!
//! Worker threads: only sending to the panels ("Show on panels", the `live` switch). Each change hands
//! the newest frame to `push`, which keeps at most one sender thread going; a drag makes many frames a
//! second, and serial writes are slower than that, so in-between frames are dropped rather than queued.
//!
//! What is saved: drawings, as JSON files in the drawings folder next to settings.json (`drawings.rs`),
//! written by Save and removed by Delete. The left half goes in "frames", the right half in "right"
//! only when something is drawn there. Nothing about the editor itself (tool, shade) is saved.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use wr_core::grid::{HEIGHT, WIDTH};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::drawings::{self, Drawing, Pic, Wide, BOTH, MAX_FRAMES};
use crate::panels::Panel;
use crate::{dialogs, AppWindow, DrawPage, EffectsPage};

/// Size of one LED in the editor picture, in window pixels (logical pixels, before `SCALE`). The Slint
/// side gets the same number so it can turn a mouse position into a cell.
const LED: usize = 12;
/// Window pixels of dark border around the panels in the editor picture.
const MARGIN: usize = 6;
/// Window pixels between the two panels in the editor
const GAP: usize = 16;
/// Pixels per window pixel in the editor's picture, so it stays sharp on scaled screens
const SCALE: usize = 2;
/// How many changes Undo can go back. Older ones are dropped.
const UNDO_STEPS: usize = 50;

/// An undo step: (all frames, the frame being edited, the frame delay in ms).
type Snapshot = (Vec<Wide>, usize, i64);

/// The editor's state. Lives on the UI thread only (see `page_state`).
struct State {
    /// The drawing being edited: frames of both panels side by side (18 x 34) and the delay between them.
    drawing: Pic,
    /// The panel the whole-frame tools work on when the panels are drawn on one at a time (0 left)
    active: usize,
    /// Columns the brush stroke stays in
    stroke_cols: (usize, usize),
    /// Which frame is being edited (index into `drawing.frames`).
    index: usize,
    /// The name it was saved or opened under; empty for a new drawing.
    name: String,
    /// The drawing as last saved or opened, to tell whether there are unsaved changes ("*" after the name).
    /// None after the open drawing was deleted, so it always counts as changed.
    saved: Option<Drawing>,
    /// Steps Undo can go back to, oldest first, at most `UNDO_STEPS`.
    undo: Vec<Snapshot>,
    /// Steps Redo can bring back; cleared by any new change.
    redo: Vec<Snapshot>,
    /// The last (row, column) the brush painted in this stroke, to join it to the next one.
    last_cell: Option<(i64, i64)>,
    /// A mouse button is down and painting.
    stroke: bool,
    /// This stroke erases (right mouse button).
    erase: bool,
    /// When Play was pressed; None while not playing.
    playing: Option<Instant>,
    /// The panels showing the drawing live ("Show on panels" on); empty when it is off.
    live_panels: Vec<Panel>,
    /// The saved drawings' names, as listed in the Open drop-down.
    names: Vec<String>,
}

/// A blank one-frame drawing that counts as saved, so a fresh page doesn't ask "Lose changes?".
impl Default for State {
    fn default() -> State {
        let drawing = drawings::new_pic();
        State {
            saved: Some(drawings::to_drawing(&drawing)),
            drawing,
            active: 0,
            stroke_cols: (0, WIDTH),
            index: 0,
            name: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_cell: None,
            stroke: false,
            erase: false,
            playing: None,
            live_panels: Vec::new(),
            names: Vec::new(),
        }
    }
}

// The editor state, one per UI thread (there is only one). Thread-local because it holds an `Rc` and
// Slint callbacks are plain closures that can reach it from anywhere on that thread.
thread_local! {
    static STATE: Rc<RefCell<State>> = Rc::default();
}

/// The editor state. Only call it on the UI thread.
fn page_state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.clone())
}

/// The newest frame waiting to go to the panels, and whether a send is under way.
/// Shared between the UI thread (which drops the newest frame in) and the one sender thread (which takes
/// it out), so it uses a mutex and an atomic flag rather than the `Rc` state.
struct Pusher {
    /// The frame to send next and the panels to send it to; a newer frame replaces an older unsent one.
    next: Mutex<Option<(Wide, Vec<Panel>)>>,
    /// True while a sender thread is running.
    busy: AtomicBool,
}

/// The one pusher for the page, a static so the sender thread can reach it without being handed it.
static PUSHER: Pusher = Pusher { next: Mutex::new(None), busy: AtomicBool::new(false) };

/// Sets the status line under the editor.
fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<DrawPage>().set_status(text.into());
}

/// An LED as it looks in the window: off ones stay faintly visible.
/// Maps 0..255 onto 0x22..0xFF, so an LED that is off is still a dark grey square, not invisible.
fn shade(v: u8) -> u8 {
    (0x22 + v as u32 * (0xFF - 0x22) / 255) as u8
}

/// Where a column of both panels starts in the editor, in window pixels from the left margin.
fn column_x(c: usize) -> usize {
    c * LED + if c >= WIDTH { GAP } else { 0 }
}

/// Both panels side by side; `active` outlines the panel the tools work on.
/// The picture is built pixel by pixel at `SCALE` times the window size so it stays sharp on a high-DPI
/// screen: a dark background, a lighter strip between the panels, a blue frame round the active panel
/// (when the panels are separate pictures), then one grey square per LED with a 1-pixel dark edge.
fn render_editor(frame: &Wide, active: Option<usize>) -> Image {
    // Picture size in real pixels: 18 LEDs across plus the gap and both margins, 34 LEDs down.
    let (w, h) = ((BOTH * LED + GAP + 2 * MARGIN) * SCALE, (HEIGHT * LED + 2 * MARGIN) * SCALE);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() {
        *p = Rgba8Pixel { r: 0x11, g: 0x11, b: 0x11, a: 255 };
    }
    // The space between the panels
    // The lighter strip fills the gap except 2 window pixels on each side, so it reads as a divider.
    let (g0, g1) = ((MARGIN + WIDTH * LED + 2) * SCALE, (MARGIN + WIDTH * LED + GAP - 2) * SCALE);
    for y in 0..h {
        for x in g0..g1 {
            px[y * w + x] = Rgba8Pixel { r: 0x2a, g: 0x2a, b: 0x2a, a: 255 };
        }
    }
    if let Some(panel) = active {
        // The outline sits 3 window pixels outside the panel and is 2 pixels thick.
        let x0 = (MARGIN + column_x(panel * WIDTH)) * SCALE - 3 * SCALE;
        let x1 = (MARGIN + column_x(panel * WIDTH) + WIDTH * LED) * SCALE + 3 * SCALE;
        let (y0, y1) = (MARGIN * SCALE - 3 * SCALE, (MARGIN + HEIGHT * LED) * SCALE + 3 * SCALE);
        let blue = Rgba8Pixel { r: 0x3a, g: 0x8e, b: 0xe6, a: 255 };
        for y in y0..y1 {
            for x in x0..x1 {
                let edge = x < x0 + 2 * SCALE || x >= x1 - 2 * SCALE || y < y0 + 2 * SCALE || y >= y1 - 2 * SCALE;
                if edge {
                    px[y * w + x] = blue;
                }
            }
        }
    }
    for (r, row) in frame.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let s = shade(*v);
            // Each LED fills its LED x LED cell except a 1-pixel border on every side.
            let (x0, y0) = ((MARGIN + column_x(c) + 1) * SCALE, (MARGIN + r * LED + 1) * SCALE);
            for y in y0..y0 + (LED - 2) * SCALE {
                for x in x0..x0 + (LED - 2) * SCALE {
                    px[y * w + x] = Rgba8Pixel { r: s, g: s, b: s, a: 255 };
                }
            }
        }
    }
    // Slint shows the buffer as an image; it is drawn at 1/SCALE size on screen.
    Image::from_rgba8(buf)
}

/// Both panels with a dark column between them.
/// One window pixel per LED (19 x 34), small enough for the frame strip, which scales it up.
fn render_thumb(frame: &Wide) -> Image {
    let tw = BOTH + 1;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(tw as u32, HEIGHT as u32);
    let px = buf.make_mut_slice();
    for (r, row) in frame.iter().enumerate() {
        // Column 9 is the dark divider; the right panel's columns move one to the right past it.
        px[r * tw + WIDTH] = Rgba8Pixel { r: 0x11, g: 0x11, b: 0x11, a: 255 };
        for (c, v) in row.iter().enumerate() {
            let s = shade(*v);
            px[r * tw + c + (c >= WIDTH) as usize] = Rgba8Pixel { r: s, g: s, b: s, a: 255 };
        }
    }
    Image::from_rgba8(buf)
}

/// Both panels drawn as one picture (the panels sit next to each other).
/// The setting lives on the Effects page ("panels sit together"), as it is shared with the monitor.
fn together(app: &AppWindow) -> bool {
    app.global::<EffectsPage>().get_panels_together()
}

/// The columns the tools work on: both panels, or the active one.
fn region(app: &AppWindow, s: &State) -> (usize, usize) {
    if together(app) {
        (0, BOTH)
    } else {
        (s.active * WIDTH, (s.active + 1) * WIDTH)
    }
}

/// The editor picture for `frame`, with the active panel outlined only when the panels are separate.
fn editor_image(app: &AppWindow, s: &State, frame: &Wide) -> Image {
    render_editor(frame, (!together(app)).then_some(s.active))
}

/// The frame showing after this many seconds of playing (loops).
/// `delay_ms` is at least 1 here, so a broken delay can't divide by zero.
fn frame_at(p: &Pic, seconds: f64) -> &Wide {
    // How many frame times have passed; modulo the frame count makes it loop.
    let step = (seconds * 1000.0 / p.delay_ms.max(1) as f64).floor() as usize;
    &p.frames[step % p.frames.len()]
}

/// True when the drawing differs from what was last saved or opened.
fn is_dirty(s: &State) -> bool {
    s.saved.as_ref() != Some(&drawings::to_drawing(&s.drawing))
}

/// Shows the frame being edited (or `frame`, while playing), the label and the frame strip.
/// While playing (`frame` given), only the editor and the panels change; the label and frame strip
/// stay as they are, which keeps playing cheap at 25 frames a second.
fn redraw(app: &AppWindow, s: &State, frame: Option<&Wide>) {
    let page = app.global::<DrawPage>();
    let current = frame.unwrap_or(&s.drawing.frames[s.index]);
    page.set_editor(editor_image(app, s, current));
    if frame.is_some() {
        push(current, &s.live_panels, app);
        return;
    }
    // The label: the drawing's name with "*" while it has unsaved changes, and which frame this is.
    let name = if s.name.is_empty() {
        "Not saved yet".to_string()
    } else {
        format!("{}{}", s.name, if is_dirty(s) { " *" } else { "" })
    };
    page.set_label(format!("{name}\nFrame {} of {}", s.index + 1, s.drawing.frames.len()).into());
    // Every frame's thumbnail is rebuilt each time; at most `MAX_FRAMES` tiny pictures, so it's quick.
    let thumbs: Vec<Image> = s.drawing.frames.iter().map(render_thumb).collect();
    page.set_thumbs(ModelRc::new(VecModel::from(thumbs)));
    page.set_index(s.index as i32);
    page.set_delay(s.drawing.delay_ms as i32);
    if page.get_live() {
        push(current, &s.live_panels, app);
    }
}

/// Saves the drawing as an undo step before a change, and forgets anything that could be redone.
fn remember(s: &mut State) {
    s.undo.push((s.drawing.frames.clone(), s.index, s.drawing.delay_ms));
    if s.undo.len() > UNDO_STEPS {
        // Drop the oldest step. (A Vec is fine here: 50 entries, removed once per change.)
        s.undo.remove(0);
    }
    s.redo.clear();
}

/// Stops Play, if it is on. Every editing action calls this first, so the editor never edits a frame
/// the player is about to replace on screen.
fn stop_playing(app: &AppWindow, s: &mut State) {
    if s.playing.take().is_some() {
        app.global::<DrawPage>().set_playing(false);
    }
}

/// Changes the frame being edited in the columns the tools work on, with undo: f(frame, c0, c1).
fn transform(app: &AppWindow, f: impl Fn(&Wide, usize, usize) -> Wide) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    remember(&mut s);
    let i = s.index;
    // (Each whole-frame tool is one undo step.)
    let (c0, c1) = region(app, &s);
    s.drawing.frames[i] = f(&s.drawing.frames[i], c0, c1);
    redraw(app, &s, None);
}

/// Changes the list of frames: f(frames, index) -> (frames, index).
/// Refuses (with a message) when the result would have more than `MAX_FRAMES` frames. The undo step is
/// only taken once the change is known to be allowed.
fn change_frames(app: &AppWindow, f: impl Fn(Vec<Wide>, usize) -> (Vec<Wide>, usize)) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    // Work on a copy, so a refused change leaves the drawing as it was.
    let (frames, index) = f(s.drawing.frames.clone(), s.index);
    if frames.len() > MAX_FRAMES {
        show(app, format!("A drawing can have up to {MAX_FRAMES} frames."));
        return;
    }
    remember(&mut s);
    s.drawing.frames = frames;
    // `f` never returns an empty list (deleting the last frame is handled by the caller), so the
    // subtraction can't underflow.
    s.index = index.min(s.drawing.frames.len() - 1);
    redraw(app, &s, None);
}

/// Undo (`undo` true) or Redo: swaps the drawing with the newest step of one list and pushes the current
/// drawing onto the other, so the two always mirror each other. Nothing to undo does nothing.
fn restore(app: &AppWindow, undo: bool) {
    let st = page_state();
    let mut s = st.borrow_mut();
    // Borrow the struct itself so the two lists and the drawing can be borrowed separately below.
    let s = &mut *s;
    let (source, target) = if undo { (&mut s.undo, &mut s.redo) } else { (&mut s.redo, &mut s.undo) };
    let Some((frames, index, delay)) = source.pop() else { return };
    target.push((s.drawing.frames.clone(), s.index, s.drawing.delay_ms));
    s.index = index.min(frames.len() - 1);
    s.drawing = Pic { frames, delay_ms: delay };
    redraw(app, s, None);
}

/// Paints a square brush (1, 2 or 3 LEDs wide) centred on (row r, column c) in the frame being edited,
/// with the chosen shade, or 0 when erasing (right button or Eraser tool). Clipped to the panel rows and
/// to the columns of the stroke, so a stroke started on one panel never spills onto the other.
fn paint(app: &AppWindow, s: &mut State, r: i64, c: i64) {
    let page = app.global::<DrawPage>();
    let value = if s.erase || page.get_tool() == 1 { 0 } else { page.get_shade().clamp(0, 255) as u8 };
    let size = page.get_size().clamp(1, 3) as i64;
    // For an even size the extra row and column go below and to the right of the cell.
    let (top, left) = (r - (size - 1) / 2, c - (size - 1) / 2);
    let i = s.index;
    let (c0, c1) = s.stroke_cols;
    for rr in top.max(0)..(top + size).min(HEIGHT as i64) {
        for cc in left.max(c0 as i64)..(left + size).min(c1 as i64) {
            s.drawing.frames[i][rr as usize][cc as usize] = value;
        }
    }
}

/// The cell under the mouse as (row, column) across both panels, None outside the 18 x 34 area.
/// The Slint side has already turned window pixels into cell numbers (and skipped the gap).
fn cell(x: i32, y: i32) -> Option<(i64, i64)> {
    ((0..BOTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y)).then_some((y as i64, x as i64))
}

/// Mouse button down on the editor at cell (x, y); `erase` is true for the right button. Starts a brush
/// stroke, or with the Fill area tool fills the area of the same shade around the cell in one go.
fn press(app: &AppWindow, x: i32, y: i32, erase: bool) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    let Some((r, c)) = cell(x, y) else { return };
    remember(&mut s);
    s.stroke = true;
    s.erase = erase;
    // Drawing on a panel makes it the one the whole-frame tools work on
    // Column 0-8 is the left panel (0), 9-17 the right (1).
    s.active = c as usize / WIDTH;
    s.stroke_cols = region(app, &s);
    // Tool 2 is "Fill area"; the right button still erases with the brush.
    if app.global::<DrawPage>().get_tool() == 2 && !erase {
        let i = s.index;
        let value = app.global::<DrawPage>().get_shade().clamp(0, 255) as u8;
        let (c0, c1) = s.stroke_cols;
        s.drawing.frames[i] = drawings::flood_fill(&s.drawing.frames[i], r as usize, c as usize, value, c0, c1);
        s.stroke = false;
        redraw(app, &s, None);
        return;
    }
    s.last_cell = Some((r, c));
    paint(app, &mut s, r, c);
    // A frame is a small fixed-size array, so copying it is cheap and ends the borrow of the drawing.
    let frame = s.drawing.frames[s.index];
    app.global::<DrawPage>().set_editor(editor_image(app, &s, &frame));
    if app.global::<DrawPage>().get_live() {
        push(&frame, &s.live_panels, app);
    }
}

/// The mouse moved with a button down: paints every cell on the straight line from the last one, then
/// updates the editor and (if live) the panels. Only the editor picture is redrawn, not the thumbnails,
/// which `release` does once at the end of the stroke.
fn drag(app: &AppWindow, x: i32, y: i32) {
    let st = page_state();
    let mut s = st.borrow_mut();
    if !s.stroke {
        return;
    }
    let Some((r1, c1)) = cell(x, y) else { return };
    if s.last_cell == Some((r1, c1)) {
        return;
    }
    // Fill in between, so quick strokes don't leave gaps
    let (r0, c0) = s.last_cell.unwrap_or((r1, c1));
    // One step per cell along the longer axis, rounding the other axis: a simple line between the
    // two points with no gaps.
    let steps = (r1 - r0).abs().max((c1 - c0).abs()).max(1);
    for i in 1..=steps {
        let r = (r0 as f64 + (r1 - r0) as f64 * i as f64 / steps as f64).round() as i64;
        let c = (c0 as f64 + (c1 - c0) as f64 * i as f64 / steps as f64).round() as i64;
        paint(app, &mut s, r, c);
    }
    s.last_cell = Some((r1, c1));
    let frame = s.drawing.frames[s.index];
    app.global::<DrawPage>().set_editor(editor_image(app, &s, &frame));
    if app.global::<DrawPage>().get_live() {
        push(&frame, &s.live_panels, app);
    }
}

/// Mouse button up: ends the stroke and redraws everything (label "*", thumbnails).
fn release(app: &AppWindow) {
    let st = page_state();
    let mut s = st.borrow_mut();
    let was = s.stroke;
    s.stroke = false;
    s.last_cell = None;
    if was {
        redraw(app, &s, None);
    }
}

/// Sends the frame to the panels soon (each panel its half): at most one send at a time, the newest
/// frame wins.
/// Runs on the UI thread and returns at once; the writing happens on a sender thread. Does nothing
/// unless "Show on panels" is on and some panels are chosen.
fn push(frame: &Wide, panels: &[Panel], app: &AppWindow) {
    if !app.global::<DrawPage>().get_live() || panels.is_empty() {
        return;
    }
    *PUSHER.next.lock().unwrap() = Some((*frame, panels.to_vec()));
    // A sender thread is already running; it will pick up the frame we just left.
    if PUSHER.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let parts = ctx().clone_parts();
    std::thread::spawn(move || loop {
        // Take the newest frame; a frame left after this one replaces it, and older ones are never sent.
        let next = PUSHER.next.lock().unwrap().take();
        let Some((frame, panels)) = next else {
            // Nothing left: mark the thread finished. The check below closes the gap where `push` put a
            // frame in after our `take` but saw `busy` still true and didn't start a thread.
            PUSHER.busy.store(false, Ordering::SeqCst);
            // A frame may have arrived just before busy was cleared
            if PUSHER.next.lock().unwrap().is_some() && !PUSHER.busy.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        };
        // Panels not borrowed yet are borrowed now ("hold" to the monitor), and any clock, game or
        // Math job on them is stopped by starting an empty job, so nothing else draws over the drawing.
        let new: Vec<Panel> = panels.iter().filter(|p| !parts.lender.is_borrowed(&p.device)).cloned().collect();
        if !new.is_empty() {
            crate::jobs::start(&new.iter().map(|p| p.device.clone()).collect::<Vec<_>>());
            parts.lender.borrow(&parts.sender, &new);
            let p2 = parts.clone();
            Ctx::later(&parts.ui, move |app| p2.update_bar(app));
            for p in &new {
                let _ = parts.sender.write(&p.device, &[cmd::animate(false)]);
            }
        }
        let writes: Vec<(String, Vec<Vec<u8>>)> = panels
            .iter()
            .map(|p| {
                // Each panel shows its own half; `greyscale` wants columns, so turn rows into columns.
                let half = drawings::half(&frame, (p.side == "right") as usize);
                let mut grid = [[0u8; cmd::HEIGHT]; cmd::WIDTH];
                for (y, row) in half.iter().enumerate() {
                    for (x, v) in row.iter().enumerate() {
                        grid[x][y] = *v;
                    }
                }
                (p.device.clone(), cmd::greyscale(&grid))
            })
            .collect();
        // The thread keeps going after an error: a later frame may get through.
        if let Err(e) = parts.sender.write_many(&writes) {
            let text = format!("Couldn't show it on the panels: {e}");
            Ctx::later(&parts.ui, move |app| show(app, text));
        }
    });
}

/// Reads the drawings folder again (fast: a directory listing) and selects `select` in the drop-down,
/// or nothing if it isn't there.
fn refresh_names(app: &AppWindow, select: Option<&str>) {
    let st = page_state();
    let mut s = st.borrow_mut();
    s.names = drawings::list_names(&drawings::folder());
    let page = app.global::<DrawPage>();
    let names: Vec<SharedString> = s.names.iter().map(|n| n.as_str().into()).collect();
    page.set_names(ModelRc::new(VecModel::from(names)));
    page.set_chosen(select.and_then(|n| s.names.iter().position(|x| x == n)).map(|i| i as i32).unwrap_or(-1));
}

/// Asks first when there are unsaved changes. No borrow of the state may be held: the question runs
/// the window's timers while it waits.
fn ok_to_lose_changes() -> bool {
    // The borrow ends at the end of this line, before the dialog opens.
    let dirty = is_dirty(&page_state().borrow());
    !dirty || dialogs::confirm("Lose changes?", "This drawing has changes that aren't saved. Carry on anyway?")
}

/// Wraps a handler that needs the window into a plain Slint callback; does nothing once the window is gone.
fn with_app(f: impl Fn(&AppWindow) + 'static) -> impl Fn() + 'static {
    move || {
        if let Some(app) = ctx().ui.upgrade() {
            f(&app);
        }
    }
}

/// Gives the Slint side the editor's sizes, fills the lists, draws the empty editor, registers the
/// player timer and connects every callback (see the file header). Runs once at start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<DrawPage>();
    page.set_led(LED as f32);
    page.set_margin(MARGIN as f32);
    page.set_gap(GAP as f32);
    let fits: Vec<SharedString> = drawings::FITS.iter().map(|(_, l)| SharedString::from(*l)).collect();
    page.set_fits(ModelRc::new(VecModel::from(fits)));
    refresh_names(app, None);
    redraw(app, &page_state().borrow(), None);
    // Each time the page is shown, re-read the drawings folder (the user may have added files to it)
    // and keep the same drawing selected.
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "draw" {
            let chosen = app.global::<DrawPage>().get_chosen();
            let st = page_state();
            // The name selected before the refresh, if any, so it stays selected.
            let name = st.borrow().names.get(chosen.max(0) as usize).cloned().filter(|_| chosen >= 0);
            refresh_names(app, name.as_deref());
        }
    }));
    // Playing, and noticing when 'Back to system monitor' took the panels back
    {
        let ui = ctx.ui.clone();
        ctx.every(0.04, move || {
            let Some(app) = ui.upgrade() else { return };
            let st = page_state();
            let mut s = st.borrow_mut();
            let page = app.global::<DrawPage>();
            // Live, the sender is idle, yet none of the live panels is borrowed any more: something
            // else gave them back (the bottom bar's button, or another page). Turn the switch off.
            if page.get_live()
                && !s.live_panels.is_empty()
                && !PUSHER.busy.load(Ordering::SeqCst)
                && !s.live_panels.iter().any(|p| crate::app::ctx().lender.is_borrowed(&p.device))
            {
                page.set_live(false);
                s.live_panels.clear();
            }
            if let Some(started) = s.playing {
                // While playing, show the frame for the time since Play was pressed (on the panels too).
                let frame = *frame_at(&s.drawing, started.elapsed().as_secs_f64());
                redraw(&app, &s, Some(&frame));
            }
        });
    }
    page.on_press(|x, y, erase| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            press(&app, x, y, erase);
        }
    });
    page.on_drag(|x, y| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            drag(&app, x, y);
        }
    });
    page.on_release(with_app(release));
    // Picking a shade means the user wants to draw: leave the Eraser for the Pen. At least 1, so
    // the pen never draws "off".
    page.on_shade_picked(|v| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let page = app.global::<DrawPage>();
        page.set_shade(v.clamp(1, 255));
        if page.get_tool() == 1 {
            page.set_tool(0);
        }
    });
    page.on_clear(with_app(|app| transform(app, |f, c0, c1| drawings::fill(f, 0, c0, c1))));
    page.on_fill(with_app(|app| {
        let v = app.global::<DrawPage>().get_shade().clamp(0, 255) as u8;
        transform(app, move |f, c0, c1| drawings::fill(f, v, c0, c1));
    }));
    page.on_swap(with_app(|app| transform(app, drawings::invert)));
    page.on_mirror(with_app(|app| transform(app, drawings::mirror)));
    page.on_flip(with_app(|app| transform(app, drawings::upside_down)));
    page.on_undo(with_app(|app| restore(app, true)));
    page.on_redo(with_app(|app| restore(app, false)));
    page.on_shift(|dy, dx| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            transform(&app, move |f, c0, c1| drawings::roll(f, dy as i64, dx as i64, c0, c1));
        }
    });
    page.on_pick_frame(|i| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let st = page_state();
        let mut s = st.borrow_mut();
        if (i as usize) < s.drawing.frames.len() {
            stop_playing(&app, &mut s);
            s.index = i as usize;
            redraw(&app, &s, None);
        }
    });
    page.on_add_blank(with_app(|app| {
        change_frames(app, |mut f, i| {
            f.insert(i + 1, drawings::blank_wide());
            (f, i + 1)
        })
    }));
    page.on_duplicate(with_app(|app| {
        change_frames(app, |mut f, i| {
            f.insert(i + 1, f[i]);
            (f, i + 1)
        })
    }));
    // The last frame can't be removed (a drawing needs one), so it is cleared instead.
    page.on_delete_frame(with_app(|app| {
        let one = page_state().borrow().drawing.frames.len() == 1;
        if one {
            let st = page_state();
            let mut s = st.borrow_mut();
            stop_playing(app, &mut s);
            remember(&mut s);
            s.drawing.frames[0] = drawings::blank_wide();
            redraw(app, &s, None);
        } else {
            change_frames(app, |mut f, i| {
                f.remove(i);
                let n = f.len();
                (f, i.min(n - 1))
            });
        }
    }));
    // Moves the frame `step` places (-1 left, +1 right); off either end does nothing.
    page.on_move_frame(|step| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        change_frames(&app, move |mut f, i| {
            let j = i as i64 + step as i64;
            if j < 0 || j as usize >= f.len() {
                return (f, i);
            }
            f.swap(i, j as usize);
            (f, j as usize)
        });
    });
    // The delay box: milliseconds between frames, clamped to the range the Picture app allows.
    page.on_delay_changed(|v| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let st = page_state();
        let mut s = st.borrow_mut();
        let value = drawings::clamp_delay(Some(&serde_json::json!(v)));
        if value != s.drawing.delay_ms {
            remember(&mut s);
            s.drawing.delay_ms = value;
        }
        redraw(&app, &s, None);
    });
    page.on_play(with_app(|app| {
        let st = page_state();
        let mut s = st.borrow_mut();
        if s.playing.is_some() {
            stop_playing(app, &mut s);
            redraw(app, &s, None);
        } else if s.drawing.frames.len() > 1 {
            s.playing = Some(Instant::now());
            app.global::<DrawPage>().set_playing(true);
        } else {
            show(app, "Add a second frame to make an animation.");
        }
    }));
    // The file dialog is modal and returns once the user picks a file or cancels.
    page.on_import(with_app(|app| {
        let Some(path) = dialogs::open_file("Import a picture", &[("Pictures", &["png", "gif", "jpg", "jpeg", "bmp"])])
        else {
            return;
        };
        let page = app.global::<DrawPage>();
        // `fit` says how a picture of another size is fitted to the LEDs; `invert` swaps light and dark.
        // Decoding and scaling a GIF takes a moment but runs on the UI thread, as it is a one-off.
        let fit = drawings::FITS.get(page.get_fit().max(0) as usize).map(|f| f.0).unwrap_or("whole");
        let both = together(app);
        match drawings::import(&path, fit, page.get_invert(), if both { BOTH } else { WIDTH }) {
            Ok(imported) => {
                let st = page_state();
                let mut s = st.borrow_mut();
                stop_playing(app, &mut s);
                remember(&mut s);
                let count = imported.frames.len();
                // Into both panels, or into the panel the tools work on (the other keeps its frames)
                let c0 = if both { 0 } else { s.active * WIDTH };
                let old = s.drawing.frames.clone();
                let frames: Vec<Wide> = imported
                    .frames
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        // Into one panel: start from that frame of the old drawing (its last frame when
                        // the import has more), so the other panel's half is kept.
                        let mut w = if both { drawings::blank_wide() } else { old[i.min(old.len() - 1)] };
                        for (y, row) in f.iter().enumerate() {
                            for (x, v) in row.iter().enumerate() {
                                w[y][c0 + x] = *v;
                            }
                        }
                        w
                    })
                    .collect();
                s.drawing = Pic { frames, delay_ms: imported.delay_ms };
                s.index = 0;
                redraw(app, &s, None);
                show(
                    app,
                    format!(
                        "Imported {count} frame{}.{}",
                        if count != 1 { "s" } else { "" },
                        if count > 1 { " Press Play to watch it." } else { "" }
                    ),
                );
            }
            Err(e) => show(app, format!("Couldn't import that picture: {e}")),
        }
    }));
    // "Show on panels" switched: on borrows the ticked panels (through `push`, at the next draw)
    // after stopping any game or Math picture; off gives them back to the monitor at once.
    page.on_live_changed(|on| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let page = app.global::<DrawPage>();
        let st = page_state();
        if on {
            let panels = crate::app::ctx().selected_panels();
            if panels.is_empty() {
                page.set_live(false);
                show(&app, "Tick a panel under \"Controls act on\", bottom left, first.");
                return;
            }
            crate::pages::games::stop_for_other(&app);
            crate::pages::math::stop_for_other(&app);
            page.set_live(true);
            let mut s = st.borrow_mut();
            s.live_panels = panels;
            redraw(&app, &s, None);
            show(&app, "The ticked panels show the drawing. 'Back to system monitor' at the bottom gives them back.");
        } else {
            page.set_live(false);
            let panels = std::mem::take(&mut st.borrow_mut().live_panels);
            if !panels.is_empty() {
                crate::app::ctx().clone_parts().give_back(panels, None);
            }
        }
    });
    // `safe_name` drops characters that can't be in a file name. Asks before replacing a different
    // drawing with the same name; saving again under the open drawing's own name doesn't ask.
    page.on_save(with_app(|app| {
        let page = app.global::<DrawPage>();
        let name = drawings::safe_name(&page.get_name());
        if name.is_empty() {
            show(app, "Type a name for the drawing first.");
            return;
        }
        let st = page_state();
        let current = st.borrow().name.clone();
        let exists = st.borrow().names.contains(&name);
        if name != current
            && exists
            && !dialogs::confirm("Replace drawing?", &format!("There is already a drawing called {name}. Replace it?"))
        {
            return;
        }
        let drawing = drawings::to_drawing(&st.borrow().drawing);
        // Whether anything is drawn on the right half, for the hint in the message.
        let both = !drawing.right.is_empty();
        // Writes <name>.json in the drawings folder; the returned name is the one actually used.
        match drawings::save(&drawings::folder(), &name, &drawing) {
            Ok(name) => {
                {
                    let mut s = st.borrow_mut();
                    s.name = name.clone();
                    s.saved = Some(drawing);
                }
                page.set_name(name.as_str().into());
                refresh_names(app, Some(&name));
                redraw(app, &st.borrow(), None);
                show(
                    app,
                    format!(
                        "Saved {name}. The Picture app on the System monitor page can show it now{}.",
                        if both { " (put it on both panels: each shows its half)" } else { "" }
                    ),
                );
            }
            Err(e) => show(app, format!("Couldn't save: {e}")),
        }
    }));
    // Opens the drawing chosen in the drop-down, after asking if there are unsaved changes.
    // Undo history is cleared, as it belonged to the other drawing.
    page.on_open(with_app(|app| {
        let page = app.global::<DrawPage>();
        let st = page_state();
        let Some(name) = st.borrow().names.get(page.get_chosen().max(0) as usize).cloned() else { return };
        if page.get_chosen() < 0 || !ok_to_lose_changes() {
            return;
        }
        match drawings::load(&drawings::folder(), &name) {
            Ok(drawing) => {
                let mut s = st.borrow_mut();
                stop_playing(app, &mut s);
                s.drawing = drawings::from_drawing(&drawing);
                // The saved copy is taken from the editor's own form, so "dirty" compares like with like.
                s.saved = Some(drawings::to_drawing(&s.drawing));
                s.index = 0;
                s.name = name.clone();
                s.undo.clear();
                s.redo.clear();
                page.set_name(name.as_str().into());
                redraw(app, &s, None);
                show(app, format!("Opened {name}."));
            }
            Err(e) => show(app, format!("Couldn't open {name}: {e}")),
        }
    }));
    page.on_new_drawing(with_app(|app| {
        if !ok_to_lose_changes() {
            return;
        }
        let st = page_state();
        let mut s = st.borrow_mut();
        stop_playing(app, &mut s);
        s.drawing = drawings::new_pic();
        s.saved = Some(drawings::to_drawing(&s.drawing));
        s.index = 0;
        s.name.clear();
        s.undo.clear();
        s.redo.clear();
        app.global::<DrawPage>().set_name("".into());
        redraw(app, &s, None);
    }));
    // Deletes the chosen drawing's file after asking. If it was the one open, it stays in the
    // editor but now counts as never saved.
    page.on_delete_drawing(with_app(|app| {
        let page = app.global::<DrawPage>();
        let st = page_state();
        let Some(name) = st.borrow().names.get(page.get_chosen().max(0) as usize).cloned() else { return };
        if page.get_chosen() < 0
            || !dialogs::confirm("Delete drawing?", &format!("Delete {name}? A panel showing it goes blank."))
        {
            return;
        }
        if let Err(e) = drawings::delete(&drawings::folder(), &name) {
            show(app, format!("Couldn't delete {name}: {e}"));
            return;
        }
        {
            let mut s = st.borrow_mut();
            if s.name == name {
                s.name.clear();
                s.saved = None;
            }
        }
        refresh_names(app, None);
        redraw(app, &st.borrow(), None);
        show(app, format!("Deleted {name}."));
    }));
    // "Panels together" changed on this or the Effects page: redraw to show or hide the outline.
    page.on_together_changed(with_app(|app| {
        let st = page_state();
        redraw(app, &st.borrow(), None);
    }));
    // Opens the drawings folder in Explorer, creating it first so there is something to open.
    page.on_show_folder(|| {
        let dir = drawings::folder();
        let _ = std::fs::create_dir_all(&dir);
        crate::app::open_path(&dir.display().to_string());
    });
}
