//! A stack of notifications in one corner, as shadcn's toasts are: the
//! newest in front, the two before it peeking out behind it, smaller,
//! and with the pointer over them, all of them laid out in a column, held
//! until the pointer leaves. Each one comes in from the edge, and goes by
//! fading, or by being swept off to the side.
//!
//! Everything that moves moves toward where it should be by a part of
//! the way that is left each frame, which is quick at first and slows as
//! it arrives. Where everything should be is worked out afresh each
//! frame, so that whatever happens meanwhile, all of it stays smooth.

use std::time::Instant;

use crate::canvas::{icons, mix, Canvas, Card, Icon, Text};
use crate::config::Treatment;
use crate::icons::Icons;
use crate::note::{Corner, Kind, Note, CRITICAL};

pub const TEXT: u32 = 0xEEEEEE;
pub const DIM: u32 = 0x9A9A9A;
pub const PANEL: u32 = 0x1E1E1E;
pub const EDGE: u32 = 0x3A3A3A;
pub const RULE: u32 = 0x333333;
pub const WELL: u32 = 0x2A2A2A;
pub const ACCENT: u32 = 0x3E63DD;
pub const ON_ACCENT: u32 = 0xFFFFFF;
pub const GREEN: u32 = 0x1FBF83;
pub const BLUE: u32 = 0x2F8BFF;
pub const AMBER: u32 = 0xFFA51F;
pub const RED: u32 = 0xE5484D;

/// How long most of a move takes: after this, a little over a third of
/// the way is left.
const EASE: f64 = 0.075;
/// How much smaller each card behind the front one is.
const SHRINK: f64 = 0.05;

/// Sizes, all from the size of the letters, which is from the monitor.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// The summary's letters, the body's, and the smallest.
    pub px: u32,
    pub small: u32,
    pub tiny: u32,
    pub pad: i32,
    /// A card across.
    pub width: i32,
    pub radius: f64,
    /// Between cards laid out in a column; and how far each behind the
    /// front one peeks out.
    pub gap: i32,
    pub peek: i32,
    pub icon: i32,
    /// From the edge of the screen to the cards: room for them to come in
    /// from, and for their shadows.
    pub margin: i32,
}

impl Look {
    /// For a monitor this tall, letters as large as clutch's box has on
    /// it; but not much larger than the desktop's own, which are `scale`
    /// times plain.
    pub fn for_monitor(height: i32, scale: f64) -> Look {
        let base = (height / 90).min((16.0 * scale) as i32).clamp(14, 30);
        Look {
            px: (base * 7 / 8) as u32,
            small: (base * 13 / 16) as u32,
            tiny: (base * 3 / 4) as u32,
            pad: base,
            width: base * 22,
            radius: f64::from(base) / 2.0,
            gap: base * 7 / 8,
            peek: base * 7 / 8,
            icon: base * 5 / 4,
            margin: base * 3 / 2,
        }
    }

    /// The tallest a card can be: two lines of summary, four of body, a
    /// bar and a row of buttons.
    pub fn tallest(&self) -> i32 {
        self.pad * 2 + 2 * Text::line_height(self.px) + 4 * Text::line_height(self.small) + self.pad + self.button() + self.pad
    }

    pub fn button(&self) -> i32 {
        Text::line_height(self.small) + self.pad / 2
    }

    /// Where the words begin on a card, with an icon before them or not.
    fn text_left(&self, icon: bool) -> i32 {
        if icon { self.pad + self.icon + self.pad * 3 / 4 } else { self.pad }
    }
}

/// A value on its way to another.
#[derive(Clone, Copy, Debug, Default)]
struct Move {
    now: f64,
    to: f64,
}

impl Move {
    fn at(value: f64) -> Move {
        Move { now: value, to: value }
    }

    /// Goes the part of the way that `seconds` takes it; says whether it
    /// is still on its way.
    fn step(&mut self, seconds: f64) -> bool {
        let left = self.to - self.now;
        if left.abs() < 0.01 {
            self.now = self.to;
            return false;
        }
        self.now += left * (1.0 - (-seconds / EASE).exp());
        true
    }
}

