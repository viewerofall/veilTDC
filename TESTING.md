# Veil Debugging & Testing Guide

## Build & Install

```bash
make release
sudo make install  # or make install-user for ~/.local
```

## Test with a GUI App

```bash
veil run-gui firefox
# or
veil run-gui gnome-calculator
```

Watch stderr for diagnostic output:

```bash
veil run-gui firefox 2>&1 | tee /tmp/veil-debug.log
```

## Diagnostic Output to Check

### 1. SHM (LD_PRELOAD) Capture

Look for:
```
[capture] checking for SHM at /dev/shm/veil_<PID>
[capture] SHM file exists, attempting open...
[capture] SHM (LD_PRELOAD) active
[capture] SHM frame: <width>x<height>
```

**If SHM works**: Window content is captured directly (no cropping needed)

**If SHM fails**: Falls back to screencopy with cropping
```
[capture] SHM file not found, falling back to screencopy
```

### 2. Screencopy + Crop

If SHM isn't available, look for:
```
[wayland_capture] connected (output scale=<N>)
[capture] full output: <width>x<height>, window: <w>x<h> @ <x>,<y>
[capture] cropped: <cw>x<ch> (scale=<scale>)
```

**Issue**: If cropped dimensions match full output, cropping isn't working.

## Input Injection Testing

### Required Tools

Install at least one of:
```bash
# Portal method (best for Wayland)
sudo pacman -S xdg-desktop-portal xdg-desktop-portal-gnome

# XWayland fallback
sudo pacman -S xdotool

# Wayland-native fallback  
sudo pacman -S ydotool
```

### How It Works

Each keypress/click triggers (in order):
1. **xdg-desktop-portal RemoteDesktop** (via dbus-send)
2. **xdotool** (XWayland if available)
3. **ydotool** (Wayland-native)

The first one that succeeds is used. If all fail, input is silently dropped.

### Debugging Input

Add to `/tmp/test-portal.sh`:
```bash
#!/bin/bash
# Test portal directly
dbus-send --session --print-reply \
  /org/freedesktop/portal/desktop \
  org.freedesktop.portal.RemoteDesktop.InjectKeyboardKeysym \
  uint32:65  uint32:1  # 'a' key press
```

Or test xdotool:
```bash
xdotool type "hello"  # Should type in focused window
```

## Common Issues

### "Full desktop renders instead of window"

**Cause**: SHM file not created by LD_PRELOAD, screencopy capturing full output, crop math wrong

**Fix**:
1. Check `/dev/shm/veil_*` exists: `ls -la /dev/shm/veil_*`
2. Check window bounds are detected: Look for `[capture] window move/resize` logs
3. Check crop is being applied: Look for `[capture] cropped: ` dimensions

### "Input doesn't reach app"

**Cause**: Portal not responding, xdotool/ydotool not available, window not focused

**Fixes**:
1. Check portal is available: `systemctl --user status xdg-desktop-portal`
2. Check tools: `which xdotool ydotool`
3. Click app window manually before typing (ensures focus)

### "Screencopy fails entirely"

**Cause**: Compositor doesn't support `zwlr_screencopy_manager_v1`

**Check**:
```bash
wayland-info | grep screencopy
```

## Architecture

- **Capture Priority 1**: SHM via LD_PRELOAD (libveil_capture.so LD_PRELOADed on app)
  - Intercepts EGL/GL renders
  - Writes RGBA frames to `/dev/shm/veil_<PID>`
  - Fastest, window-specific content
  
- **Capture Priority 2**: wlr-screencopy (compositor protocol)
  - Captures full output
  - Cropped to window bounds via pure-Rust crop_rgba()
  - Fallback if LD_PRELOAD fails

- **Input Injection**:
  - Tries xdg-desktop-portal RemoteDesktop (dbus-send)
  - Falls back to xdotool (XWayland)
  - Falls back to ydotool (Wayland-native)
  - Spawns non-blocking threads, render loop continues

## Key Files

- `veil-compositor/src/gui.rs`: Window bounds detection, SHM check, screencopy + crop
- `veil-cli/src/input.rs`: Input thread, event conversion, portal/xdotool calls
- `veil-cli/src/main.rs`: Render loop with input event processing
