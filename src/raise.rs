//! Brings the editor to the front after a snip.
//!
//! A window that reappears after being hidden is often left behind other
//! windows by the window manager's focus-stealing prevention, so on X11 we
//! ask for activation the way a taskbar does, which window managers honor.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, StackMode};

/// Our window's X11 id, when running on X11.
pub fn x11_window_id(window: &impl HasWindowHandle) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Raises and focuses the window.
pub fn activate_x11(window: u32) {
    let Ok((conn, screen)) = x11rb::connect(None) else { return };
    let root = conn.setup().roots[screen].root;
    let Some(active) = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.atom)
    else {
        return;
    };
    // Source indication 2 means "from a pager or taskbar".
    let event = ClientMessageEvent::new(32, window, active, [2u32, 0, 0, 0, 0]);
    let _ = conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event);
    let _ = conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = conn.flush();
}
