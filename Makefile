PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin
LIBDIR   = $(PREFIX)/lib/veil

CARGO   ?= cargo
ZIG     ?= zig

# ── Targets ───────────────────────────────────────────────────────────────────

.PHONY: all release debug install install-user uninstall clean

all: debug

# Development build — unoptimised, fast to compile
debug:
	$(CARGO) build
	$(MAKE) -C veil-screencopy
	cd veil-capture && $(ZIG) build
	@cp -f libveil_capture.so target/debug/ 2>/dev/null || true
	@cp -f veil-screencopy/veil-screencopy target/debug/ 2>/dev/null || true
	@echo "[veil] debug build done → target/debug/{veil,veil-screencopy,libveil_capture.so}"

# Release build — full optimisation
release:
	$(CARGO) build --release
	$(MAKE) -C veil-screencopy
	cd veil-capture && $(ZIG) build -Doptimize=ReleaseFast
	cp -f libveil_capture.so target/release/libveil_capture.so
	cp -f veil-screencopy/veil-screencopy target/release/veil-screencopy
	@echo "[veil] release build done → target/release/{veil,veil-screencopy,libveil_capture.so}"

# Install to system (default: /usr/local)
# Build first as your user: make release
# Then install as root:     sudo make install
install:
	@test -f target/release/veil || (echo "[veil] run 'make release' first (as your user, not root)"; exit 1)
	@test -f veil-screencopy/veil-screencopy || (echo "[veil] run 'make release' first"; exit 1)
	@test -f libveil_capture.so || (echo "[veil] run 'make release' first"; exit 1)
	install -Dm755 target/release/veil                   $(DESTDIR)$(BINDIR)/veil
	install -Dm755 veil-screencopy/veil-screencopy        $(DESTDIR)$(BINDIR)/veil-screencopy
	install -Dm755 libveil_capture.so                     $(DESTDIR)$(LIBDIR)/libveil_capture.so
	@echo "[veil] installed to $(DESTDIR)$(PREFIX)"
	@echo "         bin: $(DESTDIR)$(BINDIR)/{veil,veil-screencopy}"
	@echo "         lib: $(DESTDIR)$(LIBDIR)/libveil_capture.so"

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
	$(MAKE) -C veil-screencopy clean
	cd veil-capture && rm -rf zig-out .zig-cache
	rm -f libveil_capture.so
	@echo "[veil] cleaned"
