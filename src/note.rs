//! A notification: what a program sent, read out of the call it came in,
//! and what toast keeps of it afterwards.

use serde::{Deserialize, Serialize};

use crate::canvas::Icon;
use crate::dbus::Value;

/// How much a notification matters, as the programs say it.
pub const LOW: u8 = 0;
pub const NORMAL: u8 = 1;
pub const CRITICAL: u8 = 2;

/// What kind of news it is, which says the icon drawn where the program
/// gave none, and its color: as shadcn's toasts have them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Plain,
    Success,
    Info,
    Warning,
    Error,
}

impl Kind {
    pub fn named(name: &str) -> Option<Kind> {
        Some(match name.to_ascii_lowercase().as_str() {
            "plain" | "" => Kind::Plain,
            "success" | "ok" | "done" => Kind::Success,
            "info" => Kind::Info,
            "warning" | "warn" => Kind::Warning,
            "error" | "fail" | "failed" => Kind::Error,
            _ => return None,
        })
    }
}

/// One corner or edge of the screen, where a stack of them shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    Top,
    TopRight,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 6] = [Corner::TopLeft, Corner::Top, Corner::TopRight, Corner::BottomLeft, Corner::Bottom, Corner::BottomRight];

    pub fn named(name: &str) -> Option<Corner> {
        Corner::ALL.into_iter().find(|corner| corner.name() == name.to_ascii_lowercase().replace(['_', ' '], "-").replace("center", "").trim_matches('-'))
    }

    pub fn name(self) -> &'static str {
        match self {
            Corner::TopLeft => "top-left",
            Corner::Top => "top",
            Corner::TopRight => "top-right",
            Corner::BottomLeft => "bottom-left",
            Corner::Bottom => "bottom",
            Corner::BottomRight => "bottom-right",
        }
    }

    pub fn top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::Top | Corner::TopRight)
    }

    /// -1 for the left, 0 for the middle, 1 for the right.
    pub fn side(self) -> i32 {
        match self {
            Corner::TopLeft | Corner::BottomLeft => -1,
            Corner::Top | Corner::Bottom => 0,
            Corner::TopRight | Corner::BottomRight => 1,
        }
    }
}

/// A notification as it came, with what toast makes of its hints.
#[derive(Clone, Debug, Default)]
pub struct Note {
    pub id: u32,
    /// The one it takes the place of, which is `id` when there was one.
    pub replaces: u32,
    pub app: String,
    /// An icon's name or a file, as sent.
    pub icon: String,
    /// A picture sent in the call itself.
    pub image: Option<Icon>,
    pub summary: String,
    pub body: String,
    /// What can be done about it: what the program is told, and the
    /// button's words. "default" is a click on the notification itself.
    pub actions: Vec<(String, String)>,
    pub urgency: u8,
    /// In milliseconds: -1 for as long as toast thinks right, 0 for
    /// until it is dismissed.
    pub timeout: i32,
    pub category: String,
    /// How far along something is, out of 100.
    pub progress: Option<i32>,
    /// Kept on screen after one of its actions is chosen.
    pub resident: bool,
    /// Not to be kept in the history.
    pub transient: bool,
    pub kind: Kind,
    /// Where it is to be shown, if it says.
    pub corner: Option<Corner>,
    pub sticky: bool,
    /// Kept in the history only, not shown.
    pub quiet: bool,
    /// Commands toast itself runs when an action is chosen, by its key:
    /// the program that sent it need not be there to hear of it.
    pub run: Vec<(String, String)>,
    /// When it came, in seconds since 1970.
    pub time: u64,
}

impl Note {
    /// The values of a call to Notify: the program's name, the one it
    /// replaces, its icon, the summary and body, the actions, the hints
    /// and how long it lasts.
    pub fn from_call(body: &[Value]) -> Option<Note> {
        let [Value::Str(app), Value::U32(replaces), Value::Str(icon), Value::Str(summary), Value::Str(text), Value::Array(_, actions), Value::Array(_, hints), Value::I32(timeout)] = body else {
            return None;
        };
        let mut note = Note {
            replaces: *replaces,
            app: app.clone(),
            icon: icon.clone(),
            summary: plain(summary),
            body: plain(text),
            urgency: NORMAL,
            timeout: *timeout,
            time: now(),
            ..Note::default()
        };
        let words: Vec<&str> = actions.iter().filter_map(Value::as_str).collect();
        note.actions = words.chunks_exact(2).map(|pair| (pair[0].to_string(), plain(pair[1]))).collect();
        for hint in hints {
            let Value::Entry(name, value) = hint else { continue };
            let (Some(name), Value::Variant(value)) = (name.as_str(), value.as_ref()) else { continue };
            note.hint(name, value);
        }
        Some(note)
    }

    fn hint(&mut self, name: &str, value: &Value) {
        let text = || value.as_str().unwrap_or("").to_string();
        let yes = || matches!(value, Value::Bool(true)) || value.as_i32().is_some_and(|n| n != 0);
        match name {
            "urgency" => {
                self.urgency = match value {
                    Value::Byte(level) => (*level).min(CRITICAL),
                    other => other.as_i32().unwrap_or(1).clamp(0, 2) as u8,
                }
            }
            "category" => self.category = text(),
            "value" => self.progress = value.as_i32().map(|n| n.clamp(0, 100)),
            "resident" => self.resident = yes(),
            "transient" => self.transient = yes(),
            "image-data" | "image_data" | "icon_data" => self.image = picture(value).or(self.image.take()),
            "image-path" | "image_path" if !text().is_empty() => self.icon = text(),
            "x-toast-kind" => self.kind = Kind::named(&text()).unwrap_or_default(),
            "x-toast-where" => self.corner = Corner::named(&text()),
            "x-toast-sticky" => self.sticky = yes(),
            "x-toast-quiet" => self.quiet = yes(),
            // A line for each: the action's key, a tab, and the command.
            "x-toast-run" => {
                let given = text();
                self.run = given.lines().filter_map(|line| line.split_once('\t')).map(|(key, command)| (key.to_string(), command.to_string())).collect();
            }
            _ => {}
        }
    }

