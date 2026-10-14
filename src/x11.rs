//! What toast asks of an X11 desktop that a window cannot do through
//! winit: where the screen is free of panels, whether anything there
//! can see through windows, a picture of what is behind one where
//! nothing can, and cutting a window to the shape of what is drawn.

use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{AtomEnum, ClipOrdering, ConnectionExt as _, ImageFormat, Rectangle};
use x11rb::rust_connection::RustConnection;

/// Left, top, across, down.
pub type Area = (i32, i32, i32, i32);

pub struct X11 {
    conn: RustConnection,
    root: u32,
    screen: usize,
}

impl X11 {
    pub fn open() -> Option<X11> {
        let (conn, screen) = RustConnection::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(X11 { conn, root, screen })
    }

    fn atom(&self, name: &str) -> u32 {
        self.conn.intern_atom(false, name.as_bytes()).ok().and_then(|c| c.reply().ok()).map_or(0, |r| r.atom)
    }

    /// The numbers a window keeps under a name.
    fn numbers(&self, window: u32, name: &str, most: u32) -> Vec<u32> {
        let asked = self.conn.get_property(false, window, self.atom(name), AtomEnum::ANY, 0, most).ok().and_then(|c| c.reply().ok());
        asked.and_then(|reply| reply.value32().map(Iterator::collect)).unwrap_or_default()
    }

    fn bytes(&self, window: u32, name: &str) -> Vec<u8> {
        let asked = self.conn.get_property(false, window, self.atom(name), AtomEnum::ANY, 0, 1 << 16).ok().and_then(|c| c.reply().ok());
        asked.map(|reply| reply.value).unwrap_or_default()
    }

    /// Whether a compositor is running, which sees through windows.
    pub fn compositing(&self) -> bool {
        let selection = self.atom(&format!("_NET_WM_CM_S{}", self.screen));
        self.conn.get_selection_owner(selection).ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.owner != x11rb::NONE)
    }

    /// What the desktop has set Xft.dpi to, over the 96 that is plain.
    pub fn text_scale(&self) -> f64 {
        let resources = String::from_utf8_lossy(&self.bytes(self.root, "RESOURCE_MANAGER")).into_owned();
        let dpi = resources.lines().find_map(|line| line.strip_prefix("Xft.dpi:")?.trim().parse::<f64>().ok());
        dpi.map_or(1.0, |dpi| (dpi / 96.0).clamp(0.5, 4.0))
    }

    fn monitors(&self) -> Vec<Area> {
        use x11rb::protocol::randr::ConnectionExt as _;
        let asked = self.conn.randr_get_monitors(self.root, true).ok().and_then(|c| c.reply().ok());
        let area = |m: &x11rb::protocol::randr::MonitorInfo| (i32::from(m.x), i32::from(m.y), i32::from(m.width), i32::from(m.height));
        let mut monitors: Vec<Area> = asked.map_or(Vec::new(), |reply| reply.monitors.iter().map(area).collect());
        monitors.sort();
        monitors
    }

    /// The monitor the pointer is on, which is where one is looking, less
    /// the panels along its edges; and how tall the monitor is.
    pub fn area_in_use(&self) -> Option<(Area, i32)> {
        let pointer = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        let (x, y) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
        let monitors = self.monitors();
        let holds = |(left, top, width, height): &Area| (*left..left + width).contains(&x) && (*top..top + height).contains(&y);
        let monitor = monitors.iter().find(|area| holds(area)).or(monitors.first()).copied()?;
        // The work area is said for all monitors at once; on one of
        // several it is only of use where it falls within it.
        let desktop = self.numbers(self.root, "_NET_CURRENT_DESKTOP", 1).first().copied().unwrap_or(0) as usize;
        let areas = self.numbers(self.root, "_NET_WORKAREA", 256);
        let work = areas.chunks_exact(4).nth(desktop).map(|a| (a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32));
        let free = work.and_then(|work| meet(monitor, work)).unwrap_or(monitor);
        Some((free, monitor.3))
    }

    /// A picture of what is on the screen in an area, a pixel to a number.
    pub fn picture(&self, (x, y, width, height): Area) -> Vec<u32> {
        let asked = self.conn.get_image(ImageFormat::Z_PIXMAP, self.root, x as i16, y as i16, width as u16, height as u16, !0).ok().and_then(|c| c.reply().ok());
        let Some(reply) = asked.filter(|reply| reply.data.len() >= (width * height * 4) as usize) else {
            return vec![0; (width * height).max(0) as usize];
        };
        reply.data.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0x00FF_FFFF).collect()
    }

    /// Cuts a window to rows of rectangles: what is seen of it, or only
    /// what can be clicked.
    pub fn cut(&self, window: u32, runs: &[(i32, i32, i32)], seen: bool) {
        let rectangles: Vec<Rectangle> = runs.iter().map(|&(y, x, w)| Rectangle { x: x as i16, y: y as i16, width: w as u16, height: 1 }).collect();
        let kind = if seen { SK::BOUNDING } else { SK::INPUT };
        let _ = self.conn.shape_rectangles(SO::SET, kind, ClipOrdering::UNSORTED, window, 0, 0, &rectangles);
        let _ = self.conn.flush();
    }
}

/// Where two areas overlap, if they do.
fn meet(a: Area, b: Area) -> Option<Area> {
    let (left, top) = (a.0.max(b.0), a.1.max(b.1));
    let (right, bottom) = ((a.0 + a.2).min(b.0 + b.2), (a.1 + a.3).min(b.1 + b.3));
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_areas_meet() {
        assert_eq!(meet((0, 0, 1920, 1080), (0, 0, 3840, 1040)), Some((0, 0, 1920, 1040)));
        assert_eq!(meet((1920, 0, 1920, 1080), (0, 30, 3840, 1050)), Some((1920, 30, 1920, 1050)));
        assert_eq!(meet((0, 0, 10, 10), (20, 20, 5, 5)), None);
    }
}
