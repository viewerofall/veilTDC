# veil — legacy/v1 (archived)

> **This is an archived snapshot of the original veil implementation.**
> Active development continues on `main` with `veil-host`.

---

## What this was

veil v1 launched GUI apps inside `cage`, captured frames via `zwlr_screencopy_manager_v1`, and rendered them as Unicode halfblocks (`▀`) with 24-bit truecolor in any true-color terminal. Keyboard and mouse were injected back into the cage compositor via virtual input protocols.

## Workspace layout

```
veil/
├── veil-cli/        Rust — entry point (veil run-gui, veil run, veil probe)
├── veil-compositor/ Rust — cage launch, screencopy capture, input injection
├── veil-render/     Rust — halfblock colour + luma/edge ASCII render paths
├── veil-config/     Rust — Lua config loader (mlua)
├── veil-capture/    Zig  — LD_PRELOAD .so for EGL frame interception (experimental)
└── atspi_query.py   Python — AT-SPI accessibility tree walker
```

## Why it was replaced

The cage + screencopy approach was fundamentally limited: it required an external compositor process, crop math for window bounds, and had no clean path to input injection. `veil-host` replaces it with a purpose-built nested Smithay compositor that owns the surface directly.
