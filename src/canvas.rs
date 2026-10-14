//! Drawing into a window's pixels: cards with rounded corners and soft
//! shadows, text, icons. clutch's primitives, font and icons, made to
//! draw with opacity: a pixel is 0xAARRGGBB with its color already
//! weighed by its opacity, which is what a compositor wants, and what
//! lets one card fade over another.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::mpsc;

/// The icons, as letters of the icon font: tools/icons.txt.
pub mod icons {
    pub const X: char = '\u{e1b1}';
    pub const SEARCH: char = '\u{e154}';
    pub const CHECK_FILLED: char = '\u{f001}';
    pub const BELL: char = '\u{f010}';
    pub const BELL_OFF: char = '\u{f011}';
    pub const INFO_FILLED: char = '\u{f017}';
    pub const ALERT_FILLED: char = '\u{f018}';
    pub const DANGER_FILLED: char = '\u{f019}';
}

/// Drawing over a window's pixels. Everything is clipped.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
    /// Where drawing is kept to, if anywhere.
    clip: Option<(i32, i32, i32, i32)>,
}

/// A small picture with its own transparency: a program's icon. Its
/// pixels are 0xAARRGGBB, not weighed.
#[derive(Clone, Debug, Default)]
pub struct Icon {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

/// A card: where it is, how round, its colors, and how opaque it is.
#[derive(Clone, Copy)]
pub struct Card {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub radius: f64,
    pub fill: u32,
    pub edge: u32,
    /// Out of 1.
    pub alpha: f64,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32) -> Self {
        Canvas { pixels, width: width as i32, height: height as i32, clip: None }
    }

    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Keeps what is drawn to a rectangle, or with none, lets it go anywhere.
    pub fn clip(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        self.clip = rect;
    }

    /// Puts `color`, `amount` of 256 of it, over the pixel at `(x, y)`.
    fn over(&mut self, x: i32, y: i32, color: u32, amount: u32) {
        if amount == 0 || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        if let Some((left, top, across, down)) = self.clip {
            if x < left || y < top || x >= left + across || y >= top + down {
                return;
            }
        }
        let pixel = &mut self.pixels[(y * self.width + x) as usize];
        *pixel = over(*pixel, color, amount.min(256));
    }

    /// The rows of a rectangle that are on the canvas, as ranges.
    fn span(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, i32, i32) {
        (x0.floor().max(0.0) as i32, y0.floor().max(0.0) as i32, (x1.ceil() as i32).min(self.width), (y1.ceil() as i32).min(self.height))
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32, alpha: f64) {
        let amount = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
        for py in y.max(0)..(y + h).min(self.height) {
            for px in x.max(0)..(x + w).min(self.width) {
                self.over(px, py, color, amount);
            }
        }
    }

    /// A rounded rectangle, filled, `alpha` opaque.
    #[allow(clippy::too_many_arguments)]
    pub fn round_fill(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64, color: u32, alpha: f64) {
        let (x0, y0, x1, y1) = self.span(x, y, x + w, y + h);
        for py in y0..y1 {
            for px in x0..x1 {
                let cover = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                if cover > 0.0 {
                    self.over(px, py, color, (cover * alpha * 256.0) as u32);
                }
            }
        }
    }

