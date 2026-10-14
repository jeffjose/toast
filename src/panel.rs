//! The history, as a box in the middle of the screen as clutch's is:
//! everything kept, the newest first, under the day it came on. Typing
//! narrows it; Enter copies one; Delete forgets it.

use crate::canvas::{icons, Canvas, Text};
use crate::icons::Icons;
use crate::note::{Kind, Record, CRITICAL};
use crate::stack::{Look, Rect, ACCENT, AMBER, BLUE, DIM, GREEN, ON_ACCENT, PANEL, RED, RULE, TEXT};

/// The lesser words of the row picked out: gray on the accent is mud.
const DIM_ON_ACCENT: u32 = 0xC9D3F6;
const EDGE: u32 = 0x555555;
/// How many notifications are in view at once.
const ROWS: i32 = 8;

/// A line of the list: the day those under it came on, or one of them.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Day(String),
    Item(usize),
}

/// What the box asks to be done.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    Close,
    /// Copy these words, and close.
    Copy(String),
    /// Forget one, or all of them.
    Forget(Option<u32>),
}

pub struct Panel {
    look: Look,
    /// Everything kept, the newest first.
    all: Vec<Record>,
    pub typed: String,
    rows: Vec<Row>,
    /// Which row is picked out, and which is the first in view.
    selected: usize,
    top: usize,
    pointer: Option<(i32, i32)>,
    /// Whether notifications are being kept from the screen.
    pub quiet: bool,
    /// Whether Ctrl+Delete has been pressed once, and is waiting to be
    /// pressed again before everything is forgotten.
    armed: bool,
}

/// Sizes of the box, from the stack's.
struct Sizes {
    px: u32,
    small: u32,
    tiny: u32,
    pad: i32,
    width: i32,
    head: i32,
    row: i32,
    day: i32,
    foot: i32,
}

impl Panel {
    pub fn new(look: Look, mut records: Vec<Record>) -> Panel {
        records.reverse();
        let mut panel = Panel { look, all: records, typed: String::new(), rows: Vec::new(), selected: 0, top: 0, pointer: None, quiet: false, armed: false };
        panel.find();
        panel
    }

    fn sizes(&self) -> Sizes {
        let pad = self.look.pad;
        let (px, small, tiny) = (pad as u32, self.look.px, self.look.tiny);
        let line = Text::line_height(px);
        Sizes {
            px,
            small,
            tiny,
            pad,
            width: pad * 38,
            head: line + 2 * pad,
            row: line + Text::line_height(small) + pad * 3 / 4,
            day: Text::line_height(tiny) + pad * 3 / 4,
            foot: Text::line_height(px * 5 / 6) + pad,
        }
    }

    /// How large the box is.
    pub fn size(&self) -> (i32, i32) {
        let s = self.sizes();
        (s.width, s.head + ROWS * s.row + s.pad / 2 + s.foot)
    }

    pub fn set_records(&mut self, mut records: Vec<Record>) {
        records.reverse();
        self.all = records;
        self.find();
    }

    /// The rows for what is typed: every word of it is to be somewhere in
    /// the notification, its program, summary or body.
    fn find(&mut self) {
        let words: Vec<String> = self.typed.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut rows = Vec::new();
        let mut last_day = String::new();
        for (at, record) in self.all.iter().enumerate() {
            let said = format!("{} {} {}", record.app, record.summary, record.body).to_lowercase();
            if !words.iter().all(|word| said.contains(word.as_str())) {
                continue;
            }
            let day = day_of(record.time);
            if day != last_day {
                rows.push(Row::Day(day.clone()));
                last_day = day;
            }
            rows.push(Row::Item(at));
        }
        self.rows = rows;
        self.selected = self.rows.iter().position(|row| matches!(row, Row::Item(_))).unwrap_or(0);
        self.top = 0;
        self.armed = false;
    }

    fn items(&self) -> usize {
        self.rows.iter().filter(|row| matches!(row, Row::Item(_))).count()
    }

    fn selected_record(&self) -> Option<&Record> {
        match self.rows.get(self.selected)? {
            Row::Item(at) => self.all.get(*at),
            Row::Day(_) => None,
        }
    }

    /// Picks out the next notification `by` away, past the days.
    fn step(&mut self, by: isize) {
        let items: Vec<usize> = self.rows.iter().enumerate().filter(|(_, row)| matches!(row, Row::Item(_))).map(|(at, _)| at).collect();
        let Some(now) = items.iter().position(|&at| at == self.selected) else { return };
        let to = (now as isize + by).clamp(0, items.len() as isize - 1) as usize;
        self.selected = items[to];
    }