/// Why a notification went, as the programs are told: it ran out of
/// time, it was dismissed, or it was taken back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

/// What came of the pointer, for the server to tell.
#[derive(Clone, Debug, PartialEq)]
pub enum Did {
    Action(u32, String),
    Gone(u32, Reason),
}

/// How a card's words fall, worked out once for each notification.
#[derive(Clone, Debug, Default)]
struct Laid {
    height: i32,
    title: Vec<String>,
    body: Vec<String>,
    /// Each button's words, and how wide it is.
    buttons: Vec<(String, String, i32)>,
}

/// A rectangle: left, top, across, down.
pub type Rect = (i32, i32, i32, i32);

fn holds((x, y, w, h): Rect, (px, py): (i32, i32)) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

pub struct Toast {
    pub note: Note,
    pub treat: Treatment,
    laid: Laid,
    /// What of its time is left, in milliseconds; none if it stays.
    left: Option<f64>,
    pub leaving: Option<Reason>,
    /// From the edge to its nearer side; its height; how large it is;
    /// how opaque, and its words; how far it is swept to the side.
    off: Move,
    height: Move,
    size: Move,
    alpha: Move,
    words: Move,
    swept: Move,
    /// What was drawn where, the last time: the card, its close button,
    /// its buttons with their keys.
    at: Rect,
    close: Rect,
    buttons: Vec<(String, Rect)>,
    /// Whether it is new and not yet placed, which is where it comes in from.
    new: bool,
    /// Whether its time is yet to begin: the wait before it came is not its.
    fresh: bool,
}

impl Toast {
    fn lay(&mut self, look: &Look, text: &mut Text, icons: &mut Icons) {
        let icon = has_icon(&self.note, self.treat.kind, icons);
        let words = look.width - look.text_left(icon) - look.pad;
        let meta = text.width(&meta(&self.note), look.tiny) + look.pad / 2;
        let mut title = text.wrapped(&self.note.summary, words - meta, look.px, 2);
        if title.is_empty() {
            title.push(self.note.app.clone());
        }
        let body = text.wrapped(&self.note.body, words, look.small, 4);
        let mut height = look.pad + title.len() as i32 * Text::line_height(look.px) + body.len() as i32 * Text::line_height(look.small);
        if self.note.progress.is_some() {
            height += look.pad * 3 / 4 + look.pad / 4;
        }
        let mut buttons = Vec::new();
        let mut across = 0;
        for (key, label) in self.note.buttons().take(3) {
            let room = (words - across).max(look.pad * 3);
            let label = text.clipped(label, room - look.pad * 3 / 2, look.small);
            let wide = text.width(&label, look.small) + look.pad * 3 / 2;
            across += wide + look.pad / 2;
            buttons.push((key.clone(), label, wide));
        }
        if !buttons.is_empty() {
            height += look.pad * 3 / 4 + look.button();
        }
        height += look.pad;
        // Never shorter than the icon needs.
        self.laid = Laid { height: height.max(look.pad * 2 + look.icon), title, body, buttons };
    }
}

/// "Firefox · 2m": who it is from and when, as small as it gets.
fn meta(note: &Note) -> String {
    let ago = ago(note.time);
    match note.app.trim() {
        "" => ago,
        app => format!("{app} \u{b7} {ago}"),
    }
}