    /// A card: its fill with a line of `edge` round it, in one go, so
    /// that a card fading out does not show its line through its fill.
    pub fn card(&mut self, card: &Card) {
        let Card { x, y, w, h, radius, fill, edge, alpha } = *card;
        if alpha <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x0, y0, x1, y1) = self.span(x, y, x + w, y + h);
        for py in y0..y1 {
            for px in x0..x1 {
                let outer = rounded_cover(px, py, x, y, w, h, radius, 0.0);
                if outer <= 0.0 {
                    continue;
                }
                let inner = rounded_cover(px, py, x, y, w, h, radius, 1.0);
                // The line where the outer shape is and the inner is not.
                let color = mix(edge, fill, (inner / outer * 256.0) as u32);
                self.over(px, py, color, (outer * alpha * 256.0) as u32);
            }
        }
    }

    /// A shadow under a card: the card's shape moved down by `drop`,
    /// fading to nothing over `blur`, `strength` dark where it is solid.
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64, drop: f64, blur: f64, strength: f64) {
        if strength <= 0.0 {
            return;
        }
        let (y, blur) = (y + drop, blur.max(1.0));
        let (x0, y0, x1, y1) = self.span(x - blur, y - blur, x + w + blur, y + h + blur);
        for py in y0..y1 {
            for px in x0..x1 {
                let outside = rounded_distance(px, py, x, y, w, h, radius);
                // Half the blur falls inside the shape, as a blurred
                // shadow does; a curve that eases to nothing beyond.
                let t = ((outside + blur / 2.0) / blur).clamp(0.0, 1.0);
                let amount = strength * (1.0 - t) * (1.0 - t);
                self.over(px, py, 0, (amount * 256.0) as u32);
            }
        }
    }

    /// Draws an icon to fill a square of side `size`, over what is there.
    /// Each pixel drawn is the average of the part of the icon it covers,
    /// which is what keeps a large icon made small from looking torn.
    pub fn icon(&mut self, icon: &Icon, x: i32, y: i32, size: i32, alpha: f64) {
        if size <= 0 || icon.width == 0 || icon.height == 0 || icon.pixels.len() < icon.width * icon.height {
            return;
        }
        let size = size as usize;
        for row in 0..size {
            let (top, bottom) = (row * icon.height / size, ((row + 1) * icon.height).div_ceil(size).max(row * icon.height / size + 1));
            for column in 0..size {
                let (left, right) = (column * icon.width / size, ((column + 1) * icon.width).div_ceil(size).max(column * icon.width / size + 1));
                // Colors are weighed by how opaque they are, or the
                // unseen color of a clear pixel would bleed in.
                let (mut a, mut r, mut g, mut b, mut count) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for sy in top..bottom.min(icon.height) {
                    for pixel in &icon.pixels[sy * icon.width + left..sy * icon.width + right.min(icon.width)] {
                        let opacity = u64::from(pixel >> 24);
                        a += opacity;
                        r += u64::from(pixel >> 16 & 0xFF) * opacity;
                        g += u64::from(pixel >> 8 & 0xFF) * opacity;
                        b += u64::from(pixel & 0xFF) * opacity;
                        count += 1;
                    }
                }
                if a == 0 || count == 0 {
                    continue;
                }
                let color = ((r / a) << 16 | (g / a) << 8 | (b / a)) as u32;
                let opacity = (a / count) as f64 / 255.0;
                self.over(x + column as i32, y + row as i32, color, (opacity * alpha * 256.0) as u32);
            }
        }
    }

    /// Every pixel solid, put over the picture of what is behind the
    /// window, for a desktop that cannot see through windows. Says which
    /// pixels have anything of ours in them, or are in `keep`, as the
    /// rows of rectangles the window is to be cut to.
    pub fn over_picture(&mut self, behind: &[u32], keep: Option<(i32, i32, i32, i32)>) -> Vec<(i32, i32, i32)> {
        let mut runs = Vec::new();
        for y in 0..self.height {
            let mut began = None;
            for x in 0..=self.width {
                let at = (y * self.width + x) as usize;
                let alpha = if x < self.width { self.pixels[at] >> 24 } else { 0 };
                let kept = keep.is_some_and(|(left, top, across, down)| x >= left && y >= top && x < left + across && y < top + down);
                if x < self.width && (alpha > 2 || kept) {
                    // The pixel of ours is already weighed: what is left
                    // of what is behind shows through it.
                    let under = behind.get(at).copied().unwrap_or(0) | 0xFF00_0000;
                    self.pixels[at] = add(scale(under, 256 - alpha), self.pixels[at]) | 0xFF00_0000;
                    began.get_or_insert(x);
                } else if let Some(from) = began.take() {
                    runs.push((y, from, x - from));
                }
            }
        }
        runs
    }

    /// The rows of rectangles of the pixels more than `least` of 255
    /// opaque, and of `keep`: what of the window can be clicked.
    pub fn solid_runs(&self, least: u32, keep: Option<(i32, i32, i32, i32)>) -> Vec<(i32, i32, i32)> {
        let mut runs = Vec::new();
        for y in 0..self.height {
            let mut began = None;
            for x in 0..=self.width {
                let kept = keep.is_some_and(|(left, top, across, down)| x >= left && y >= top && x < left + across && y < top + down);
                let solid = x < self.width && (kept || self.pixels[(y * self.width + x) as usize] >> 24 > least);
                match (solid, began) {
                    (true, None) => began = Some(x),
                    (false, Some(from)) => {
                        runs.push((y, from, x - from));
                        began = None;
                    }
                    _ => {}
                }
            }
        }
        runs
    }
}

