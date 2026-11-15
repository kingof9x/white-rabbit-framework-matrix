//! Draw (DrawTab in `draw_tab.py`): draw on one panel's 9 x 34 LEDs in shades of grey, make an
//! animation out of several frames, import a PNG or GIF, and save drawings for the Picture app.
//!
//! The idea of a drawing editor comes from Deo's LED Matrix Manager (MIT licence); this is our own code.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use wr_core::grid::{HEIGHT, WIDTH};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::drawings::{self, Drawing, Frame, MAX_FRAMES};
use crate::panels::Panel;
use crate::{dialogs, AppWindow, DrawPage};

const LED: usize = 14;
const MARGIN: usize = 6;
/// Pixels per window pixel in the editor's picture, so it stays sharp on scaled screens
const SCALE: usize = 2;
const UNDO_STEPS: usize = 50;

type Snapshot = (Vec<Frame>, usize, i64);

struct State {
    drawing: Drawing,
    index: usize,
    name: String,
    saved: Option<Drawing>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_cell: Option<(i64, i64)>,
    stroke: bool,
    erase: bool,
    playing: Option<Instant>,
    live_panels: Vec<Panel>,
    names: Vec<String>,
}

impl Default for State {
    fn default() -> State {
        let drawing = drawings::new_drawing();
        State {
            saved: Some(drawing.clone()),
            drawing,
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

thread_local! {
    static STATE: Rc<RefCell<State>> = Rc::default();
}

fn page_state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.clone())
}

/// The newest frame waiting to go to the panels, and whether a send is under way.
struct Pusher {
    next: Mutex<Option<(Frame, Vec<Panel>)>>,
    busy: AtomicBool,
}

static PUSHER: Pusher = Pusher { next: Mutex::new(None), busy: AtomicBool::new(false) };

fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<DrawPage>().set_status(text.into());
}

/// An LED as it looks in the window: off ones stay faintly visible.
fn shade(v: u8) -> u8 {
    (0x22 + v as u32 * (0xFF - 0x22) / 255) as u8
}

fn render_editor(frame: &Frame) -> Image {
    let (w, h) = ((WIDTH * LED + 2 * MARGIN) * SCALE, (HEIGHT * LED + 2 * MARGIN) * SCALE);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() {
        *p = Rgba8Pixel { r: 0x11, g: 0x11, b: 0x11, a: 255 };
    }
    for (r, row) in frame.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let s = shade(*v);
            let (x0, y0) = ((MARGIN + c * LED + 1) * SCALE, (MARGIN + r * LED + 1) * SCALE);
            for y in y0..y0 + (LED - 2) * SCALE {
                for x in x0..x0 + (LED - 2) * SCALE {
                    px[y * w + x] = Rgba8Pixel { r: s, g: s, b: s, a: 255 };
                }
            }
        }
    }
    Image::from_rgba8(buf)
}

fn render_thumb(frame: &Frame) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(WIDTH as u32, HEIGHT as u32);
    let px = buf.make_mut_slice();
    for (r, row) in frame.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let s = shade(*v);
            px[r * WIDTH + c] = Rgba8Pixel { r: s, g: s, b: s, a: 255 };
        }
    }
    Image::from_rgba8(buf)
}

fn is_dirty(s: &State) -> bool {
    s.saved.as_ref() != Some(&s.drawing)
}

/// Shows the frame being edited (or `frame`, while playing), the label and the frame strip.
fn redraw(app: &AppWindow, s: &State, frame: Option<&Frame>) {
    let page = app.global::<DrawPage>();
    let current = frame.unwrap_or(&s.drawing.frames[s.index]);
    page.set_editor(render_editor(current));
    if frame.is_some() {
        push(current, &s.live_panels, app);
        return;
    }
    let name = if s.name.is_empty() {
        "Not saved yet".to_string()
    } else {
        format!("{}{}", s.name, if is_dirty(s) { " *" } else { "" })
    };
    page.set_label(format!("{name}\nFrame {} of {}", s.index + 1, s.drawing.frames.len()).into());
    let thumbs: Vec<Image> = s.drawing.frames.iter().map(render_thumb).collect();
    page.set_thumbs(ModelRc::new(VecModel::from(thumbs)));
    page.set_index(s.index as i32);
    page.set_delay(s.drawing.delay_ms as i32);
    if page.get_live() {
        push(current, &s.live_panels, app);
    }
}

fn remember(s: &mut State) {
    s.undo.push((s.drawing.frames.clone(), s.index, s.drawing.delay_ms));
    if s.undo.len() > UNDO_STEPS {
        s.undo.remove(0);
    }
    s.redo.clear();
}

fn stop_playing(app: &AppWindow, s: &mut State) {
    if s.playing.take().is_some() {
        app.global::<DrawPage>().set_playing(false);
    }
}