/// How long ago, in as few letters as will do.
pub fn ago(time: u64) -> String {
    let seconds = crate::note::now().saturating_sub(time);
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// The icon a kind of news is drawn with, in its color.
fn kind_icon(kind: Kind) -> Option<(char, u32)> {
    match kind {
        Kind::Plain => None,
        Kind::Success => Some((icons::CHECK_FILLED, GREEN)),
        Kind::Info => Some((icons::INFO_FILLED, BLUE)),
        Kind::Warning => Some((icons::ALERT_FILLED, AMBER)),
        Kind::Error => Some((icons::DANGER_FILLED, RED)),
    }
}

fn has_icon(note: &Note, kind: Kind, icons: &mut Icons) -> bool {
    kind != Kind::Plain || note.image.is_some() || icons.get(&note.icon).is_some()
}

pub struct Stack {
    pub corner: Corner,
    /// The newest first.
    pub toasts: Vec<Toast>,
    /// Laid out in a column, the pointer being over them.
    expanded: bool,
    pointer: Option<(i32, i32)>,
    /// Where a button went down, on which notification, and whether it
    /// has moved far enough to be sweeping it rather than clicking.
    press: Option<((i32, i32), u32, bool)>,
    last: Option<Instant>,
}

impl Stack {
    pub fn new(corner: Corner) -> Stack {
        Stack { corner, toasts: Vec::new(), expanded: false, pointer: None, press: None, last: None }
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    pub fn holds(&self, id: u32) -> bool {
        self.toasts.iter().any(|t| t.note.id == id && t.leaving.is_none())
    }

    /// Puts a notification in front, or where it replaces one, in its place.
    pub fn add(&mut self, note: Note, treat: Treatment, look: &Look, text: &mut Text, icons: &mut Icons) {
        let left = treat.lasts.map(|ms| ms as f64);
        if let Some(toast) = self.toasts.iter_mut().find(|t| t.note.id == note.id && t.leaving.is_none()) {
            (toast.note, toast.treat, toast.left, toast.fresh) = (note, treat, left, true);
            toast.lay(look, text, icons);
            return;
        }
        let mut toast = Toast {
            note,
            treat,
            laid: Laid::default(),
            left,
            leaving: None,
            off: Move::default(),
            height: Move::default(),
            size: Move::at(1.0),
            alpha: Move::default(),
            words: Move::default(),
            swept: Move::default(),
            at: (0, 0, 0, 0),
            close: (0, 0, 0, 0),
            buttons: Vec::new(),
            new: true,
            fresh: true,
        };
        toast.lay(look, text, icons);
        self.toasts.insert(0, toast);
    }

    /// Sends one on its way out; says whether it was here to.
    pub fn remove(&mut self, id: u32, reason: Reason) -> bool {
        match self.toasts.iter_mut().find(|t| t.note.id == id && t.leaving.is_none()) {
            Some(toast) => {
                toast.leaving = Some(reason);
                true
            }
            None => false,
        }
    }

    /// Lays every card out again, for letters of another size.
    pub fn relay(&mut self, look: &Look, text: &mut Text, icons: &mut Icons) {
        self.toasts.iter_mut().for_each(|t| t.lay(look, text, icons));
    }

    /// Moves time on: counts down what is shown and not held, and moves
    /// everything toward where it should be. What ran out of time, and
    /// whether anything is still moving, and how long until the next to
    /// run out of time does, in milliseconds.
    pub fn tick(&mut self, look: &Look, most: usize) -> (Vec<Did>, bool, Option<f64>) {
        let now = Instant::now();
        let elapsed = self.last.map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        // A long wait is not a long step: what moves goes on from where it was.
        let seconds = elapsed.min(0.1);
        self.last = Some(now);
        let mut did = Vec::new();
        // Only those in view count down, and none while the pointer holds them.
        if !self.expanded {
            let mut shown = 0;
            for toast in self.toasts.iter_mut().filter(|t| t.leaving.is_none()) {
                shown += 1;
                if shown > most {
                    break;
                }
                if std::mem::take(&mut toast.fresh) {
                    continue;
                }
                if let Some(left) = &mut toast.left {
                    *left -= elapsed * 1000.0;
                    if *left <= 0.0 {
                        toast.leaving = Some(Reason::Expired);
                        did.push(Did::Gone(toast.note.id, Reason::Expired));
                    }
                }
            }
        }
        self.aim(look, most);
        let mut moving = false;
        for toast in &mut self.toasts {
            for value in [&mut toast.off, &mut toast.height, &mut toast.size, &mut toast.alpha, &mut toast.words, &mut toast.swept] {
                moving |= value.step(seconds);
            }
        }
        // Gone once it has faded.
        self.toasts.retain(|t| t.leaving.is_none() || t.alpha.now > 0.01);
        let shown = self.toasts.iter().filter(|t| t.leaving.is_none()).take(most);
        let soonest = if self.expanded { None } else { shown.filter_map(|t| t.left).reduce(f64::min) };
        (did, moving, soonest)
    }

    /// Moves everything to where it is going, as if a while had passed in
    /// which nothing ran out of time: for pictures of it.
    pub fn settle(&mut self, look: &Look, most: usize) {
        for _ in 0..120 {
            self.last = Some(Instant::now() - std::time::Duration::from_millis(16));
            self.toasts.iter_mut().for_each(|t| t.fresh = true);
            let _ = self.tick(look, most);
        }
    }

    /// Says where each card should be, for the stack as it is now.
    fn aim(&mut self, look: &Look, most: usize) {
        let front = self.toasts.iter().find(|t| t.leaving.is_none()).map_or(0, |t| t.laid.height);
        let (mut index, mut before) = (0usize, 0.0);
        let expanded = self.expanded;
        for toast in &mut self.toasts {
            let own = f64::from(toast.laid.height);
            if toast.leaving.is_some() {
                // It fades where it is, a little smaller, unless swept.
                toast.alpha.to = 0.0;
                toast.size.to = toast.size.now.min(1.0) * 0.96;
                continue;
            }
            let shown = index < most;
            let depth = index.min(most.saturating_sub(1)) as f64;
            if expanded {
                toast.off.to = before;
                toast.height.to = own;
                toast.size.to = 1.0;
                toast.words.to = 1.0;
                before += own + f64::from(look.gap);
            } else {
                let size = 1.0 - SHRINK * depth;
                let height = if index == 0 { own } else { f64::from(front) * size };
                toast.size.to = size;
                toast.height.to = height;
                // Its far side a peek beyond the front one's.
                toast.off.to = f64::from(front) + depth * f64::from(look.peek) - height;
                toast.words.to = if index == 0 { 1.0 } else { 0.0 };
            }
            toast.alpha.to = if shown { 1.0 } else { 0.0 };
            if self.press.is_none_or(|(_, id, _)| id != toast.note.id) {
                toast.swept.to = 0.0;
            }
            if toast.new {
                // It comes in from beyond the edge, as tall as it will be.
                toast.new = false;
                toast.height = Move::at(toast.height.to);
                toast.size = Move::at(toast.size.to);
                toast.off = Move { now: -own * 0.8, to: toast.off.to };
                toast.words = Move { now: toast.words.to, to: toast.words.to };
            }
            index += 1;
        }
    }

    /// How tall the stack is at most, now or where it is going: the room
    /// a window needs to hold it.
    pub fn reach(&self) -> i32 {
        let far = |t: &Toast| (t.off.now.max(t.off.to) + t.height.now.max(t.height.to)).ceil() as i32;
        self.toasts.iter().map(far).max().unwrap_or(0)
    }

    /// The card's place in a window `width` by `height`.
    fn place(&self, toast: &Toast, look: &Look, width: i32, height: i32) -> (f64, f64, f64, f64) {
        let (w, h) = (f64::from(look.width) * toast.size.now, toast.height.now);
        let x = f64::from(width) / 2.0 - w / 2.0 + toast.swept.now;
        let y = match self.corner.top() {
            true => f64::from(look.margin) + toast.off.now,
            false => f64::from(height - look.margin) - toast.off.now - h,
        };
        (x, y, w, h)
    }

    /// The part of the window the stack takes up, laid out: the pointer
    /// in it keeps it so.
    pub fn bounds(&self) -> Option<Rect> {
        let shown: Vec<Rect> = self.toasts.iter().filter(|t| t.leaving.is_none() && t.alpha.to > 0.0).map(|t| t.at).collect();
        let left = shown.iter().map(|r| r.0).min()?;
        let top = shown.iter().map(|r| r.1).min()?;
        let right = shown.iter().map(|r| r.0 + r.2).max()?;
        let bottom = shown.iter().map(|r| r.1 + r.3).max()?;
        Some((left, top, right - left, bottom - top))
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Draws the stack, the cards furthest back first.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, canvas: &mut Canvas, text: &mut Text, icons: &mut Icons, look: &Look, shadows: bool) {
        canvas.clear();
        let (width, height) = (canvas.width, canvas.height);
        let hovered = self.pointer.and_then(|p| self.toasts.iter().find(|t| t.leaving.is_none() && holds(t.at, p)).map(|t| t.note.id));
        let places: Vec<_> = self.toasts.iter().map(|t| self.place(t, look, width, height)).collect();
        for (toast, &(x, y, w, h)) in self.toasts.iter_mut().zip(&places).rev() {
            let alpha = toast.alpha.now.clamp(0.0, 1.0) * (1.0 - (toast.swept.now.abs() / f64::from(look.width)).min(1.0) * 0.8);
            if alpha <= 0.0 {
                toast.at = (0, 0, 0, 0);
                continue;
            }
            if shadows {
                canvas.shadow(x, y, w, h, look.radius, f64::from(look.pad) / 4.0, f64::from(look.pad) * 1.25, 0.3 * alpha);
            }
            let edge = if toast.note.urgency == CRITICAL { mix(EDGE, RED, 150) } else { EDGE };
            canvas.card(&Card { x, y, w, h, radius: look.radius, fill: PANEL, edge, alpha });
            toast.at = (x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32);
            let words = toast.words.now.clamp(0.0, 1.0) * alpha;
            if words > 0.01 {
                canvas.clip(Some(toast.at));
                draw_words(canvas, text, icons, look, toast, (x.round() as i32, y.round() as i32), words, hovered == Some(toast.note.id), self.pointer);
                canvas.clip(None);
            } else {
                toast.close = (0, 0, 0, 0);
                toast.buttons.clear();
            }
        }
    }

    /// The pointer moved to `at`, or left; says whether to draw again.
    pub fn point(&mut self, at: Option<(i32, i32)>, look: &Look) -> bool {
        self.pointer = at;
        let mut changed = false;
        if let (Some((start, id, sweeping)), Some(at)) = (self.press, at) {
            let across = at.0 - start.0;
            if !sweeping && across.abs() > look.pad / 3 {
                self.press = Some((start, id, true));
            }
            if let Some(toast) = self.toasts.iter_mut().find(|t| t.note.id == id) {
                if self.press.is_some_and(|p| p.2) {
                    // Swept the way it would go easily; the other way, grudgingly.
                    let away = if self.corner.side() < 0 { -1 } else { 1 };
                    let across = f64::from(across);
                    let free = self.corner.side() == 0 || across * f64::from(away) > 0.0;
                    toast.swept = Move::at(if free { across } else { across / 4.0 });
                    changed = true;
                }
            }
        }
        // Held open while the pointer is over the cards, or in what
        // they take up laid out.
        let inside = at.is_some_and(|p| self.toasts.iter().any(|t| t.leaving.is_none() && holds(t.at, p)) || self.expanded && self.bounds().is_some_and(|b| holds(b, p)));
        let expanded = inside || self.press.is_some();
        if expanded != self.expanded {
            self.expanded = expanded;
            changed = true;
        }
        changed || at.is_some()
    }

    /// A button went down: 1 the left, 2 the middle, 3 the right.
    pub fn press(&mut self, button: u8) -> Vec<Did> {
        let Some(at) = self.pointer else { return Vec::new() };
        let Some(toast) = self.toasts.iter().find(|t| t.leaving.is_none() && holds(t.at, at)) else { return Vec::new() };
        let id = toast.note.id;
        match button {
            1 => {
                self.press = Some((at, id, false));
                Vec::new()
            }
            // The right button dismisses one, the middle all of them.
            3 => self.dismiss(Some(id)),
            2 => self.dismiss(None),
            _ => Vec::new(),
        }
    }

    pub fn dismiss(&mut self, id: Option<u32>) -> Vec<Did> {
        let mut did = Vec::new();
        for toast in self.toasts.iter_mut().filter(|t| t.leaving.is_none() && id.is_none_or(|id| t.note.id == id)) {
            toast.leaving = Some(Reason::Dismissed);
            did.push(Did::Gone(toast.note.id, Reason::Dismissed));
        }
        did
    }

    /// The left button came up: a click, or the end of a sweep.
    pub fn release(&mut self, look: &Look) -> Vec<Did> {
        let Some((_, id, sweeping)) = self.press.take() else { return Vec::new() };
        let Some(toast) = self.toasts.iter_mut().find(|t| t.note.id == id && t.leaving.is_none()) else { return Vec::new() };
        if sweeping {
            let swept = toast.swept.now;
            if swept.abs() > f64::from(look.width) * 0.35 {
                toast.swept.to = swept.signum() * f64::from(look.width) * 1.1;
                toast.leaving = Some(Reason::Dismissed);
                return vec![Did::Gone(id, Reason::Dismissed)];
            }
            toast.swept.to = 0.0;
            return Vec::new();
        }
        let Some(at) = self.pointer else { return Vec::new() };
        let key = if holds(toast.close, at) {
            None
        } else if let Some((key, _)) = toast.buttons.iter().find(|(_, rect)| holds(*rect, at)) {
            Some(key.clone())
        } else if toast.note.action("default").is_some() {
            Some("default".to_string())
        } else {
            // A click on one with nothing to do does nothing.
            return Vec::new();
        };
        let mut did = Vec::new();
        if let Some(key) = key.clone() {
            did.push(Did::Action(id, key));
        }
        // An action is taken and the notification goes with it, unless it
        // asked to stay; the close button just closes it.
        if key.is_none() || !toast.note.resident {
            toast.leaving = Some(Reason::Dismissed);
            did.push(Did::Gone(id, Reason::Dismissed));
        }
        did
    }
}

/// A card's icon, words, bar and buttons, with its top-left at `(x, y)`.
#[allow(clippy::too_many_arguments)]
fn draw_words(canvas: &mut Canvas, text: &mut Text, icons: &mut Icons, look: &Look, toast: &mut Toast, (x, y): (i32, i32), alpha: f64, hovered: bool, pointer: Option<(i32, i32)>) {
    let note = &toast.note;
    let line = Text::line_height(look.px);
    let mut has_icon = true;
    let icon_top = y + look.pad + (line - look.icon) / 2;
    if let Some((glyph, color)) = kind_icon(toast.treat.kind) {
        let px = (look.icon as f64 / 1.15) as u32;
        text.draw(canvas, x + look.pad, icon_top + (look.icon - Text::line_height(px)) / 2, px, color, alpha, &glyph.to_string());
    } else if let Some(image) = &note.image {
        draw_icon(canvas, image, x + look.pad, icon_top, look.icon, alpha);
    } else if let Some(icon) = icons.get(&note.icon) {
        draw_icon(canvas, icon, x + look.pad, icon_top, look.icon, alpha);
    } else {
        has_icon = false;
    }
    let left = x + look.text_left(has_icon);
    let right = x + look.width - look.pad;
    let mut top = y + look.pad;
    // Who and when, at the right of the first line; the close button in
    // its place under the pointer.
    let said = meta(note);
    let said_width = text.width(&said, look.tiny);
    let meta_top = top + (line - Text::line_height(look.tiny)) / 2;
    if hovered {
        let side = Text::line_height(look.tiny) + look.pad / 4;
        let close = (right - side + look.pad / 4, top + (line - side) / 2, side, side);
        let over = pointer.is_some_and(|p| holds(close, p));
        if over {
            canvas.round_fill(close.0 as f64, close.1 as f64, close.2 as f64, close.3 as f64, side as f64 / 2.0, WELL, alpha);
        }
        let glyph = icons::X.to_string();
        let glyph_width = text.width(&glyph, look.tiny);
        text.draw(canvas, close.0 + (side - glyph_width) / 2, close.1 + (side - Text::line_height(look.tiny)) / 2, look.tiny, if over { TEXT } else { DIM }, alpha, &glyph);
        toast.close = close;
    } else {
        text.draw(canvas, right - said_width, meta_top, look.tiny, DIM, alpha, &said);
        toast.close = (0, 0, 0, 0);
    }
    for title in &toast.laid.title {
        text.draw(canvas, left, top, look.px, TEXT, alpha, title);
        top += line;
    }
    for body in &toast.laid.body {
        text.draw(canvas, left, top, look.small, DIM, alpha, body);
        top += Text::line_height(look.small);
    }
    if let Some(progress) = note.progress {
        top += look.pad * 3 / 4;
        let (bar, across) = (look.pad / 4, right - left);
        canvas.round_fill(left as f64, top as f64, across as f64, bar as f64, bar as f64 / 2.0, RULE, alpha);
        let done = f64::from(across) * f64::from(progress) / 100.0;
        canvas.round_fill(left as f64, top as f64, done, bar as f64, bar as f64 / 2.0, ACCENT, alpha);
        top += bar;
    }
    toast.buttons.clear();
    if !toast.laid.buttons.is_empty() {
        top += look.pad * 3 / 4;
        let mut across = left;
        for (at, (key, label, wide)) in toast.laid.buttons.iter().enumerate() {
            let rect = (across, top, *wide, look.button());
            let over = pointer.is_some_and(|p| holds(rect, p));
            // The first is the one most likely wanted.
            let (ground, ink) = if at == 0 { (ACCENT, ON_ACCENT) } else { (WELL, TEXT) };
            let ground = if over { mix(ground, 0xFFFFFF, 26) } else { ground };
            canvas.round_fill(rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64, look.radius * 0.75, ground, alpha);
            text.draw(canvas, across + look.pad * 3 / 4, top + (rect.3 - Text::line_height(look.small)) / 2, look.small, ink, alpha, label);
            toast.buttons.push((key.clone(), rect));
            across += wide + look.pad / 2;
        }
    }
}

fn draw_icon(canvas: &mut Canvas, icon: &Icon, x: i32, y: i32, size: i32, alpha: f64) {
    canvas.icon(icon, x, y, size, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::NORMAL;

    fn treat(lasts: Option<u64>) -> Treatment {
        Treatment { show: true, keep: true, corner: Corner::BottomRight, lasts, kind: Kind::Plain }
    }

    fn note(id: u32) -> Note {
        Note { id, summary: format!("number {id}"), urgency: NORMAL, timeout: -1, ..Note::default() }
    }

    fn settle(stack: &mut Stack, look: &Look) {
        for _ in 0..100 {
            stack.last = Some(Instant::now() - std::time::Duration::from_millis(16));
            let _ = stack.tick(look, 3);
        }
    }

    #[test]
    fn the_newest_in_front_and_the_rest_peeking_behind() {
        let look = Look::for_monitor(1080, 1.0);
        let (mut text, mut icons) = (Text::load(), Icons::default());
        let mut stack = Stack::new(Corner::BottomRight);
        for id in 1..=4 {
            stack.add(note(id), treat(None), &look, &mut text, &mut icons);
        }
        settle(&mut stack, &look);
        let ids: Vec<u32> = stack.toasts.iter().map(|t| t.note.id).collect();
        assert_eq!(ids, [4, 3, 2, 1]);
        let front = &stack.toasts[0];
        assert_eq!((front.off.now, front.size.now, front.alpha.now), (0.0, 1.0, 1.0));
        // Each further back is smaller, and peeks out further.
        let far = |t: &Toast| t.off.now + t.height.now;
        assert!(far(&stack.toasts[1]) > far(front) && far(&stack.toasts[2]) > far(&stack.toasts[1]));
        assert!(stack.toasts[2].size.now < stack.toasts[1].size.now);
        // The fourth is not shown, and words are only on the front one.
        assert_eq!(stack.toasts[3].alpha.now, 0.0);
        assert_eq!(stack.toasts[1].words.now, 0.0);
    }

    #[test]
    fn laid_out_in_a_column_under_the_pointer_and_held() {
        let look = Look::for_monitor(1080, 1.0);
        let (mut text, mut icons) = (Text::load(), Icons::default());
        let mut stack = Stack::new(Corner::BottomRight);
        stack.add(note(1), treat(Some(5000)), &look, &mut text, &mut icons);
        stack.add(note(2), treat(Some(5000)), &look, &mut text, &mut icons);
        settle(&mut stack, &look);
        let mut pixels = vec![0; 600 * 600];
        stack.draw(&mut Canvas::new(&mut pixels, 600, 600), &mut text, &mut icons, &look, true);
        let front = stack.toasts[0].at;
        stack.point(Some((front.0 + 10, front.1 + 10)), &look);
        assert!(stack.expanded());
        for _ in 0..5 {
            settle(&mut stack, &look);
        }
        // In a column: the second beyond the first, a gap between.
        let (a, b) = (&stack.toasts[0], &stack.toasts[1]);
        assert!((b.off.now - (a.height.now + f64::from(look.gap))).abs() < 1.0);
        // Held: nothing ran out of time while the pointer was there.
        assert!(stack.toasts.iter().all(|t| t.leaving.is_none()));
        stack.point(None, &look);
        assert!(!stack.expanded());
    }

    #[test]
    fn a_click_does_what_the_button_says() {
        let look = Look::for_monitor(1080, 1.0);
        let (mut text, mut icons) = (Text::load(), Icons::default());
        let mut stack = Stack::new(Corner::BottomRight);
        let mut sent = note(7);
        sent.actions = vec![("default".into(), "Open".into()), ("later".into(), "Remind me".into())];
        stack.add(sent, treat(None), &look, &mut text, &mut icons);
        settle(&mut stack, &look);
        let mut pixels = vec![0; 600 * 600];
        stack.draw(&mut Canvas::new(&mut pixels, 600, 600), &mut text, &mut icons, &look, false);
        let front = stack.toasts[0].at;
        stack.point(Some((front.0 + 5, front.1 + 5)), &look);
        stack.draw(&mut Canvas::new(&mut pixels, 600, 600), &mut text, &mut icons, &look, false);
        let button = stack.toasts[0].buttons[0].1;
        stack.point(Some((button.0 + 2, button.1 + 2)), &look);
        stack.press(1);
        assert_eq!(stack.release(&look), [Did::Action(7, "later".into()), Did::Gone(7, Reason::Dismissed)]);
    }

    #[test]
    fn swept_away() {
        let look = Look::for_monitor(1080, 1.0);
        let (mut text, mut icons) = (Text::load(), Icons::default());
        let mut stack = Stack::new(Corner::BottomRight);
        stack.add(note(3), treat(None), &look, &mut text, &mut icons);
        settle(&mut stack, &look);
        let mut pixels = vec![0; 600 * 600];
        stack.draw(&mut Canvas::new(&mut pixels, 600, 600), &mut text, &mut icons, &look, false);
        let front = stack.toasts[0].at;
        stack.point(Some((front.0 + 20, front.1 + 20)), &look);
        stack.press(1);
        stack.point(Some((front.0 + 20 + look.width / 2, front.1 + 20)), &look);
        assert_eq!(stack.release(&look), [Did::Gone(3, Reason::Dismissed)]);
        settle(&mut stack, &look);
        assert!(stack.is_empty());
    }

    #[test]
    fn how_long_ago() {
        let now = crate::note::now();
        assert_eq!(ago(now), "now");
        assert_eq!(ago(now - 125), "2m");
        assert_eq!(ago(now - 7300), "2h");
    }
}