    /// The heights of the rows from the first in view, and which fit.
    fn in_view(&mut self) -> std::ops::Range<usize> {
        let s = self.sizes();
        let room = ROWS * s.row;
        let height = |row: &Row| if matches!(row, Row::Day(_)) { s.day } else { s.row };
        // Scrolled so the one picked out is in view, with its day if it can be.
        if self.selected < self.top {
            self.top = self.selected.saturating_sub(usize::from(self.selected > 0 && matches!(self.rows[self.selected - 1], Row::Day(_))));
        }
        loop {
            let used: i32 = self.rows[self.top..=self.selected.min(self.rows.len().saturating_sub(1)).max(self.top)].iter().map(height).sum();
            if used <= room || self.top >= self.selected {
                break;
            }
            self.top += 1;
        }
        let mut used = 0;
        let mut end = self.top;
        while end < self.rows.len() && used + height(&self.rows[end]) <= room {
            used += height(&self.rows[end]);
            end += 1;
        }
        self.top..end
    }

    /// Which row is where, as last drawn.
    fn row_at(&mut self, (x, y): (i32, i32)) -> Option<usize> {
        let s = self.sizes();
        if x < 0 || x >= s.width {
            return None;
        }
        let mut top = s.head + s.pad / 4;
        for at in self.in_view() {
            let height = if matches!(self.rows[at], Row::Day(_)) { s.day } else { s.row };
            if y >= top && y < top + height {
                return matches!(self.rows[at], Row::Item(_)).then_some(at);
            }
            top += height;
        }
        None
    }

    pub fn point(&mut self, at: Option<(i32, i32)>) {
        self.pointer = at;
    }

    pub fn click(&mut self) -> Option<Asked> {
        let at = self.row_at(self.pointer?)?;
        self.selected = at;
        self.enter()
    }

    pub fn wheel(&mut self, down: bool) {
        self.step(if down { 1 } else { -1 });
    }

    fn enter(&self) -> Option<Asked> {
        let record = self.selected_record()?;
        let words = if record.body.is_empty() { record.summary.clone() } else { format!("{}\n{}", record.summary, record.body) };
        Some(Asked::Copy(words))
    }

    /// A key: its name as winit has it, or the letter typed; and whether
    /// Ctrl is held.
    pub fn key(&mut self, name: &str, ctrl: bool) -> Option<Asked> {
        let armed = std::mem::take(&mut self.armed);
        match name {
            "Escape" if !self.typed.is_empty() => {
                self.typed.clear();
                self.find();
            }
            "Escape" => return Some(Asked::Close),
            "Enter" => return self.enter(),
            "ArrowDown" => self.step(1),
            "ArrowUp" => self.step(-1),
            "PageDown" => self.step(ROWS as isize - 1),
            "PageUp" => self.step(1 - ROWS as isize),
            "Home" => self.step(-(self.rows.len() as isize)),
            "End" => self.step(self.rows.len() as isize),
            "Delete" if ctrl => {
                if armed {
                    self.all.clear();
                    self.find();
                    return Some(Asked::Forget(None));
                }
                self.armed = true;
            }
            "Delete" => {
                let id = self.selected_record()?.id;
                let at = self.selected;
                self.all.retain(|r| r.id != id);
                self.find();
                // The one after it is picked out in its place.
                self.selected = at.min(self.rows.len().saturating_sub(1));
                if matches!(self.rows.get(self.selected), Some(Row::Day(_))) {
                    self.step(1);
                }
                return Some(Asked::Forget(Some(id)));
            }
            "Backspace" if ctrl => {
                self.typed.clear();
                self.find();
            }
            "Backspace" => {
                self.typed.pop();
                self.find();
            }
            letter if letter.chars().count() == 1 && !ctrl => {
                self.typed.push_str(letter);
                self.find();
            }
            _ => {}
        }
        None
    }

