# veil

**Terminal GUI renderer** — capture and encode GUI applications to ASCII art.

> [!IMPORTANT]
> **AI-Assisted Project:** This repository was developed with the help of AI. Whilst I have tried to review the code, I can not guarantee the code to be efficent. **Use caution**

## Warnings & usage
Currently right now, veil isnt built to get used. There are major issues, like frame rendering in cli apps causing your screen to look like a crt, not being able to actually send any mouse and keyboard inputs to it, and actual gui windows just not working. It also is only supported on niri currently but am working to expand that with a modular registry design to swap out components with what you are on from a TTY to a sway compositor. Currently everything needs to be rendered under a config.lua in a /home/abyss/veil (local place) but well I doubt you have that. This isnt meant for use yet. Wait.

## What is it?

Veil renders graphical applications inside your terminal by:
1. Launching GUI apps under Niri (Wayland compositor)
2. Capturing frames with `grim`
3. Converting to luma-based ASCII art with edge detection
4. Overlaying actual text from accessibility APIs (AT-SPI)

Currently optimized for Niri on Linux. Works with any GTK/Qt app.

## Architecture

- **veil-cli** — TUI interface for launching and rendering apps
- **veil-compositor** — Window detection, frame capture pipeline
  - Niri IPC for window discovery
  - grim fullscreen capture
  - Luma computation + hysteresis for character selection
  - AT-SPI text overlay from accessibility tree
- **veil-render** — Character encoding (ASCII luma, edge detection, dirty-row optimization)
- **veil-config** — Lua configuration layer
- **veil-capture** — Zig library for future LD_PRELOAD GPU capture (wl_shm interception, EGL hooks)

## Status

**April 2026**: Basic rendering working. GUI apps (nautilus, zen-browser) render as ASCII. Fullscreen capture includes desktop background (known limitation of Niri coordinate system).

**Next**: GPU rendering path (May 1st start). Proper window region capture via Wayland screencopy protocol or calculated tile positions. Smithay rewrite for full compositor.

## Usage

```bash
# Terminal app
veil run my-tui-app

# GUI app (renders to terminal)
veil run-gui nautilus
veil run-gui zen-browser

# Press Ctrl+C to exit
```

Override FPS or quality:
```bash
veil run-gui --override fps=15 zen-browser
```

## Limitations

- Requires Niri compositor (uses `niri msg --json windows` for window detection)
- Fullscreen capture only (shows desktop background behind app)
- No window positioning — app fills terminal view
- CPU-bound luma computation (GPU path in progress)

## Build

```bash
cargo build --release
./target/release/veil run-gui nautilus
```

Requires: `grim` (Wayland screenshot), `python3` (AT-SPI), `niri` (compositor)

---

**veil** — your apps, your terminal.
