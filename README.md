# VeilTDC
**⚠️ VeilTDC is currently a placeholder repo and there is nothing happening currently with progress on it, development starts April 9th.⚠️** 

**TDC — Terminal Display Compositor**

A Wayland compositor that runs inside your terminal. Veil hosts graphical applications and encodes their framebuffers directly to terminal output, letting you run a full desktop environment over SSH, in tmux, or anywhere a terminal works.

## What is it?

Veil is a **Terminal Display Compositor (TDC)** — it manages Wayland and X11 windows like any other compositor, but instead of rendering to a GPU framebuffer, it encodes frames to your terminal using a quality ladder:

1. **Kitty Graphics Protocol** (best quality)
2. **Sixel** (wide compatibility)
3. **ASCII edge detection** (fast, stylized)
4. **ASCII luminance** (universal fallback)

## Features

- **Full Wayland/X11 support** — runs native graphical applications via Smithay and XWayland
- **Quality ladder** — auto-detects terminal capabilities and system performance
- **TTY boot** — launches directly from a TTY using libseat and DRM
- **Adaptive performance** — CPU speed and core count determine encoding tier
- **Manual overrides** — `--override quality=X,fps=N` for fine control
- **Minimal footprint** — targets i686 and Raspberry Pi Zero 2W as baseline hardware

## Architecture

- **veil-compositor** — Smithay-based Wayland compositor with XWayland integration
- **veil-render** — Zig encoding engine (quality ladder, dirty-rect tracking, frame budgeting)
- **veil-config** — Configuration layer
- **veil-cli** — Command-line interface

## Project Status

Early development. A working skeleton that runs terminal applications exists. Current focus is beta (mid-May) and v1 (mid-June) releases.

## Distribution Plan

- **v1**: AUR packages, curl installer, AppImage, GitHub releases
- **v1.x**: apt PPA, Flathub (nested only), RPM COPR
- **v2**: Windows (WSL2 via winget), macOS (Lima VM via Homebrew)

## Use Cases

- Remote desktop over SSH
- Running graphical apps in tmux/screen sessions
- Headless server GUI access
- Low-bandwidth remote workflows
- Retro computing aesthetics

---

**veilTDC** — because sometimes the terminal is all you need.
