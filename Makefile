PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin
LIBDIR   = $(PREFIX)/lib/veil

CARGO   ?= cargo
ZIG     ?= zig

# ── Targets ───────────────────────────────────────────────────────────────────

.PHONY: all release debug install install-user uninstall clean

all: debug

# Development build — unoptimised, fast to compile (Cargo handles Zig via build.rs)
debug:
	$(CARGO) build
	@echo "[veil] debug build done → target/debug/veil"

# Release build — full optimisation
release:
	$(CARGO) build --release
	@echo "[veil] release build done → target/release/veil"

# Install to system (default: /usr/local)
# Build first as your user: make release
# Then install as root:     sudo make install
install:
	@test -f target/release/veil || (echo "[veil] run 'make release' first (as your user, not root)"; exit 1)
	install -Dm755 target/release/veil $(DESTDIR)$(BINDIR)/veil
	@echo "[veil] installed to $(DESTDIR)$(PREFIX)"
	@echo "         bin: $(DESTDIR)$(BINDIR)/veil"

# Install to ~/.local (no sudo needed, build and install as same user)
install-user: release
	$(MAKE) install PREFIX=$(HOME)/.local

# Remove installed files
uninstall:
	rm -f  $(DESTDIR)$(BINDIR)/veil
	rm -f  $(DESTDIR)$(BINDIR)/veil-screencopy
	rm -f  $(DESTDIR)$(LIBDIR)/libveil_capture.so
	-rmdir $(DESTDIR)$(LIBDIR) 2>/dev/null || true
	@echo "[veil] uninstalled from $(DESTDIR)$(PREFIX)"

clean:
	$(CARGO) clean
	cd veil-capture && rm -rf zig-out .zig-cache
	@echo "[veil] cleaned"
