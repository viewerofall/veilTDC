# Veil Roadmap: V2.0 → V4.0 & Beyond

**Current Status:** V2.0 shipped. Dwindle tiling, IPC daemon, Lua keybinds, DRM/KMS output working. Multi-compositor nesting (Niri + Hyprland) verified.

**Known Issue:** Sway crashes on spawn (nested). Niri + Hyprland work. Debug post-shipping.

---

## V2.0 ✅ SHIPPED

**What's Done:**
- ✅ Dwindle layout (4-window max, 2×2 grid)
- ✅ DRM/KMS framebuffer output (bare TTY)
- ✅ libinput input handling
- ✅ libseat session management
- ✅ IPC daemon + socket (`veil append`)
- ✅ Lua keybinds (Super key only)
- ✅ Multi-compositor nesting (Niri, Hyprland working)
- ✅ Keyboard + mouse input dispatch
- ✅ Window focus + click-to-focus

**Not Included (Deferred):**
- ❌ Damage tracking (full redraw, acceptable for v2)
- ❌ GPU dmabuf import (SHM fallback only)
- ❌ XWayland X11 rendering
- ❌ Terminal shell integration

**Blocker:** Sway nesting crashes. Investigate post-v3.

---

## Post-V2.0 Break: Abyss Login Manager (1–2 weeks, ~8–10h)

**Goal:** Simple TTY login manager. Built during post-V2.0 break before V3.0 work.

**Features:**
- ✅ PAM authentication (username/password)
- ✅ Session selection (Veil, fallback shell, etc.)
- ✅ TTY-native rendering (no X11 needed)
- ✅ Minimal dependencies (libpam only)
- ✅ Clean TTY-based UI (text, arrows, clean)

**Tech Stack:**
- Rust (veil-login crate)
- libpam (PAM integration)
- Crossterm (TTY I/O)

**Result:** Lightweight login gate for VoidOS.

**Note:** Renamed to "Abyss" post-V3.0 when rebranding to Void ecosystem.

---

## V2.5: Optimization Pass (2–3 weeks, Optional)

**Goal:** Performance + stability for embedded.

**Features:**
- ✅ Damage tracking (reduce full redraws)
- ✅ GPU dmabuf import (via GBM/EGL)
- ✅ XWayland X11 rendering
- ✅ Terminal fallback mode (SSH compatibility)
- ✅ Sway crash fix

**Effort:** 15–20h
**Shipping:** Optional. Can skip to V3 if stable enough.

---

## V3.0: Terminal Compositor (6–8 weeks)

**Goal:** Revolutionary UI: Floating windows + interactive shell + TUI apps, all in one framebuffer.

**Architecture:**
```
Veil V3.0
  ├─ Floating windows (not tiled, moveable, resizable)
  ├─ Shell region (bottom 5 rows, interactive $ prompt)
  ├─ TUI apps (vim, htop, lazygit in shell)
  ├─ Font rendering (fontdue)
  ├─ Multi-PTY (Super+T spawns new shell window)
  └─ Scrollback buffer + history
```

**Phases:**

### Phase 1: Font Rendering (1 week, 4–5h)
- Add `fontdue = "0.4"` dependency
- Create `veil-host/src/font.rs`
- Render text glyphs to framebuffer
- Cache glyph atlas

### Phase 2: Shell PTY Integration (1–2 weeks, 5–6h)
- Create `veil-host/src/shell.rs`
- PTY spawning + I/O handling
- Shell buffer + cursor management
- Read shell output in event loop

### Phase 3: Shell Region Rendering (3–4 days, 2–3h)
- Designate bottom N rows for shell
- Render shell text + cursor
- Clear/update shell region each frame

### Phase 4: Input Dispatch (3–4 days, 2h)
- Focus tracking (shell vs window)
- Route keyboard input appropriately
- Click in shell region → focus shell

### Phase 5: Floating Layout (1 week, 4–5h)
- Replace dwindle with floating layout
- Mouse drag window moves
- Mouse drag corner resizes
- Window titlebars

### Phase 6: Multi-PTY Support (3–4 days, 3–4h)
- Super+T → spawn new shell window
- Each shell independent PTY + buffer
- Focus switching between shells

### Phase 7: Scrollback + History (2–3 days, 2–3h)
- Circular buffer (10k lines max)
- Scroll wheel support
- Old output accessible

**Effort Total:** 30–35h
**Timeline:** 6–8 weeks

**Result:** "Terminal Compositor" paradigm proven.

---

## V3.1: UI Polish (2–3 weeks)

**Goal:** Make Veil feel premium, not utilitarian.

**Features:**
- ✅ App launcher (Super+Space → fuzzy search)
- ✅ Sidebar (right edge, clock/stats/user menu)
- ✅ Theme engine (colorschemes: Nord, Dracula, Catppuccin)
- ✅ Hot config reload (SIGHUP)
- ✅ Custom fonts

**Effort:** 10–12h

---

## V4.0: Node Management Suite (3–4 weeks)

**Goal:** Web panel + CLI for cluster orchestration.

**Components:**
- `veil-manager` (new Rust crate)
  - Web panel (dashboard, terminal, alerts)
  - CLI tool (`veilmng` binary)
  - Backend API (SSH key sync, metrics, alerts)

**Features:**
- Web dashboard (node status, CPU/RAM/disk)
- xterm.js SSH terminal in browser
- Email alerts (node down, threshold breach)
- CLI commands: `veilmng list`, `veilmng ssh`, `veilmng exec`
- Metrics polling (systemd-logind compatible)