/// `color`, `amount` of 256 opaque, put over the weighed pixel `under`.
pub fn over(under: u32, color: u32, amount: u32) -> u32 {
    // The color weighed by its amount, alpha and all.
    let source = scale(color | 0xFF00_0000, amount);
    let left = 256 - (source >> 24);
    let lerp = |mask: u32| ((under & mask) * left >> 8) & mask;
    let kept = (((under >> 24) * left) >> 8) << 24 | lerp(0xFF00FF) | lerp(0x00FF00);
    add(kept, source)
}

/// A weighed pixel made `amount` of 256 as strong: color and opacity both.
fn scale(pixel: u32, amount: u32) -> u32 {
    let alpha = ((pixel >> 24) * amount) >> 8;
    let lerp = |mask: u32| ((pixel & mask) * amount >> 8) & mask;
    alpha << 24 | lerp(0xFF00FF) | lerp(0x00FF00)
}

fn add(a: u32, b: u32) -> u32 {
    let channel = |shift: u32| ((a >> shift & 0xFF) + (b >> shift & 0xFF)).min(255) << shift;
    channel(24) | channel(16) | channel(8) | channel(0)
}

/// Blends `amount`/256 of `b` into `a`, two channels at a time.
pub fn mix(a: u32, b: u32, amount: u32) -> u32 {
    let amount = amount.min(256);
    let lerp = |mask: u32| (((a & mask) * (256 - amount) + (b & mask) * amount) >> 8) & mask;
    lerp(0xFF00FF) | lerp(0x00FF00)
}

/// How far the middle of the pixel at `(px, py)` is outside a rectangle
/// with corners of `radius`: less than nothing inside it.
fn rounded_distance(px: i32, py: i32, x: f64, y: f64, w: f64, h: f64, radius: f64) -> f64 {
    let (half_w, half_h) = (w / 2.0, h / 2.0);
    let radius = radius.clamp(0.0, half_w.min(half_h).max(0.0));
    let across = (f64::from(px) + 0.5 - x - half_w).abs() - (half_w - radius);
    let down = (f64::from(py) + 0.5 - y - half_h).abs() - (half_h - radius);
    across.max(0.0).hypot(down.max(0.0)) + across.max(down).min(0.0) - radius
}

/// How much of the pixel at `(px, py)`, from 0 to 1, lies within a
/// rectangle with corners of `radius`, shrunk by `inset` on every side.
/// Fractions along the edge are what make the curve smooth.
#[allow(clippy::too_many_arguments)]
fn rounded_cover(px: i32, py: i32, x: f64, y: f64, w: f64, h: f64, radius: f64, inset: f64) -> f64 {
    let outside = rounded_distance(px, py, x + inset, y + inset, w - 2.0 * inset, h - 2.0 * inset, radius - inset);
    (0.5 - outside).clamp(0.0, 1.0)
}

/// The font toast is set in: clutch's and jog's.
pub const GEIST: &[u8] = include_bytes!("../assets/Geist-Regular.ttf");

/// Which font the icons are, where the others are counted from 0.
const ICONS: usize = usize::MAX;

fn is_icon(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}

