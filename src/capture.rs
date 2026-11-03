//! Screen capture backends.
//!
//! On X11 we read the root window directly and list the visible top-level
//! windows (for window snips). On Wayland apps may not read the screen, so we
//! ask the xdg-desktop-portal Screenshot API for a full-screen image instead.

use eframe::egui::Pos2;
use image::RgbaImage;

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl IRect {
    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    pub fn contains(&self, p: Pos2) -> bool {
        p.x >= self.x as f32
            && p.y >= self.y as f32
            && p.x < self.right() as f32
            && p.y < self.bottom() as f32
    }

    pub fn intersect(&self, other: IRect) -> Option<IRect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| IRect {
            x: x0,
            y: y0,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        })
    }
}

/// A frozen screenshot plus what we need to snip from it.
pub struct Capture {
    pub image: RgbaImage,
    /// Top-left of the captured area in desktop coordinates.
    pub origin: (i32, i32),
    /// Visible top-level windows, topmost first, relative to `image`.
    pub windows: Vec<IRect>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// The monitor under the mouse pointer.
    CursorMonitor,
    /// Every monitor.
    AllMonitors,
}

pub fn capture(area: Area) -> Result<Capture, String> {
    if is_wayland() {
        wayland::capture()
    } else {
        x11::capture(area)
    }
}

fn is_wayland() -> bool {
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(kind) => kind == "wayland",
        Err(_) => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

mod x11 {
    use super::{Area, Capture, IRect};
    use image::RgbaImage;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, Window};
    use x11rb::rust_connection::RustConnection;

    fn err(e: impl std::fmt::Display) -> String {
        format!("X11 capture failed: {e}")
    }

    pub fn capture(area: Area) -> Result<Capture, String> {
        let (conn, screen_num) = x11rb::connect(None).map_err(err)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let desktop = IRect {
            x: 0,
            y: 0,
            w: screen.width_in_pixels as u32,
            h: screen.height_in_pixels as u32,
        };
        let rect = match area {
            Area::AllMonitors => desktop,
            Area::CursorMonitor => cursor_monitor(&conn, root).unwrap_or(desktop),
        };

        let image = grab(&conn, root, rect)?;
        let windows = list_windows(&conn, root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|w| w.intersect(rect))
            .map(|w| IRect { x: w.x - rect.x, y: w.y - rect.y, ..w })
            .collect();

        Ok(Capture { image, origin: (rect.x, rect.y), windows })
    }

    fn cursor_monitor(conn: &RustConnection, root: Window) -> Option<IRect> {
        let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
        let monitors = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
        monitors
            .monitors
            .iter()
            .map(|m| IRect { x: m.x as i32, y: m.y as i32, w: m.width as u32, h: m.height as u32 })
            .find(|m| {
                let (px, py) = (pointer.root_x as i32, pointer.root_y as i32);
                px >= m.x && py >= m.y && px < m.right() && py < m.bottom()
            })
    }

    fn grab(conn: &RustConnection, root: Window, r: IRect) -> Result<RgbaImage, String> {
        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, root, r.x as i16, r.y as i16, r.w as u16, r.h as u16, !0)
            .map_err(err)?
            .reply()
            .map_err(err)?;
        let setup = conn.setup();
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == reply.depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(0);
        if bpp != 32 {
            return Err(format!("unsupported X11 pixel format ({} bpp, depth {})", bpp, reply.depth));
        }
        let lsb = setup.image_byte_order == ImageOrder::LSB_FIRST;
        let mut rgba = Vec::with_capacity((r.w * r.h * 4) as usize);
        for px in reply.data.chunks_exact(4) {
            let (b, g, red) = if lsb { (px[0], px[1], px[2]) } else { (px[3], px[2], px[1]) };
            rgba.extend_from_slice(&[red, g, b, 255]);
        }
        RgbaImage::from_raw(r.w, r.h, rgba).ok_or_else(|| err("short image reply"))
    }

    fn list_windows(conn: &RustConnection, root: Window) -> Option<Vec<IRect>> {
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
        let stacking = atom("_NET_CLIENT_LIST_STACKING")?;
        let extents = atom("_NET_FRAME_EXTENTS")?;

        let list = conn
            .get_property(false, root, stacking, AtomEnum::WINDOW, 0, u32::MAX)
            .ok()?
            .reply()
            .ok()?;
        let clients: Vec<Window> = list.value32()?.collect();

        let mut rects = Vec::new();
        // The stacking list is bottom-to-top; we want topmost first.
        for &win in clients.iter().rev() {
            let Some(attrs) = conn.get_window_attributes(win).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            if attrs.map_state != MapState::VIEWABLE {
                continue;
            }
            let Some(geom) = conn.get_geometry(win).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            let Some(pos) = conn.translate_coordinates(win, root, 0, 0).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            // Include the window manager's frame (title bar and borders).
            let frame: Vec<u32> = conn
                .get_property(false, win, extents, AtomEnum::CARDINAL, 0, 4)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().map(|v| v.collect()))
                .unwrap_or_default();
            let [left, right, top, bottom] = <[u32; 4]>::try_from(frame).unwrap_or([0; 4]);
            rects.push(IRect {
                x: pos.dst_x as i32 - left as i32,
                y: pos.dst_y as i32 - top as i32,
                w: geom.width as u32 + left + right,
                h: geom.height as u32 + top + bottom,
            });
        }
        Some(rects)
    }
}

mod wayland {
    use super::Capture;
    use ashpd::desktop::screenshot::Screenshot;
    use std::sync::OnceLock;
    use tokio::runtime::Runtime;

    /// ashpd keeps one D-Bus connection for the whole process, and its tasks
    /// run on the runtime that opened it. A runtime per capture would leave
    /// the second capture waiting forever on a dead connection.
    fn runtime() -> Result<&'static Runtime, String> {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        if let Some(runtime) = RUNTIME.get() {
            return Ok(runtime);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(RUNTIME.get_or_init(|| runtime))
    }

    pub fn capture() -> Result<Capture, String> {
        let uri = runtime()?
            .block_on(async {
                let request = Screenshot::request().interactive(false).modal(false).send().await?;
                Ok::<_, ashpd::Error>(request.response()?.uri().as_str().to_owned())
            })
            .map_err(|e| format!("screenshot portal failed: {e}"))?;

        let path = uri
            .strip_prefix("file://")
            .map(percent_decode)
            .ok_or_else(|| format!("screenshot portal returned an unexpected URI: {uri}"))?;
        let image = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
        // The portal saved a file just for us; don't leave it behind.
        let _ = std::fs::remove_file(&path);

        // The portal doesn't tell us where windows are, so window snips fall
        // back to rectangle snips.
        Ok(Capture { image, origin: (0, 0), windows: Vec::new() })
    }

    fn percent_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}
