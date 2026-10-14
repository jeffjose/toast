//! Notifications to show what toast looks like: sent with `toast demo`,
//! or drawn to a file with `toast picture`, for the README.

use std::fs;

use crate::canvas::{Canvas, Text};
use crate::config::Config;
use crate::icons::Icons;
use crate::note::{now, Corner, Kind, Note, CRITICAL, NORMAL};
use crate::panel::Panel;
use crate::stack::{Look, Stack};

/// A few of the kinds of thing a desktop is told, oldest first.
pub fn notes() -> Vec<Note> {
    let note = |app: &str, summary: &str, body: &str| Note { app: app.into(), summary: summary.into(), body: body.into(), urgency: NORMAL, timeout: -1, time: now(), ..Note::default() };
    let pair = |key: &str, label: &str| (key.to_string(), label.to_string());
    vec![
        Note { kind: Kind::Warning, sticky: true, ..note("backup", "Backup is 3 days old", "The last run on the NAS stopped: its disk is full.") },
        Note { progress: Some(62), ..note("Firefox", "Downloading ubuntu-24.04.iso", "3.1 GB of 5.0 GB, 2 minutes left") },
        Note { kind: Kind::Info, actions: vec![pair("again", "5 more minutes"), pair("stop", "Stop")], ..note("clutch", "Tea is ready", "The 4 minute timer is done.") },
        Note { kind: Kind::Error, urgency: CRITICAL, ..note("deploy", "Deploy failed", "api: the health check timed out after 30s.") },
        Note { actions: vec![pair("default", "Open"), pair("reply", "Reply"), pair("read", "Mark as read")], ..note("Messages", "Sam", "Are we still on for 6? I can bring the projector if you want to watch it on the wall.") },
        Note { kind: Kind::Success, actions: vec![pair("open", "Open folder")], ..note("cargo", "Build finished", "toast compiled in 41.2s, with no warnings.") },
    ]
}

/// The window the stack would be drawn in, as a picture: over a ground
/// like a desktop's, to see the shadows by.
pub fn picture(path: &str, scene: &str) -> Result<(), String> {
    let look = Look::for_monitor(1080, 1.0);
    let (mut text, mut icons) = (Text::load(), Icons::default());
    let config = Config::default();
    let (width, height, pixels) = match scene {
        "panel" => {
            // The demo ones, as if they had come over the last day and a half.
            let records = notes().iter().enumerate().map(|(at, note)| Note { id: at as u32 + 1, time: now() - (5 - at as u64) * 9000, ..note.clone() }.record()).collect();
            let mut panel = Panel::new(look, records);
            let (width, height) = panel.size();
            let mut pixels = vec![0u32; (width * height) as usize];
            panel.draw(&mut Canvas::new(&mut pixels, width as u32, height as u32), &mut text, &mut icons);
            (width, height, pixels)
        }
        _ => {
            let mut stack = Stack::new(Corner::BottomRight);
            let notes = notes();
            let count = match scene {
                "one" => 1,
                _ => notes.len(),
            };
            for (at, mut note) in notes.into_iter().rev().take(count).rev().enumerate() {
                note.id = at as u32 + 1;
                let treat = config.treat(&note);
                note.kind = treat.kind;
                stack.add(note, treat, &look, &mut text, &mut icons);
                stack.settle(&look, config.stack);
            }
            let (width, height) = (look.width + 2 * look.margin, look.margin * 2 + 3 * (look.tallest() * 2 / 3 + look.gap));
            let mut pixels = vec![0u32; (width * height) as usize];
            stack.draw(&mut Canvas::new(&mut pixels, width as u32, height as u32), &mut text, &mut icons, &look, true);
            if scene == "expanded" {
                let at = stack.bounds().ok_or("nothing drawn")?;
                stack.point(Some((at.0 + at.2 / 2, at.1 + at.3 - look.pad)), &look);
                stack.settle(&look, config.stack);
                stack.draw(&mut Canvas::new(&mut pixels, width as u32, height as u32), &mut text, &mut icons, &look, true);
            }
            while text.waiting() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                stack.draw(&mut Canvas::new(&mut pixels, width as u32, height as u32), &mut text, &mut icons, &look, true);
            }
            (width, height, pixels)
        }
    };
    write_png(path, width as u32, height as u32, &pixels)
}

/// Writes weighed pixels to a PNG, over a ground of blue going to purple,
/// as a desktop's might be.
fn write_png(path: &str, width: u32, height: u32, pixels: &[u32]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(pixels.len() * 3);
    for (at, pixel) in pixels.iter().enumerate() {
        let (x, y) = (at as u32 % width, at as u32 / width);
        let t = (x + y) as f64 / f64::from(width + height);
        let ground = [(0x2B as f64 * (1.0 - t) + 0x4A as f64 * t), (0x35 as f64 * (1.0 - t) + 0x2E as f64 * t), (0x55 as f64 * (1.0 - t) + 0x6B as f64 * t)];
        let left = 1.0 - f64::from(pixel >> 24) / 255.0;
        for (channel, shift) in [16, 8, 0].into_iter().enumerate() {
            let mine = f64::from(pixel >> shift & 0xFF);
            bytes.push((mine + ground[channel] * left).round().min(255.0) as u8);
        }
    }
    let file = fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    let written = encoder.write_header().and_then(|mut writer| writer.write_image_data(&bytes));
    written.map_err(|e| format!("{path}: {e}"))
}