/// Text rendering with the font toast carries, and the icons, which are
/// letters of a font of their own.
pub struct Text {
    font: Option<fontdue::Font>,
    icons: Option<fontdue::Font>,
    /// Fonts for the letters the main one lacks, Korean say, each found
    /// through fontconfig the first time such a letter comes up. A font
    /// for a large script takes a while to load, so that happens on
    /// another thread; until it is in, the letter is not drawn.
    others: Vec<fontdue::Font>,
    loading: Option<mpsc::Receiver<Option<fontdue::Font>>>,
    /// Letters a font has been looked for, so each is only asked about once.
    sought: HashSet<char>,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Text {
    pub fn load() -> Self {
        let load = |bytes: &[u8]| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok();
        let font = load(GEIST);
        let icons = load(include_bytes!("../assets/solar.ttf"));
        Text { font, icons, others: Vec::new(), loading: None, sought: HashSet::new(), glyphs: HashMap::new() }
    }

    pub fn line_height(px: u32) -> i32 {
        (px * 4 / 3) as i32
    }

    /// Whether a font is still on its way, after which text drawn in the
    /// meantime will want drawing again.
    pub fn waiting(&self) -> bool {
        self.loading.is_some()
    }

    /// Which font has letter `c`: 0 for the main one, otherwise one of the
    /// others, counted from 1. Looks for a font that has it if none does.
    fn font_with(&mut self, c: char) -> usize {
        if is_icon(c) {
            return ICONS;
        }
        let has = |font: &fontdue::Font| font.lookup_glyph_index(c) != 0;
        if self.font.as_ref().is_none_or(has) || c.is_control() {
            return 0;
        }
        if let Some(Ok(found)) = self.loading.as_ref().map(mpsc::Receiver::try_recv) {
            self.loading = None;
            self.others.extend(found);
            // What was left out can now be drawn.
            self.glyphs.clear();
        }
        if let Some(at) = self.others.iter().position(has) {
            return at + 1;
        }
        if self.loading.is_none() && self.sought.insert(c) {
            let (sender, receiver) = mpsc::channel();
            self.loading = Some(receiver);
            std::thread::spawn(move || drop(sender.send(font_for(c))));
        }
        0
    }

    fn font(&self, which: usize) -> Option<&fontdue::Font> {
        match which {
            0 => self.font.as_ref(),
            ICONS => self.icons.as_ref(),
            other => self.others.get(other - 1),
        }
    }

    /// The size a letter is drawn at: icons a little larger than the text
    /// they sit in, which they would otherwise look small beside.
    fn size(which: usize, px: u32) -> f32 {
        if which == ICONS { px as f32 * 1.15 } else { px as f32 }
    }

    pub fn width(&mut self, text: &str, px: u32) -> i32 {
        let mut width = 0.0;
        for c in text.chars() {
            let which = self.font_with(c);
            width += self.font(which).map_or(0.0, |font| font.metrics(c, Self::size(which, px)).advance_width);
        }
        width.ceil() as i32
    }

    /// `line`, or as much of it as fits in `width` with a mark that it
    /// was cut.
    pub fn clipped(&mut self, line: &str, width: i32, px: u32) -> String {
        if self.width(line, px) <= width {
            return line.to_string();
        }
        let mut kept: Vec<char> = line.chars().collect();
        loop {
            kept.pop();
            let shorter: String = kept.iter().chain(&['\u{2026}']).collect();
            if kept.is_empty() || self.width(&shorter, px) <= width {
                return shorter;
            }
        }
    }

    /// Text broken into lines no wider than `width`, at spaces where it
    /// can be and anywhere where a word is wider than a line; no more
    /// than `most` of them, the last cut short with a mark if it had to be.
    pub fn wrapped(&mut self, text: &str, width: i32, px: u32, most: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        'paragraphs: for paragraph in text.lines() {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let tried = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if self.width(&tried, px) <= width {
                    line = tried;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    if lines.len() > most {
                        break 'paragraphs;
                    }
                }
                // A word longer than a line is broken where it must be.
                let mut rest = word.to_string();
                while self.width(&rest, px) > width && rest.chars().count() > 1 {
                    let mut fits = rest.chars().count() - 1;
                    while fits > 1 && self.width(&rest.chars().take(fits).collect::<String>(), px) > width {
                        fits -= 1;
                    }
                    lines.push(rest.chars().take(fits).collect());
                    rest = rest.chars().skip(fits).collect();
                    if lines.len() > most {
                        break 'paragraphs;
                    }
                }
                line = rest;
            }
            lines.push(line);
            if lines.len() > most {
                break;
            }
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        if lines.len() > most {
            lines.truncate(most);
            if let Some(last) = lines.last_mut() {
                *last = self.clipped(&format!("{last}\u{2026}\u{2026}"), width, px);
                if last.ends_with("\u{2026}\u{2026}") {
                    last.pop();
                }
            }
        }
        lines
    }

    /// Draws one line of text whose line box has its top-left at `(x, y)`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, canvas: &mut Canvas, x: i32, y: i32, px: u32, color: u32, alpha: f64, text: &str) {
        let Some(font) = &self.font else { return };
        let (ascent, descent) = font
            .horizontal_line_metrics(px as f32)
            .map_or((px as f32 * 0.8, px as f32 * -0.2), |m| (m.ascent, m.descent));
        let baseline = y + ((Self::line_height(px) as f32 + ascent + descent) / 2.0).round() as i32;
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }

        let mut pen = x as f32;
        for c in text.chars() {
            if !self.glyphs.contains_key(&(c, px)) {
                let which = self.font_with(c);
                let Some(font) = self.font(which) else { continue };
                let glyph = font.rasterize(c, Self::size(which, px));
                self.glyphs.insert((c, px), glyph);
            }
            let (metrics, coverage) = &self.glyphs[&(c, px)];
            let left = pen.round() as i32 + metrics.xmin;
            // An icon sits in the middle of the line, whatever its font
            // takes for a baseline; letters stand on the text's.
            let top = match is_icon(c) {
                true => y + (Self::line_height(px) - metrics.height as i32) / 2,
                false => baseline - metrics.ymin - metrics.height as i32,
            };
            for (row, line) in coverage.chunks_exact(metrics.width.max(1)).enumerate() {
                for (column, &amount) in line.iter().enumerate() {
                    if amount > 0 {
                        let amount = (f64::from(u32::from(amount) + u32::from(amount >> 7)) * alpha) as u32;
                        canvas.over(left + column as i32, top + row as i32, color, amount);
                    }
                }
            }
            pen += metrics.advance_width;
        }
    }
}