    /// The label of the action under `key`.
    pub fn action(&self, key: &str) -> Option<&str> {
        self.actions.iter().find(|(k, _)| k == key).map(|(_, label)| label.as_str())
    }

    /// The buttons drawn for it: every action but the click on the
    /// notification itself.
    pub fn buttons(&self) -> impl Iterator<Item = &(String, String)> {
        self.actions.iter().filter(|(key, _)| key != "default")
    }

    pub fn record(&self) -> Record {
        Record {
            id: self.id,
            time: self.time,
            app: self.app.clone(),
            icon: if self.icon.starts_with("data:") { String::new() } else { self.icon.clone() },
            summary: self.summary.clone(),
            body: self.body.clone(),
            urgency: self.urgency,
            kind: self.kind,
            actions: self.buttons().map(|(_, label)| label.clone()).collect(),
        }
    }
}

/// What the history keeps of a notification.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: u32,
    pub time: u64,
    #[serde(default)]
    pub app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default)]
    pub urgency: u8,
    #[serde(default, skip_serializing_if = "is_plain")]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

fn is_plain(kind: &Kind) -> bool {
    *kind == Kind::Plain
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// A picture sent as its pixels: width, height, the bytes to a row,
/// whether it has opacity, the bits to a sample, how many samples to a
/// pixel, and the bytes.
fn picture(value: &Value) -> Option<Icon> {
    let Value::Struct(parts) = value else { return None };
    let [width, height, row, _, bits, samples, Value::Bytes(bytes)] = &parts[..] else { return None };
    let (width, height, row, samples) = (width.as_i32()? as usize, height.as_i32()? as usize, row.as_i32()? as usize, samples.as_i32()? as usize);
    if bits.as_i32()? != 8 || !(3..=4).contains(&samples) || width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let at = y * row + x * samples;
            let p = bytes.get(at..at + samples)?;
            let alpha = if samples == 4 { p[3] } else { 255 };
            pixels.push(u32::from(alpha) << 24 | u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]));
        }
    }
    Some(Icon { width, height, pixels })
}

/// Text with what markup it had taken out: the tags, and the names for
/// letters that are written as names.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['<', '&']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if rest.starts_with('<') {
            // A tag ends at its '>'; one that never does was not a tag.
            match rest.find('>') {
                Some(end) if !rest[1..end].contains('<') => {
                    let tag = rest[1..end].trim_start_matches('/').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                    if tag == "br" || tag == "br/" {
                        out.push('\n');
                    }
                    rest = &rest[end + 1..];
                }
                _ => {
                    out.push('<');
                    rest = &rest[1..];
                }
            }
        } else {
            let end = rest.find(';').filter(|end| *end <= 10);
            let named = end.and_then(|end| {
                let name = &rest[1..end];
                let letter = match name {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    "nbsp" => ' ',
                    _ => match name.strip_prefix("#x").or(name.strip_prefix("#X")) {
                        Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?)?,
                        None => char::from_u32(name.strip_prefix('#')?.parse().ok()?)?,
                    },
                };
                Some((letter, end))
            });
            match named {
                Some((letter, end)) => {
                    out.push(letter);
                    rest = &rest[end + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_taken_out() {
        assert_eq!(plain("<b>Build</b> done &amp; <i>passed</i>"), "Build done & passed");
        assert_eq!(plain("a < b &lt; c"), "a < b < c");
        assert_eq!(plain("line<br/>next &#8212; &#x41;"), "line\nnext \u{2014} A");
        assert_eq!(plain("<a href=\"https://x\">link</a> & more"), "link & more");
    }

    #[test]
    fn a_call_to_notify() {
        let hints = Value::Array(
            "{sv}".into(),
            vec![
                Value::Entry(Box::new(Value::text("urgency")), Box::new(Value::any(Value::Byte(2)))),
                Value::Entry(Box::new(Value::text("value")), Box::new(Value::any(Value::I32(140)))),
                Value::Entry(Box::new(Value::text("x-toast-where")), Box::new(Value::any(Value::text("top")))),
                Value::Entry(Box::new(Value::text("x-toast-run")), Box::new(Value::any(Value::text("open\txdg-open .\n")))),
            ],
        );
        let actions = Value::Array("s".into(), ["default", "Open", "open", "Open folder"].map(Value::text).to_vec());
        let body = [Value::text("cargo"), Value::U32(0), Value::text(""), Value::text("Built"), Value::text("<b>toast</b>"), actions, hints, Value::I32(-1)];
        let note = Note::from_call(&body).unwrap();
        assert_eq!((note.app.as_str(), note.summary.as_str(), note.body.as_str()), ("cargo", "Built", "toast"));
        assert_eq!((note.urgency, note.progress, note.corner), (CRITICAL, Some(100), Some(Corner::Top)));
        assert_eq!(note.buttons().count(), 1);
        assert_eq!(note.run, vec![("open".to_string(), "xdg-open .".to_string())]);
    }

    #[test]
    fn corners_by_name() {
        assert_eq!(Corner::named("bottom-right"), Some(Corner::BottomRight));
        assert_eq!(Corner::named("top-center"), Some(Corner::Top));
        assert_eq!(Corner::named("Top Left"), Some(Corner::TopLeft));
        assert_eq!(Corner::named("middle"), None);
    }
}