    pub fn draw(&mut self, canvas: &mut Canvas, text: &mut Text, icons: &mut Icons) {
        let s = self.sizes();
        let (width, height) = self.size();
        canvas.clear();
        canvas.card(&crate::canvas::Card { x: 0.0, y: 0.0, w: f64::from(width), h: f64::from(height), radius: f64::from(s.pad * 2 / 3), fill: PANEL, edge: EDGE, alpha: 1.0 });
        let line = Text::line_height(s.px);

        // What is typed, after the icon for looking; how many there are.
        let search = icons::SEARCH.to_string();
        text.draw(canvas, s.pad + s.pad / 4, s.pad, s.px, DIM, 1.0, &search);
        let typed_left = s.pad * 3;
        if self.typed.is_empty() {
            text.draw(canvas, typed_left + s.pad / 3, s.pad, s.px, DIM, 1.0, "Search notifications");
        } else {
            text.draw(canvas, typed_left, s.pad, s.px, TEXT, 1.0, &self.typed);
        }
        let caret = typed_left + if self.typed.is_empty() { 0 } else { text.width(&self.typed, s.px) + 1 };
        canvas.fill(caret, s.pad + line / 6, 1, line * 2 / 3, TEXT, 1.0);
        let count = match (self.items(), self.typed.is_empty()) {
            (0, _) => String::new(),
            (n, true) => format!("{n}"),
            (n, false) => format!("{n} of {}", self.all.len()),
        };
        let mut right = s.width - s.pad;
        if self.quiet {
            let quiet = format!("{} quiet", icons::BELL_OFF);
            let wide = text.width(&quiet, s.small);
            text.draw(canvas, right - wide, s.pad + (line - Text::line_height(s.small)) / 2, s.small, AMBER, 1.0, &quiet);
            right -= wide + s.pad;
        }
        let wide = text.width(&count, s.px);
        text.draw(canvas, right - wide, s.pad, s.px, DIM, 1.0, &count);
        canvas.fill(0, s.head, s.width, 1, RULE, 1.0);

        let hovered = self.pointer.and_then(|p| self.row_at(p));
        let mut top = s.head + s.pad / 4;
        if self.rows.is_empty() {
            let said = if self.all.is_empty() { "Nothing yet. What comes is kept here." } else { "Nothing like that." };
            let glyph = icons::BELL.to_string();
            let middle = s.head + ROWS * s.row / 2;
            let (glyph_px, wide) = (s.px * 2, text.width(said, s.px));
            let glyph_width = text.width(&glyph, glyph_px);
            text.draw(canvas, (s.width - glyph_width) / 2, middle - Text::line_height(glyph_px) - s.pad / 2, glyph_px, RULE, 1.0, &glyph);
            text.draw(canvas, (s.width - wide) / 2, middle, s.px, DIM, 1.0, said);
        }
        for at in self.in_view() {
            match self.rows[at].clone() {
                Row::Day(day) => {
                    text.draw(canvas, s.pad + s.pad / 4, top + s.pad / 2, s.tiny, DIM, 1.0, &day);
                    top += s.day;
                }
                Row::Item(index) => {
                    let rect: Rect = (s.pad / 2, top, s.width - s.pad, s.row);
                    let picked = at == self.selected;
                    if picked {
                        canvas.round_fill(rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64, f64::from(s.pad / 2), ACCENT, 1.0);
                    } else if hovered == Some(at) {
                        canvas.round_fill(rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64, f64::from(s.pad / 2), 0x2A2A2A, 1.0);
                    }
                    let record = self.all[index].clone();
                    self.draw_row(canvas, text, icons, &s, &record, top, picked);
                    top += s.row;
                }
            }
        }

        let foot = height - s.foot;
        canvas.fill(0, foot, s.width, 1, RULE, 1.0);
        let keys: &[(&str, &str)] = if self.armed {
            &[("Ctrl+Del", "again to forget everything"), ("Esc", "keep it all")]
        } else {
            &[("Enter", "copy"), ("Del", "forget"), ("Ctrl+Del", "forget all"), ("Esc", "close")]
        };
        let small = s.px * 5 / 6;
        let mut left = s.pad + s.pad / 4;
        for (key, said) in keys {
            text.draw(canvas, left, foot + s.pad / 2, small, TEXT, 1.0, key);
            left += text.width(key, small) + s.pad / 3;
            let color = if self.armed { AMBER } else { DIM };
            text.draw(canvas, left, foot + s.pad / 2, small, color, 1.0, said);
            left += text.width(said, small) + s.pad * 3 / 2;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(&mut self, canvas: &mut Canvas, text: &mut Text, icons: &mut Icons, s: &Sizes, record: &Record, top: i32, picked: bool) {
        let (ink, dim) = if picked { (ON_ACCENT, DIM_ON_ACCENT) } else { (TEXT, DIM) };
        let icon = self.look.icon;
        let icon_left = s.pad + s.pad / 4;
        let icon_top = top + (s.row - icon) / 2;
        let kind = match record.kind {
            Kind::Success => Some((icons::CHECK_FILLED, GREEN)),
            Kind::Info => Some((icons::INFO_FILLED, BLUE)),
            Kind::Warning => Some((icons::ALERT_FILLED, AMBER)),
            Kind::Error => Some((icons::DANGER_FILLED, RED)),
            Kind::Plain if record.urgency == CRITICAL => Some((icons::DANGER_FILLED, RED)),
            Kind::Plain => None,
        };
        let glyph_px = (icon as f64 / 1.15) as u32;
        let glyph_top = icon_top + (icon - Text::line_height(glyph_px)) / 2;
        match (kind, icons.get(&record.icon)) {
            (Some((glyph, color)), _) => text.draw(canvas, icon_left, glyph_top, glyph_px, if picked { ON_ACCENT } else { color }, 1.0, &glyph.to_string()),
            (None, Some(picture)) => canvas.icon(picture, icon_left, icon_top, icon, 1.0),
            (None, None) => text.draw(canvas, icon_left, glyph_top, glyph_px, dim, 1.0, &icons::BELL.to_string()),
        }
        let left = icon_left + icon + s.pad * 3 / 4;
        let right = s.width - s.pad - s.pad / 4;
        // When, and from whom, at the right.
        let when = time_of(record.time);
        let app = text.clipped(&record.app, s.pad * 8, s.tiny);
        let line = Text::line_height(s.px);
        let first = top + s.pad * 3 / 8;
        let when_width = text.width(&when, s.tiny);
        text.draw(canvas, right - when_width, first + (line - Text::line_height(s.tiny)) / 2, s.tiny, dim, 1.0, &when);
        let app_width = text.width(&app, s.tiny);
        text.draw(canvas, right - app_width, first + line, s.tiny, dim, 1.0, &app);
        let room = right - left - s.pad - when_width.max(app_width);
        let summary = text.clipped(&record.summary, room, s.px);
        text.draw(canvas, left, first, s.px, ink, 1.0, &summary);
        let body = record.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let body = if body.is_empty() && !record.actions.is_empty() { record.actions.join(" \u{b7} ") } else { body.to_string() };
        let body = text.clipped(&body, room, s.small);
        text.draw(canvas, left, first + line, s.small, dim, 1.0, &body);
    }
}

/// The local time, as the system has it: the day of the year it is, the
/// day of the week, and the hour and minute.
fn local(time: u64) -> (i32, i32, i32, i32, i32, i32) {
    let seconds = time as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes only to the struct it is given.
    unsafe { libc::localtime_r(&seconds, &mut parts) };
    (parts.tm_year, parts.tm_yday, parts.tm_wday, parts.tm_mon, parts.tm_mday, parts.tm_hour * 60 + parts.tm_min)
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// "Today", "Yesterday", "Monday 12 October".
pub fn day_of(time: u64) -> String {
    let (year, day, weekday, month, date, _) = local(time);
    let (this_year, today, ..) = local(crate::note::now());
    match (year == this_year, today - day) {
        (true, 0) => "Today".into(),
        (true, 1) => "Yesterday".into(),
        _ => format!("{} {date} {}", DAYS[weekday as usize % 7], MONTHS[month as usize % 12]),
    }
}

/// "14:05".
pub fn time_of(time: u64) -> String {
    let minutes = local(time).5;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u32, summary: &str, time: u64) -> Record {
        Record { id, summary: summary.into(), app: "test".into(), time, ..Record::default() }
    }

    #[test]
    fn newest_first_under_their_days_and_narrowed() {
        let now = crate::note::now();
        let records = vec![record(1, "old one", now - 3 * 86400), record(2, "build done", now - 60), record(3, "build failed", now)];
        let mut panel = Panel::new(Look::for_monitor(1080, 1.0), records);
        assert_eq!(panel.rows, [Row::Day("Today".into()), Row::Item(0), Row::Item(1), Row::Day(day_of(now - 3 * 86400)), Row::Item(2)]);
        assert_eq!(panel.selected_record().unwrap().id, 3);
        panel.step(1);
        assert_eq!(panel.selected_record().unwrap().id, 2);
        // Past the day, to the next.
        panel.step(1);
        assert_eq!(panel.selected_record().unwrap().id, 1);
        for letter in ["f", "a", "i", "l"] {
            panel.key(letter, false);
        }
        assert_eq!(panel.items(), 1);
        assert_eq!(panel.key("Enter", false), Some(Asked::Copy("build failed".into())));
    }

    #[test]
    fn forgotten_one_at_a_time_or_all_on_asking_twice() {
        let now = crate::note::now();
        let mut panel = Panel::new(Look::for_monitor(1080, 1.0), vec![record(1, "a", now), record(2, "b", now)]);
        assert_eq!(panel.key("Delete", false), Some(Asked::Forget(Some(2))));
        assert_eq!(panel.selected_record().unwrap().id, 1);
        assert_eq!(panel.key("Delete", true), None);
        assert_eq!(panel.key("Delete", true), Some(Asked::Forget(None)));
        assert_eq!(panel.items(), 0);
    }
}
