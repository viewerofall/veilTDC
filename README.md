# veil

**Terminal compositor** — render GUI applications inside your terminal with full colour, live resize, and accessibility text overlay.

> [!IMPORTANT]
> **AI-Assisted Project:** Developed with AI assistance. Code reviewed but not guaranteed production-ready. **Use with caution.**

---

## What it does

Veil launches a GUI application, captures its frames from the Wayland compositor, and renders them into your terminal using half-block Unicode characters (`▀`) with 24-bit truecolor — doubling effective vertical resolution. Text from the accessibility tree (AT-SPI) is overlaid on top for readable labels and buttons.

No VNC. No X forwarding. Just your terminal.

---

## Architecture

Multi-language workspace — each component uses the right tool for the job.

```
veil/
├── veil-cli/           Rust — CLI entrypoint, render loop, dirty-row output
├── veil-compositor/    Rust — window detection, capture pipeline, AT-SPI
│   ├── gui.rs              GuiCompositor: launch app, capture thread, IPC polling
│   ├── wayland_capture.rs  zwlr_screencopy_manager_v1 client (Niri + Hyprland)
│   ├── tui.rs              PTY compositor for terminal apps
│   └── capture_shm.rs      SHM reader for LD_PRELOAD captured frames
├── veil-render/        Rust — half-block colour renderer, luma/edge engine
├── veil-config/        Rust — Lua config loader (mlua)
├── veil-screencopy/    C — ext-image-copy-capture-v1 client (Hyprland, future)
├── veil-capture/       Zig — LD_PRELOAD .so: intercepts wl_shm + EGL inside app
└── atspi_query.py      Python — AT-SPI accessibility tree walker
```

### Capture priority chain

On each frame the compositor tries backends in order, falling through on failure:

| Priority | Backend | How |
|----------|---------|-----|
| 1 | **SHM / LD_PRELOAD** | Zig `.so` injected via `LD_PRELOAD`; intercepts `wl_shm_create_pool` and `eglSwapBuffers`, writes frames to `/dev/shm/veil_<pid>` |
| 2 | **wlr-screencopy** | Rust `WaylandCapture` using `zwlr_screencopy_manager_v1`; captures full output then crops to window bounds from compositor IPC |
| 3 | **idle** | No backend available — logs and waits |

Window coordinates are sourced from:
- **Niri**: `niri msg --json windows` → `geometry.{x,y,width,height}`
- **Hyprland**: `hyprctl clients -j` → `at`, `size`, `class`

Window bounds are re-polled every 500 ms inside the capture thread — resize and move are tracked live.

### Render pipeline

```
RGBA frame (physical px)
  │
  ├─ crop_rgba()           slice window region from full output
  │
  └─ rgba_to_halfblocks()  map to ColorCell grid (cols × rows)
       │  ▀ top-half = fg RGB
       │  ▀ bot-half = bg RGB
       │  2× effective vertical resolution
       │
       └─ color_dirty_loop()  \x1b[38;2;R;G;Bm\x1b[48;2;R;G;Bm▀ per cell
                               only redraws rows that changed
```

AT-SPI text overlay runs on a separate 500 ms thread and stamps readable strings over the grid.

---

## Compositor support

| Compositor | Window discovery | Screencopy |
|------------|-----------------|------------|
| **Niri** | `niri msg --json windows` | `zwlr_screencopy_manager_v1` ✓ |
| **Hyprland** | `hyprctl clients -j` | `zwlr_screencopy_manager_v1` ✓ |

`ext-image-copy-capture-v1` (`veil-screencopy` C binary) is built but removed from the active chain — `zwlr` covers both compositors.

---

## Build

Three build systems unified under one root Makefile: Cargo (Rust), Zig, and Make (C).

**Requirements**

- Rust stable toolchain
- Zig 0.15+
- `wayland-scanner`, `pkg-config`, `libwayland-client`
- `python3` + `python-atspi` (`gi.repository.Atspi`)
- Niri or Hyprland compositor at runtime

```bash
# Development build (unoptimised)
make

# Release build — run as your user
make release

# Install system-wide — run as root after make release
sudo make install

# Install to ~/.local — no sudo needed
make install-user

# Uninstall
sudo make uninstall

# Clean all build artifacts
make clean
```

Installed paths:

```
/usr/local/bin/veil
/usr/local/bin/veil-screencopy
/usr/local/lib/veil/libveil_capture.so
```

---

## Usage

```bash
# TUI / terminal app — full PTY passthrough
veil run my-tui-app

# GUI app — captured and rendered in colour
veil run-gui nautilus
veil run-gui zen-browser
veil run-gui kitty

# Override config at runtime
veil run-gui --override fps=15 zen-browser

# Show terminal info and active config
veil probe
```

Press `Ctrl+C` to exit and kill the launched application.

### config.lua

```lua
quality = "kitty"   -- terminal hint (informational)
fps     = 30        -- target render FPS (capture runs at fps/2)
```

Place in the working directory or pass `--config path/to/config.lua`.

---

## Limitations

- **Terminal must support truecolor** — Kitty, Alacritty, foot, WezTerm, etc.
- **No input forwarding** — keyboard and mouse events are not sent to the rendered app
- **AT-SPI text overlay** — runs but only applied on the TUI render path currently
- **SHM/LD_PRELOAD** (`libveil_capture.so`) — EGL interception built but not fully wired into the pipeline yet
- **veil-screencopy** — compiled and installed, not used in the active capture chain (kept for future `ext-image-copy-capture-v1` support)

---

## Status

**May 2026**

- Full-colour half-block rendering (`▀` with 24-bit fg/bg, 2× vertical resolution) ✓
- Window-only capture — full output captured, cropped to window bounds, no desktop bleed ✓
- Live resize and move tracking via 500 ms IPC re-poll ✓
- Niri and Hyprland compositor detection, per-compositor IPC ✓
- `zwlr_screencopy_manager_v1` working on both compositors ✓
- Multi-language build: Rust + C + Zig + Python under one Makefile ✓
- LD_PRELOAD SHM frame injection (Zig) — built, priority 1 when `/dev/shm/veil_<pid>` exists ✓
- AT-SPI text overlay thread ✓
- grim dependency removed ✓

**Planned**

- Input forwarding (keyboard + mouse to captured app)
- AT-SPI overlay on colour render path
- Complete EGL interception pipeline (Zig)
- Kitty Graphics Protocol render mode (native pixel output, no char mapping)

---

**veil** — your apps, your terminal.
