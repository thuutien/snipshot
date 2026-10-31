# Snipshot

A snipping tool for Linux, modeled on the Windows Snipping Tool, written in Rust with [egui](https://github.com/emilk/egui).

## Features

- **Snip modes:** rectangle, window, full screen, freeform
- **Delay:** none, 3, 5 or 10 seconds
- **Markup:** ballpoint pen and highlighter (with color palette and size), shapes (rectangle, oval, line, arrow; hold Shift for squares, circles and 15° angles), text labels (click to type, click to edit, drag to move), eraser, image crop, undo/redo, zoom
- **Output:** copies the snip to the clipboard and saves it to `~/Pictures/Screenshots` automatically (both can be turned off in the `•••` menu). Ctrl+S saves as PNG or JPEG.

## Build and run

```bash
cargo build --release
./target/release/snipshot
```

Start a snip straight away, for example from a keyboard shortcut:

```bash
snipshot --rect            # or --window, --fullscreen, --freeform
snipshot --rect --delay 3
```

On Cinnamon, open **Keyboard → Shortcuts → Custom Shortcuts** and bind e.g. `Shift+Super+S` to `snipshot --rect`.

## Install

Download `snipshot-<version>-x86_64-linux.tar.gz` from the [latest release](https://github.com/thuutien/snipshot/releases/latest), unpack it and run the install script. It installs the binary to `~/.local/bin`, plus the app icon and an app-menu entry:

```bash
tar xzf snipshot-*-x86_64-linux.tar.gz
cd snipshot-*-x86_64-linux
packaging/install.sh
```

From source:

```bash
cargo build --release
packaging/install.sh
```

## Updates

Snipshot checks GitHub releases of the repository in `Cargo.toml` (`repository = ...`) at most once a day, and shows a banner when a newer version is published. Turn it off, or check by hand, in the `•••` menu. To publish an update, bump `version` in `Cargo.toml` and create a GitHub release tagged like `v0.2.0`.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+N | New snip |
| Esc | Cancel snip / cancel crop |
| Enter | Apply crop |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Ctrl+C | Copy |
| Ctrl+S | Save as |
| Ctrl + / Ctrl − / Ctrl+0 | Zoom in / out / fit |

## Platform notes

- **X11:** the screen is captured directly, and window snips use the window manager's window list.
- **Wayland:** the screen is captured through the xdg-desktop-portal Screenshot API. Wayland doesn't let apps see where other windows are, so window snips act like rectangle snips there.
- The overlay covers the monitor under the mouse pointer. Full-screen snips capture every monitor.
- On X11 the clipboard contents belong to the running app. If you close Snipshot without a clipboard manager running, the copied snip is gone.

## Code layout

| File | Purpose |
|---|---|
| `src/main.rs` | Command-line flags, window setup |
| `src/app.rs` | Main window, screen switching, copy/save |
| `src/capture.rs` | X11 and Wayland screen capture, window list |
| `src/overlay.rs` | Full-screen snip overlay and the snip modes |
| `src/editor.rs` | Markup editor: tools, crop, undo, zoom |
| `src/raster.rs` | Burns ink strokes into the saved image |
| `src/text.rs` | Text labels: on-screen drawing and burning into the image |
| `src/updater.rs` | Checks GitHub for a newer release |
| `src/icons.rs` | Toolbar icons drawn in code |
| `src/settings.rs` | Settings in `~/.config/snipshot/settings.conf` |
| `assets/` | App icon (SVG source and 256 px PNG) |
| `packaging/` | Desktop entry and per-user install script |