**Effort:** 25–30h
**Timeline:** 3–4 weeks

**Tech Stack:**
- Backend: Actix-web
- Frontend: HTMX + Tailwind (lightweight)
- Terminal: xterm.js
- DB: SQLite

---

## V4.1+: Post-Shipping (Future)

**Stretch goals (if market traction):**

### V4.1: Network Display (8–12h)
- SSH app forwarding
- VNC backend
- Wayland protocol forwarding

### V4.2: Advanced Layouts (8–10h)
- Stacking (Windows 95 style)
- Fullscreen (one app, Alt+Tab)
- Tabbed (apps as tabs)
- Hot-swap between layouts

### V4.3: Recording + Streaming (6–8h)
- MP4/WebM export
- RTMP streaming
- GIF/screenshot regions

### V4.4: Accessibility (10–15h)
- Screen reader support (AT-SPI)
- High contrast themes
- Keyboard-only navigation

---

## Post-V3.0: Ecosystem Rebrand → VOID

**Timeline:** When V3.0 ships (Jan 2027)

**Rename & Package:**

```
Void Suite Components:
  ├─ VoidWM (the compositor)
  │  └─ Formerly "Veil" (V2.0 shipped, now rebranded)
  │
  ├─ Abyss (login manager)
  │  └─ Formerly "VeilLogin" (built post-V2.0)
  │  └─ TTY-native PAM auth + session selection
  │
  └─ VoidManager (cluster orchestration)
     └─ V4.0 addition (web panel + CLI + alerts)

Void Suite = VoidWM + Abyss + VoidManager (integrated, working together)

VoidOS (distribution):
  └─ Artix fork + Void philosophy + Fedora UX
  ├─ Init freedom (runit/OpenRC/s6 choice)
  ├─ Minimal + functional (Void pkgmgr)
  ├─ Modern defaults (Fedora approach)
  ├─ Pre-installed: VoidWM, Abyss, VoidManager
  └─ Target: "Fedora's ease + Artix's freedom"

The Big Void (commercial):
  └─ VoidOS distribution + support + consulting
  └─ Target market: Homelabbers, DevOps, embedded
```

**Effort:** Rebranding only (no code changes), ~20h for docs/marketing

---

## Timeline Summary

| Release | Timeline | Goal |
|---------|----------|------|
| **V2.0** | ✅ Shipped | Tiling WM on bare TTY |
| **Abyss** | 1–2 weeks | Login manager (break project) |
| **V2.5** | 2–3 weeks | GPU + X11 + perf (optional) |
| **V3.0** | 6–8 weeks | Terminal Compositor |
| **V3.1** | 2–3 weeks | UI polish (launcher, sidebar) |
| **V4.0** | 3–4 weeks | Cluster management |
| **Rebrand** | Concurrent | → Void Suite (Abyss joins Void) |
| **VoidOS** | Post-V3 | Artix-based distro (4–6 weeks) |
| **Total** | ~6 months | Full ecosystem shipped |

---

## Known Issues & Deferred

- **Sway crashes on spawn:** Debug after V3 ships
- **Damage tracking:** Acceptable without it for V3, add in V3.1
- **dmabuf:** SHM fallback works, real impl in V2.5
- **XWayland:** Optional, add in V2.5 if time

---

## Success Metrics by Release

### V2.0 ✅
- ✅ Dwindle layout works
- ✅ Multi-compositor nesting (Niri + Hyprland)
- ✅ IPC daemon stable
- ✅ 60 FPS on Pi Zero 2W

### V3.0
- ✅ Shell PTY interactive and responsive
- ✅ Floating windows with mouse control
- ✅ Multi-PTY spawning (Super+T)
- ✅ "Terminal Compositor" paradigm works
- ✅ Usable for real development

### V3.1
- ✅ Launcher + sidebar functional
- ✅ Themes switchable
- ✅ Feels polished, not utilitarian

### V4.0
- ✅ Web panel operational
- ✅ SSH terminal works
- ✅ Alert system functional
- ✅ Cluster management viable

### Post-Ship
- ✅ Void branding understood by users
- ✅ VoidOS distro installable
- ✅ Market traction (real users, feedback)

---

## Git Strategy

```
main: V2.0 (shipped, stable)
  ├─ v2.5-wip (performance, GPU, X11)
  ├─ v3.0-wip (terminal compositor)
  │  ├─ font-rendering branch
  │  ├─ shell-pty branch
  │  └─ floating-layout branch
  └─ v3.1-wip (polish)
```

**Tag each release:**
- `v2.0.0` (shipped)
- `v2.5.0` (if shipping)
- `v3.0.0` (terminal compositor release)
- `v3.1.0` (UI polish)

---

## Next Steps (Right Now)

1. **Debug Sway crash** (before V3 work)
   - Why does Sway hang but Niri/Hyprland work?
   - Likely: Input dispatch or focus routing
   - Defer to post-V3 unless critical

2. **Start V3.0 prep** (if Sway can wait)
   - Create `veil-host/src/font.rs` scaffold
   - Start `shell.rs` PTY handling
   - Lock Lua config for V3 keybinds

3. **Document current state**
   - V2.0 feature list
   - Known limitations
   - Architecture diagram

---

## Philosophy

**Veil/Void principles (always):**
- ✅ Lightweight (no bloat)
- ✅ Do everything (full-featured, not toy)
- ✅ Runs everywhere (TTY-native, hardware-agnostic)
- ✅ Modular (each piece independent)
- ✅ Open source (forever)

**Don't violate these for shipping speed.**

---

