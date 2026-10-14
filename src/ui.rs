//! The windows: one for each corner a stack is in, while it has
//! anything in it, and the history's box when it is asked for.
//!
//! On X11 a stack's window places itself, over everything, out of the
//! window manager's hands; it is cut to what is drawn in it, so that
//! the rest of it can be clicked through, and where nothing sees through
//! windows, what is behind it is drawn into it from a picture taken as
//! it opened. On Wayland a window cannot say where it goes: GNOME's
//! shell, with toast's extension in it, puts it in its corner, by its
//! title, and keeps it there as it grows and shrinks.

use std::env;
use std::num::NonZeroU32;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::{WindowAttributesExtX11, WindowType};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Icon as WindowIcon, Window, WindowAttributes, WindowId, WindowLevel};

use crate::canvas::{Canvas, Text};
use crate::config::{self, Settings};
use crate::dbus::Writer;
use crate::icons::{self, Icons};
use crate::note::{Note, CRITICAL};
use crate::panel::{Asked, Panel};
use crate::server::{self, Event, Shared};
use crate::stack::{Did, Look, Reason, Stack};
use crate::x11::X11;
use crate::history;

/// A window and what is drawn into it.
struct View {
    window: Arc<Window>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    /// The window's number, on X11.
    x11: Option<u32>,
    /// What was on the screen behind it as it opened, where nothing sees
    /// through windows.
    behind: Vec<u32>,
    /// Whether it has been put on the screen yet: on X11 not until it is
    /// cut to its shape, so that it never shows as a whole rectangle.
    mapped: bool,
}

impl View {
    fn open(event_loop: &ActiveEventLoop, attributes: WindowAttributes, x11: Option<&X11>, compositing: bool, area: Option<(i32, i32, i32, i32)>) -> Option<View> {
        let window = Arc::new(event_loop.create_window(attributes).ok()?);
        let surface = Context::new(window.clone()).and_then(|context| Surface::new(&context, window.clone())).ok()?;
        let id = match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Xlib(handle)) => Some(handle.window as u32),
            Ok(RawWindowHandle::Xcb(handle)) => Some(handle.window.get()),
            _ => None,
        };
        let behind = match (x11, compositing, area) {
            (Some(x11), false, Some(area)) => x11.picture(area),
            _ => Vec::new(),
        };
        Some(View { window, surface, x11: id, behind, mapped: false })
    }

    /// Draws with `paint`, then cuts the window to what was drawn, or puts
    /// what was behind it under it, and shows it. `keep` is a part of the
    /// window to keep whole, drawn in or not.
    fn draw(&mut self, x11: Option<&X11>, compositing: bool, keep: Option<(i32, i32, i32, i32)>, paint: impl FnOnce(&mut Canvas)) {
        let size = self.window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if self.surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = self.surface.buffer_mut() else { return };
        let mut canvas = Canvas::new(&mut buffer, size.width, size.height);
        paint(&mut canvas);
        if let (Some(x11), Some(id)) = (x11, self.x11) {
            match compositing {
                // Seen through where nothing is drawn; clicked through
                // where little is.
                true => x11.cut(id, &canvas.solid_runs(100, keep), false),
                false => x11.cut(id, &canvas.over_picture(&self.behind, keep), true),
            }
        }
        let _ = buffer.present();
        if !self.mapped {
            self.mapped = true;
            self.window.set_visible(true);
            // What was drawn before it was on the screen was not kept.
            self.window.request_redraw();
        }
    }
}

/// A corner's stack, and its window while it has anything in it.
struct Corner {
    stack: Stack,
    view: Option<View>,
    /// How tall its window is to be, on Wayland, where it grows and
    /// shrinks with the stack.
    tall: i32,
}

struct History {
    panel: Panel,
    view: Option<View>,
    focused: bool,
}

pub struct App {
    settings: Settings,
    look: Look,
    text: Text,
    icons: Icons,
    corners: Vec<Corner>,
    history: Option<History>,
    x11: Option<X11>,
    compositing: bool,
    writer: Writer,
    shared: Arc<Shared>,
    modifiers: ModifiersState,
    /// The last notifications kept, counted, to trim the history now and then.
    kept: usize,
}

pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::<Event>::with_user_event().build().map_err(|e| format!("no display to show notifications on: {e}"))?;
    let (writer, shared) = server::start(event_loop.create_proxy())?;
    let settings = Settings::load(config::path());
    history::trim(settings.config.keep);
    // On Wayland, the desktop places windows, and an X11 connection
    // would only be to its stand-in for X11 programs.
    let x11 = if env::var_os("WAYLAND_DISPLAY").is_some() { None } else { X11::open() };
    let compositing = x11.as_ref().is_some_and(X11::compositing);
    let look = match &x11 {
        Some(x11) => x11.area_in_use().map_or(Look::for_monitor(1080, 1.0), |(_, height)| Look::for_monitor(height, x11.text_scale())),
        None => Look::for_monitor(1080, 1.0),
    };
    let mut app = App {
        settings,
        look,
        text: Text::load(),
        icons: Icons::default(),
        corners: Vec::new(),
        history: None,
        x11,
        compositing,
        writer,
        shared,
        modifiers: ModifiersState::empty(),
        kept: 0,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

/// toast's own icon, for the desktops that show a window's.
fn window_icon() -> Option<WindowIcon> {
    let icon = icons::own()?;
    let rgba = icon.pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
    WindowIcon::from_rgba(rgba, icon.width as u32, icon.height as u32).ok()
}

/// Runs a line of shell, apart from toast, which does not wait for it.
fn spawn(line: &str, id: u32, key: &str) {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(line).env("TOAST_ID", id.to_string()).env("TOAST_ACTION", key);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    if let Ok(mut child) = command.spawn() {
        // Waited for on a thread of its own, so that it does not linger.
        std::thread::spawn(move || child.wait());
    }
}

/// Puts words on the clipboard, with whatever there is to do it.
fn copy(words: &str) {
    let wayland = env::var_os("WAYLAND_DISPLAY").is_some();
    let (program, arguments): (&str, &[&str]) = if wayland { ("wl-copy", &[]) } else { ("xclip", &["-selection", "clipboard"]) };
    let Ok(mut child) = Command::new(program).args(arguments).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return };
    if let Some(mut input) = child.stdin.take() {
        use std::io::Write;
        let _ = input.write_all(words.as_bytes());
    }
    std::thread::spawn(move || child.wait());
}

impl App {
    fn most(&self) -> usize {
        self.settings.config.stack.max(1)
    }

    /// Something came of the pointer, or of time: the program is told,
    /// and what toast was to run for an action, it runs.
    fn did(&mut self, did: Vec<Did>) {
        for did in did {
            match did {
                Did::Action(id, key) => {
                    let run = self.corners.iter().flat_map(|c| &c.stack.toasts).find(|t| t.note.id == id).and_then(|t| t.note.run.iter().find(|(k, _)| *k == key).map(|(_, line)| line.clone()));
                    if let Some(line) = run {
                        spawn(&line, id, &key);
                    }
                    server::invoked(&self.writer, id, &key);
                }
                Did::Gone(id, reason) => server::closed(&self.writer, id, reason),
            }
        }
    }

    fn notify(&mut self, mut note: Note, event_loop: &ActiveEventLoop) {
        let config = self.settings.fresh().clone();
        let treat = config.treat(&note);
        note.kind = treat.kind;
        if treat.keep {
            history::add(&note.record());
            self.kept += 1;
            if self.kept % 100 == 0 {
                history::trim(config.keep);
            }
            if let Some(history) = &mut self.history {
                history.panel.set_records(history::all());
                if let Some(view) = &history.view {
                    view.window.request_redraw();
                }
            }
        }
        let quiet = self.shared.quiet.load(Ordering::Relaxed) && note.urgency != CRITICAL;
        if !treat.show || quiet {
            // Kept, and not shown: as far as the program is concerned,
            // it came and went.
            server::closed(&self.writer, note.id, Reason::Expired);
            return;
        }
        let look = self.look;
        let at = match self.corners.iter().position(|c| c.stack.holds(note.id)) {
            Some(at) => at,
            None => match self.corners.iter().position(|c| c.stack.corner == treat.corner) {
                Some(at) => at,
                None => {
                    self.corners.push(Corner { stack: Stack::new(treat.corner), view: None, tall: 0 });
                    self.corners.len() - 1
                }
            },
        };
        self.corners[at].stack.add(note, treat, &look, &mut self.text, &mut self.icons);
        self.open(at, event_loop);
    }