/// A font that has letter `c`, as fontconfig picks one. The file may hold
/// several fonts, in which case fontconfig says which.
fn font_for(c: char) -> Option<fontdue::Font> {
    let pattern = format!("sans:charset={:x}", c as u32);
    let output = Command::new("fc-match").args(["-f", "%{file}\n%{index}", &pattern]).output().ok()?;
    let answer = String::from_utf8(output.stdout).ok()?;
    let (file, index) = answer.split_once('\n')?;
    let settings = fontdue::FontSettings { collection_index: index.trim().parse().unwrap_or(0), ..Default::default() };
    let font = fontdue::Font::from_bytes(std::fs::read(file).ok()?, settings).ok()?;
    (font.lookup_glyph_index(c) != 0).then_some(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_color_over_another() {
        // Solid over anything is itself.
        assert_eq!(over(0xFF11_2233, 0x00AA_BBCC, 256), 0xFFAA_BBCC);
        // Over nothing, half of white is half white, weighed.
        assert_eq!(over(0, 0xFFFFFF, 128), 0x7F7F_7F7F);
        // Nothing over anything leaves it.
        assert_eq!(over(0xFF11_2233, 0xFFFFFF, 0), 0xFF11_2233);
    }

    #[test]
    fn put_over_what_is_behind() {
        let mut pixels = vec![0, 0x7F7F_7F7F, 0xFF00_00FF];
        let mut canvas = Canvas::new(&mut pixels, 3, 1);
        let runs = canvas.over_picture(&[0x00FF_0000; 3], None);
        assert_eq!(runs, vec![(0, 1, 2)]);
        // Half white over red; solid blue stays blue.
        assert_eq!(pixels[1], 0xFFFF_7F7F);
        assert_eq!(pixels[2], 0xFF00_00FF);
    }

    #[test]
    fn text_is_wrapped() {
        let mut text = Text::load();
        let width = text.width("the quick brown", 16);
        let lines = text.wrapped("the quick brown fox jumps over the lazy dog", width, 16, 5);
        assert_eq!(lines, ["the quick brown", "fox jumps over", "the lazy dog"]);
        let lines = text.wrapped("the quick brown fox jumps over the lazy dog", width, 16, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('\u{2026}'));
        assert_eq!(text.wrapped("one\n\ntwo", width, 16, 5), ["one", "", "two"]);
    }
}
