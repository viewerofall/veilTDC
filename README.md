# veil

**Nested Wayland compositor → terminal renderer.**
Run any GUI app inside your terminal — full colour, live input, clipboard, resize.

> [!IMPORTANT]
> **AI-Assisted Project:** Developed with AI assistance. Code reviewed but not guaranteed production-ready.

> [!NOTE]
> **The original screencopy-based pipeline (`veil-compositor` / `veil-cli`) is discontinued.**
> It has been archived on the [`legacy/v1`](../../tree/legacy/v1) branch — cage isolation, wlr-screencopy capture, halfblock renderer. All development is now on `veil-host`.

---

## How it works

`veil-host` is a Smithay-based nested Wayland compositor. It:

1. Opens its own `WAYLAND_DISPLAY` socket
2. Spawns the target app pointed at that socket
3. Receives SHM buffer commits (shm path; GPU/dmabuf apps fall back to software automatically)
4. Composites surfaces + subsurfaces + popups + cursor into a single RGBA frame
5. Encodes and streams that frame to the terminal via Kitty graphics / half-block Unicode / ASCII

Input flows back: keyboard, mouse (click, drag, scroll), and clipboard (both directions) are fully bridged.

---

## Architecture

```
veil/
├── veil-host/      Smithay nested compositor + standalone binary
│   ├── server.rs       Wayland globals, dispatch loop, software compositor
│   ├── main.rs         Terminal I/O, render dispatch, CLI
│   ├── input.rs        InputCmd enum
│   └── sink.rs         Frame struct
├── veil-render/    Render engines (kitty, halfblock, ascii, ascii-edge)
└── veil-config/    Lua config loader + terminal auto-detection
```

### Compositor globals

| Global | Status |
|---|---|
| `wl_compositor` + `wl_subcompositor` | ✓ full |
| `xdg_shell` (toplevel + popup) | ✓ full |
| `wl_seat` (keyboard, pointer, touch) | ✓ full |
| `wl_shm` | ✓ full |
| `wl_data_device_manager` (clipboard) | ✓ full |
| `wl_output` + `xdg_output` | ✓ full |
| `xdg_activation` | ✓ stub (always grant) |
| `xdg_decoration` | ✓ client-side always |
| `zwp_linux_dmabuf_v1` | stub — advertised, every import fails → shm fallback |
| `wp_viewporter` / `wp_fractional_scale` / `wp_presentation_time` | globals only |
| XWayland | ✓ spawned at startup, X11 apps work |

---

## Install

### From a release (recommended)

```bash
curl -fsSL https://viewerofall.pages.dev/install/veil/install.sh | bash
```

or with wget:

```bash
wget -qO- https://viewerofall.pages.dev/install/veil/install.sh | bash
```

Installs to `/usr/local/bin/veil-host` (uses `sudo` if needed). Supports `x86_64` and `aarch64`.

Options:

```bash
# Install to ~/.local/bin (no sudo)
INSTALL_DIR=~/.local/bin curl -fsSL .../install.sh | bash

# Pin a specific version
VERSION=v0.1.0 curl -fsSL .../install.sh | bash
```

### From source

Requirements: Rust stable, a Wayland compositor running (Niri, Hyprland, etc.).

```bash
git clone https://github.com/viewerofall/veilTDC.git
cd veilTDC

# Build
cargo build --release -p veil-host   # → target/release/veil-host

# Install system-wide
sudo install -Dm755 target/release/veil-host /usr/local/bin/veil-host

# Install to ~/.local (no sudo)
install -Dm755 target/release/veil-host ~/.local/bin/veil-host
```

---

## Usage

```bash
# Run any GUI app
veil-host run thunar
veil-host run firefox
veil-host run weston-terminal

# Flags
veil-host run -d thunar                  # debug log → /tmp/veil.log
veil-host run -m halfblock thunar        # force render mode
veil-host run -m ascii-edge thunar       # edge-detection ascii
veil-host run -w 1920 -h 1080 thunar     # explicit compositor resolution
veil-host run -s wayland-veil-1 thunar   # custom socket name

# Probe terminal capabilities and resolved config
veil-host probe
```

The compositor size defaults to `terminal_cols × 8` × `terminal_rows × 16` (standard 8×16 cell assumption). Resize the terminal window and the compositor reconfigures live.

Press `Ctrl+C` to exit. veil-host shuts down automatically when the hosted app exits.

---

## Render modes

| Mode | Flag | Terminal requirement | Notes |
|---|---|---|---|
| **Kitty** | `kitty` | Kitty graphics protocol | Best quality, native pixel output |
| **Halfblock** | `halfblock` | 24-bit truecolor | `▀` Unicode, 2× effective vertical resolution |
| **ASCII** | `ascii` | Any | Luma → ASCII character map |
| **ASCII Edge** | `ascii-edge` | Any | Luma with hysteresis edge-detection for sharper output |

Without `-m`, the mode is resolved automatically:

1. Check `quality` in `config.lua`
2. If `auto`, detect from `$TERM` / `$COLORTERM`:
   - `xterm-kitty` or WezTerm → Kitty
   - `truecolor` / `24bit` → Halfblock
   - Otherwise → ASCII

---

## config.lua

Place at `./config.lua` or `~/.config/veil/config.lua`.

```lua
quality = "auto"   -- auto | kitty | pixel | ascii | ascii_edge | ascii_luma
fps     = 60       -- compositor frame rate cap
```

`veil-host probe` shows which config file was loaded and the resolved mode.

---

## Clipboard

Full bidirectional bridge:

- **App → host**: when the hosted app copies text, it appears in your host clipboard
- **Host → app**: host clipboard is polled every second and offered to the hosted app when it has no active selection

---

## Running browsers (and other keyring apps) under veil

> [!NOTE]
> **A browser opened under veil can look like its profile was wiped — logged out
> everywhere, saved passwords gone. Your data is safe.** It's the same profile
> directory on the same disk; nothing is deleted or moved.
>
> Chromium/Firefox encrypt cookies and passwords with a key held in your
> **keyring** (`gnome-keyring` / `org.freedesktop.secrets`), which your display
> manager unlocks via PAM at login. A session started *outside* that login path —
> e.g. a bare compositor (`niri`, `Hyprland`) hosted under veil — never gets that
> key handed to it, so the browser can't decrypt your existing logins and shows a
> fresh-looking session. Bookmarks and history (unencrypted) are still there.
>
> **It returns to normal in your real login session** — the key is available
> again and the untouched files decrypt as usual. To avoid churn, close the
> browser before leaving veil and don't bother re-signing-in inside it.
>
> If you use **profile-sync-daemon (psd)**, be extra careful: psd mirrors a
> profile to tmpfs and syncs it back on exit, so a "reset"-looking session could
> be written over your good backup. Close psd-managed browsers before running
> them under a nested session.

## Limitations

- **dmabuf / GPU import** — Chromium, Electron, Firefox require `zwp_linux_dmabuf_v1` with real GBM/EGL import. Planned for v1.5. For now, set `LIBGL_ALWAYS_SOFTWARE=1` (done automatically) to push them to shm.
- **Single app** — one toplevel client per compositor instance. Multi-app support is a v2 goal.
- **Cell resolution mouse** — terminal mouse events are cell-granularity, not pixel-granularity.

---

## What's next

- **v1.5** — real dmabuf import (GBM + EGL) for GPU apps
- **v2** — multi-app support, `veil-cli` migration to consume veil-host
- **Expansion** — Linux TTY / framebuffer sink, Sixel, iTerm2 protocol

---

**veil** — your apps, your terminal.
