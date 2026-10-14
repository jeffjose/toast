//! The pictures notifications are drawn with: a program's icon, found by
//! its name as clutch finds them, or a file it names.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::canvas::Icon;

/// Where icons are kept: your own first, then the system's.
fn data_dirs() -> Vec<PathBuf> {
    let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    let own = env::var_os("XDG_DATA_HOME").map_or(home.join(".local/share"), PathBuf::from);
    let shared = env::var("XDG_DATA_DIRS").unwrap_or_default();
    let shared = if shared.is_empty() { "/usr/local/share:/usr/share".to_string() } else { shared };
    std::iter::once(own).chain(shared.split(':').map(PathBuf::from)).collect()
}

/// The sizes an icon is looked for in, the nearest above what a card
/// needs first.
const SIZES: &[&str] = &["48x48", "64x64", "96x96", "128x128", "256x256", "32x32", "24x24", "512x512"];
const THEMES: &[&str] = &["hicolor", "Papirus", "elementary-xfce", "Adwaita", "gnome", "Humanity"];
/// What a notification's icon is, by the folders themes keep them in.
const KINDS: &[&str] = &["apps", "status", "devices", "actions", "categories", "mimetypes", "places", "emblems"];

/// Icons once looked for, found or not.
#[derive(Default)]
pub struct Icons {
    found: HashMap<String, Option<Icon>>,
}

impl Icons {
    pub fn get(&mut self, name: &str) -> Option<&Icon> {
        if !self.found.contains_key(name) {
            let icon = find(name);
            self.found.insert(name.to_string(), icon);
        }
        self.found.get(name)?.as_ref()
    }
}

/// The picture for an icon's name, where it is a PNG that can be found:
/// the name is a file itself, or one in the usual folders.
pub fn find(name: &str) -> Option<Icon> {
    let name = name.strip_prefix("file://").unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    if name.starts_with('/') {
        return read(Path::new(name));
    }
    let dirs = data_dirs();
    for dir in &dirs {
        for theme in THEMES {
            for size in SIZES {
                for kind in KINDS {
                    if let Some(found) = read(&dir.join(format!("icons/{theme}/{size}/{kind}/{name}.png"))) {
                        return Some(found);
                    }
                }
            }
        }
    }
    dirs.iter().find_map(|dir| read(&dir.join(format!("pixmaps/{name}.png"))))
}

pub fn read(path: &Path) -> Option<Icon> {
    decode(std::io::BufReader::new(fs::File::open(path).ok()?))
}

/// toast's own icon, which it carries.
pub fn own() -> Option<Icon> {
    decode(std::io::Cursor::new(include_bytes!("../assets/toast.png")))
}

pub fn decode(png: impl std::io::BufRead + std::io::Seek) -> Option<Icon> {
    let mut decoder = png::Decoder::new(png);
    // Every kind of PNG comes out as eight bits each of what it has.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut bytes = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut bytes).ok()?;
    let pixel = |p: &[u8]| -> u32 {
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (p[0], p[1], p[2], p[3]),
            png::ColorType::Rgb => (p[0], p[1], p[2], 255),
            png::ColorType::GrayscaleAlpha => (p[0], p[0], p[0], p[1]),
            _ => (p[0], p[0], p[0], 255),
        };
        u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    };
    let pixels = bytes[..info.buffer_size()].chunks_exact(info.color_type.samples()).map(pixel).collect();
    Some(Icon { width: info.width as usize, height: info.height as usize, pixels })
}