/// Changes the frame being edited, with undo.
fn transform(app: &AppWindow, f: impl Fn(&Frame) -> Frame) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    remember(&mut s);
    let i = s.index;
    s.drawing.frames[i] = f(&s.drawing.frames[i]);
    redraw(app, &s, None);
}

/// Changes the list of frames: f(frames, index) -> (frames, index).
fn change_frames(app: &AppWindow, f: impl Fn(Vec<Frame>, usize) -> (Vec<Frame>, usize)) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    let (frames, index) = f(s.drawing.frames.clone(), s.index);
    if frames.len() > MAX_FRAMES {
        show(app, format!("A drawing can have up to {MAX_FRAMES} frames."));
        return;
    }
    remember(&mut s);
    s.drawing.frames = frames;
    s.index = index.min(s.drawing.frames.len() - 1);
    redraw(app, &s, None);
}

fn restore(app: &AppWindow, undo: bool) {
    let st = page_state();
    let mut s = st.borrow_mut();
    let s = &mut *s;
    let (source, target) = if undo { (&mut s.undo, &mut s.redo) } else { (&mut s.redo, &mut s.undo) };
    let Some((frames, index, delay)) = source.pop() else { return };
    target.push((s.drawing.frames.clone(), s.index, s.drawing.delay_ms));
    s.index = index.min(frames.len() - 1);
    s.drawing = Drawing { frames, delay_ms: delay };
    redraw(app, s, None);
}

fn paint(app: &AppWindow, s: &mut State, r: i64, c: i64) {
    let page = app.global::<DrawPage>();
    let value = if s.erase || page.get_tool() == 1 { 0 } else { page.get_shade().clamp(0, 255) as u8 };
    let size = page.get_size().clamp(1, 3) as i64;
    let (top, left) = (r - (size - 1) / 2, c - (size - 1) / 2);
    let i = s.index;
    for rr in top.max(0)..(top + size).min(HEIGHT as i64) {
        for cc in left.max(0)..(left + size).min(WIDTH as i64) {
            s.drawing.frames[i][rr as usize][cc as usize] = value;
        }
    }
}

fn cell(x: i32, y: i32) -> Option<(i64, i64)> {
    ((0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y)).then_some((y as i64, x as i64))
}

fn press(app: &AppWindow, x: i32, y: i32, erase: bool) {
    let st = page_state();
    let mut s = st.borrow_mut();
    stop_playing(app, &mut s);
    let Some((r, c)) = cell(x, y) else { return };
    remember(&mut s);
    s.stroke = true;
    s.erase = erase;
    if app.global::<DrawPage>().get_tool() == 2 && !erase {
        let i = s.index;
        let value = app.global::<DrawPage>().get_shade().clamp(0, 255) as u8;
        s.drawing.frames[i] = drawings::flood_fill(&s.drawing.frames[i], r as usize, c as usize, value);
        s.stroke = false;
        redraw(app, &s, None);
        return;
    }
    s.last_cell = Some((r, c));
    paint(app, &mut s, r, c);
    let frame = s.drawing.frames[s.index];
    app.global::<DrawPage>().set_editor(render_editor(&frame));
    if app.global::<DrawPage>().get_live() {
        push(&frame, &s.live_panels, app);
    }
}

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
    let steps = (r1 - r0).abs().max((c1 - c0).abs()).max(1);
    for i in 1..=steps {
        let r = (r0 as f64 + (r1 - r0) as f64 * i as f64 / steps as f64).round() as i64;
        let c = (c0 as f64 + (c1 - c0) as f64 * i as f64 / steps as f64).round() as i64;
        paint(app, &mut s, r, c);
    }
    s.last_cell = Some((r1, c1));
    let frame = s.drawing.frames[s.index];
    app.global::<DrawPage>().set_editor(render_editor(&frame));
    if app.global::<DrawPage>().get_live() {
        push(&frame, &s.live_panels, app);
    }
}

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

/// Sends the frame to the panels soon: at most one send at a time, the newest frame wins.
fn push(frame: &Frame, panels: &[Panel], app: &AppWindow) {
    if !app.global::<DrawPage>().get_live() || panels.is_empty() {
        return;
    }
    *PUSHER.next.lock().unwrap() = Some((*frame, panels.to_vec()));
    if PUSHER.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let parts = ctx().clone_parts();
    std::thread::spawn(move || loop {
        let next = PUSHER.next.lock().unwrap().take();
        let Some((frame, panels)) = next else {
            PUSHER.busy.store(false, Ordering::SeqCst);
            // A frame may have arrived just before busy was cleared
            if PUSHER.next.lock().unwrap().is_some() && !PUSHER.busy.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        };
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
        let mut grid = [[0u8; cmd::HEIGHT]; cmd::WIDTH];
        for (y, row) in frame.iter().enumerate() {
            for (x, v) in row.iter().enumerate() {
                grid[x][y] = *v;
            }
        }
        let writes: Vec<(String, Vec<Vec<u8>>)> =
            panels.iter().map(|p| (p.device.clone(), cmd::greyscale(&grid))).collect();
        if let Err(e) = parts.sender.write_many(&writes) {
            let text = format!("Couldn't show it on the panels: {e}");
            Ctx::later(&parts.ui, move |app| show(app, text));
        }
    });
}

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
    let dirty = is_dirty(&page_state().borrow());
    !dirty || dialogs::confirm("Lose changes?", "This drawing has changes that aren't saved. Carry on anyway?")
}