    /// Opens the window of a stack that has none.
    fn open(&mut self, at: usize, event_loop: &ActiveEventLoop) {
        if self.corners[at].view.is_some() {
            if let Some(view) = &self.corners[at].view {
                view.window.request_redraw();
            }
            return;
        }
        let corner = self.corners[at].stack.corner;
        // The letters are sized for the monitor in use, when nothing else
        // is on the screen to be the wrong size beside them.
        let alone = self.corners.iter().all(|c| c.view.is_none());
        if alone {
            if let Some((_, height)) = self.x11.as_ref().and_then(X11::area_in_use) {
                let scale = self.x11.as_ref().map_or(1.0, X11::text_scale);
                self.look = Look::for_monitor(height, scale);
            } else if let Some(monitor) = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next()) {
                self.look = Look::for_monitor(monitor.size().height as i32, monitor.scale_factor());
            }
            let look = self.look;
            for c in &mut self.corners {
                c.stack.relay(&look, &mut self.text, &mut self.icons);
            }
        }
        let look = self.look;
        let width = look.width + 2 * look.margin;
        let base = Window::default_attributes()
            .with_title(format!("toast {}", corner.name()))
            .with_window_icon(window_icon())
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let base = WindowAttributesExtX11::with_name(base, "toast", "toast");
        let base = WindowAttributesExtWayland::with_name(base, "toast", "toast");
        let (attributes, area) = match &self.x11 {
            Some(x11) => {
                let (area, _) = x11.area_in_use().unwrap_or(((0, 0, 1920, 1080), 1080));
                let height = (look.margin * 2 + self.most() as i32 * (look.tallest() + look.gap) + look.peek * 2).min(area.3);
                let x = area.0 + match corner.side() {
                    -1 => 0,
                    0 => (area.2 - width) / 2,
                    _ => area.2 - width,
                };
                let y = if corner.top() { area.1 } else { area.1 + area.3 - height };
                let attributes = base
                    .with_override_redirect(true)
                    .with_x11_window_type(vec![WindowType::Notification])
                    .with_transparent(self.compositing)
                    .with_visible(false)
                    .with_position(PhysicalPosition::new(x, y))
                    .with_inner_size(PhysicalSize::new(width as u32, height as u32));
                (attributes, Some((x, y, width, height)))
            }
            None => {
                let tall = self.corners[at].stack.reach() + 2 * look.margin;
                self.corners[at].tall = tall;
                (base.with_transparent(true).with_inner_size(PhysicalSize::new(width as u32, tall.max(1) as u32)), None)
            }
        };
        self.corners[at].view = View::open(event_loop, attributes, self.x11.as_ref(), self.compositing, area);
        if let Some(view) = &self.corners[at].view {
            view.window.request_redraw();
        }
    }

    fn toggle_history(&mut self, event_loop: &ActiveEventLoop) {
        if self.history.take().is_some() {
            return;
        }
        let mut panel = Panel::new(self.look, history::all());
        panel.quiet = self.shared.quiet.load(Ordering::Relaxed);
        let (width, height) = panel.size();
        let base = Window::default_attributes()
            .with_title("toast history")
            .with_window_icon(window_icon())
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(width as u32, height as u32));
        let base = WindowAttributesExtX11::with_name(base, "toast", "toast");
        let base = WindowAttributesExtWayland::with_name(base, "toast", "toast");
        let (attributes, area) = match &self.x11 {
            // Across the middle of the monitor in use, a fifth of the way
            // down, as clutch's box is.
            Some(x11) => {
                let (area, _) = x11.area_in_use().unwrap_or(((0, 0, 1920, 1080), 1080));
                let (x, y) = (area.0 + (area.2 - width) / 2, area.1 + area.3 / 5);
                let attributes = base
                    .with_x11_window_type(vec![WindowType::Dialog])
                    .with_transparent(self.compositing)
                    .with_visible(false)
                    .with_position(PhysicalPosition::new(x, y));
                (attributes, Some((x, y, width, height)))
            }
            None => (base.with_transparent(true), None),
        };
        let view = View::open(event_loop, attributes, self.x11.as_ref(), self.compositing, area);
        if let Some(view) = &view {
            view.window.request_redraw();
        }
        self.history = Some(History { panel, view, focused: false });
    }

    fn asked(&mut self, asked: Option<Asked>) {
        match asked {
            None => {}
            Some(Asked::Close) => self.history = None,
            Some(Asked::Copy(words)) => {
                copy(&words);
                self.history = None;
            }
            Some(Asked::Forget(id)) => {
                history::forget(id);
            }
        }
        if let Some(view) = self.history.as_ref().and_then(|h| h.view.as_ref()) {
            view.window.request_redraw();
        }
    }

    fn history_event(&mut self, event: WindowEvent) {
        let Some(history) = &mut self.history else { return };
        let mut asked = None;
        match event {
            WindowEvent::CloseRequested => asked = Some(Asked::Close),
            WindowEvent::RedrawRequested => {
                let (panel, text, icons) = (&mut history.panel, &mut self.text, &mut self.icons);
                if let Some(view) = &mut history.view {
                    view.draw(self.x11.as_ref(), self.compositing, None, |canvas| panel.draw(canvas, text, icons));
                    // On X11 it is given the keyboard once it is there.
                    if view.mapped && !history.focused {
                        view.window.focus_window();
                    }
                }
                if self.text.waiting() {
                    if let Some(view) = &history.view {
                        view.window.request_redraw();
                    }
                }
                return;
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if event.state == ElementState::Pressed => {
                let name = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Space) => " ".to_string(),
                    Key::Named(named) => format!("{named:?}"),
                    Key::Character(letters) => letters.to_string(),
                    _ => String::new(),
                };
                asked = history.panel.key(&name, self.modifiers.control_key());
            }
            WindowEvent::CursorMoved { position, .. } => history.panel.point(Some((position.x as i32, position.y as i32))),
            WindowEvent::CursorLeft { .. } => history.panel.point(None),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => asked = history.panel.click(),
            WindowEvent::MouseWheel { delta, .. } => {
                let down = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => lines < 0.0,
                    MouseScrollDelta::PixelDelta(by) => by.y < 0.0,
                };
                history.panel.wheel(down);
            }
            // Clicked away from: it was not wanted after all.
            WindowEvent::Focused(focused) => {
                if history.focused && !focused {
                    asked = Some(Asked::Close);
                }
                history.focused |= focused;
            }
            _ => return,
        }
        self.asked(asked);
    }

    fn stack_event(&mut self, at: usize, event: WindowEvent) {
        let look = self.look;
        let corner = &mut self.corners[at];
        let mut did = Vec::new();
        let mut redraw = true;
        match event {
            WindowEvent::RedrawRequested => {
                let keep = if corner.stack.expanded() { corner.stack.bounds() } else { None };
                let shadows = self.compositing || self.x11.is_none();
                let (stack, text, icons) = (&mut corner.stack, &mut self.text, &mut self.icons);
                if let Some(view) = &mut corner.view {
                    view.draw(self.x11.as_ref(), self.compositing, keep, |canvas| stack.draw(canvas, text, icons, &look, shadows));
                }
                redraw = false;
            }
            WindowEvent::CursorMoved { position, .. } => redraw = corner.stack.point(Some((position.x as i32, position.y as i32)), &look),
            WindowEvent::CursorLeft { .. } => redraw = corner.stack.point(None, &look),
            WindowEvent::MouseInput { state, button, .. } => {
                did = match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => corner.stack.press(1),
                    (ElementState::Pressed, MouseButton::Middle) => corner.stack.press(2),
                    (ElementState::Pressed, MouseButton::Right) => corner.stack.press(3),
                    (ElementState::Released, MouseButton::Left) => corner.stack.release(&look),
                    _ => Vec::new(),
                };
            }
            _ => redraw = false,
        }
        if redraw {
            if let Some(view) = &corner.view {
                view.window.request_redraw();
            }
        }
        self.did(did);
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Note(note) => self.notify(*note, event_loop),
            Event::Close(id) => {
                for corner in &mut self.corners {
                    if corner.stack.remove(id, Reason::Closed) {
                        server::closed(&self.writer, id, Reason::Closed);
                    }
                }
            }
            Event::Dismiss(id) => {
                let did: Vec<Did> = self.corners.iter_mut().flat_map(|c| c.stack.dismiss(id)).collect();
                self.did(did);
            }
            Event::Panel => self.toggle_history(event_loop),
            Event::Quiet(quiet) => {
                if let Some(history) = &mut self.history {
                    history.panel.quiet = quiet;
                }
            }
            Event::Forgot => {
                if let Some(history) = &mut self.history {
                    history.panel.set_records(history::all());
                }
            }
        }
        if let Some(view) = self.history.as_ref().and_then(|h| h.view.as_ref()) {
            view.window.request_redraw();
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.history.as_ref().and_then(|h| h.view.as_ref()).is_some_and(|v| v.window.id() == id) {
            return self.history_event(event);
        }
        if let Some(at) = self.corners.iter().position(|c| c.view.as_ref().is_some_and(|v| v.window.id() == id)) {
            self.stack_event(at, event);
        }
    }

    /// Time moves on for every stack: what ran out goes, and what moves
    /// is drawn again; then a wait, as long as nothing needs doing.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let (look, most) = (self.look, self.most());
        let mut moving = self.text.waiting();
        let mut soonest: Option<f64> = None;
        let mut did = Vec::new();
        for corner in &mut self.corners {
            let (gone, still, next) = corner.stack.tick(&look, most);
            did.extend(gone);
            moving |= still;
            soonest = match (soonest, next) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if still || self.text.waiting() {
                if let Some(view) = &corner.view {
                    view.window.request_redraw();
                }
            }
            // On Wayland the window grows and shrinks with the stack.
            if self.x11.is_none() {
                if let Some(view) = &corner.view {
                    let tall = (corner.stack.reach() + 2 * look.margin).max(1);
                    if tall != corner.tall {
                        corner.tall = tall;
                        let _ = view.window.request_inner_size(PhysicalSize::new((look.width + 2 * look.margin) as u32, tall as u32));
                        view.window.request_redraw();
                    }
                }
            }
        }
        self.did(did);
        // A stack with nothing left in it has no window.
        for corner in &mut self.corners {
            if corner.stack.is_empty() {
                corner.view = None;
            }
        }
        self.corners.retain(|c| !c.stack.is_empty());
        let showing: usize = self.corners.iter().map(|c| c.stack.toasts.iter().filter(|t| t.leaving.is_none()).count()).sum();
        self.shared.showing.store(showing as u32, Ordering::Relaxed);
        let now = Instant::now();
        let flow = if moving {
            ControlFlow::WaitUntil(now + Duration::from_millis(16))
        } else if let Some(ms) = soonest {
            ControlFlow::WaitUntil(now + Duration::from_millis(ms.max(1.0) as u64))
        } else if !self.corners.is_empty() {
            // "now" becomes "1m" in a while.
            ControlFlow::WaitUntil(now + Duration::from_secs(20))
        } else {
            ControlFlow::Wait
        };
        event_loop.set_control_flow(flow);
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if let winit::event::StartCause::ResumeTimeReached { .. } = cause {
            for view in self.corners.iter().filter_map(|c| c.view.as_ref()) {
                view.window.request_redraw();
            }
        }
    }
}