fn with_app(f: impl Fn(&AppWindow) + 'static) -> impl Fn() + 'static {
    move || {
        if let Some(app) = ctx().ui.upgrade() {
            f(&app);
        }
    }
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<DrawPage>();
    page.set_led(LED as f32);
    page.set_margin(MARGIN as f32);
    let fits: Vec<SharedString> = drawings::FITS.iter().map(|(_, l)| SharedString::from(*l)).collect();
    page.set_fits(ModelRc::new(VecModel::from(fits)));
    refresh_names(app, None);
    redraw(app, &page_state().borrow(), None);
    ctx.on_page.lock().unwrap().push(Box::new(|app, key| {
        if key == "draw" {
            let chosen = app.global::<DrawPage>().get_chosen();
            let st = page_state();
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
            if page.get_live()
                && !s.live_panels.is_empty()
                && !PUSHER.busy.load(Ordering::SeqCst)
                && !s.live_panels.iter().any(|p| crate::app::ctx().lender.is_borrowed(&p.device))
            {
                page.set_live(false);
                s.live_panels.clear();
            }
            if let Some(started) = s.playing {
                let frame = *s.drawing.frame_at(started.elapsed().as_secs_f64());
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
    page.on_shade_picked(|v| {
        let Some(app) = crate::app::ctx().ui.upgrade() else { return };
        let page = app.global::<DrawPage>();
        page.set_shade(v.clamp(1, 255));
        if page.get_tool() == 1 {
            page.set_tool(0);
        }
    });
    page.on_clear(with_app(|app| transform(app, |_| drawings::blank())));
    page.on_fill(with_app(|app| {
        let v = app.global::<DrawPage>().get_shade().clamp(0, 255) as u8;
        transform(app, move |_| [[v; WIDTH]; HEIGHT]);
    }));
    page.on_swap(with_app(|app| transform(app, drawings::invert)));
    page.on_mirror(with_app(|app| transform(app, drawings::mirror)));
    page.on_flip(with_app(|app| transform(app, drawings::upside_down)));
    page.on_undo(with_app(|app| restore(app, true)));
    page.on_redo(with_app(|app| restore(app, false)));
    page.on_shift(|dy, dx| {
        if let Some(app) = crate::app::ctx().ui.upgrade() {
            transform(&app, move |f| drawings::roll(f, dy as i64, dx as i64));
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
            f.insert(i + 1, drawings::blank());
            (f, i + 1)
        })
    }));
    page.on_duplicate(with_app(|app| {
        change_frames(app, |mut f, i| {
            f.insert(i + 1, f[i]);
            (f, i + 1)
        })
    }));
    page.on_delete_frame(with_app(|app| {
        let one = page_state().borrow().drawing.frames.len() == 1;
        if one {
            transform(app, |_| drawings::blank());
        } else {
            change_frames(app, |mut f, i| {
                f.remove(i);
                let n = f.len();
                (f, i.min(n - 1))
            });
        }
    }));
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
    page.on_import(with_app(|app| {
        let Some(path) = dialogs::open_file("Import a picture", &[("Pictures", &["png", "gif", "jpg", "jpeg", "bmp"])])
        else {
            return;
        };
        let page = app.global::<DrawPage>();
        let fit = drawings::FITS.get(page.get_fit().max(0) as usize).map(|f| f.0).unwrap_or("whole");
        match drawings::import(&path, fit, page.get_invert()) {
            Ok(imported) => {
                let st = page_state();
                let mut s = st.borrow_mut();
                stop_playing(app, &mut s);
                remember(&mut s);
                let count = imported.frames.len();
                s.drawing = imported;
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
        let drawing = st.borrow().drawing.clone();
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
                show(app, format!("Saved {name}. The Picture app on the System monitor page can show it now."));
            }
            Err(e) => show(app, format!("Couldn't save: {e}")),
        }
    }));
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
                s.saved = Some(drawing.clone());
                s.drawing = drawing;
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
        s.drawing = drawings::new_drawing();
        s.saved = Some(s.drawing.clone());
        s.index = 0;
        s.name.clear();
        s.undo.clear();
        s.redo.clear();
        app.global::<DrawPage>().set_name("".into());
        redraw(app, &s, None);
    }));
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
    page.on_show_folder(|| {
        let dir = drawings::folder();
        let _ = std::fs::create_dir_all(&dir);
        crate::app::open_path(&dir.display().to_string());
    });
}
