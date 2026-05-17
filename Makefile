PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin

CARGO   ?= cargo

BINS = veil veil-host

.PHONY: all release debug install install-user uninstall clean

all: debug

# Development build — both binaries
debug:
	$(CARGO) build -p veil-cli -p veil-host
	@echo "[veil] debug build done → target/debug/{veil,veil-host}"

# Release build — both binaries, full optimisation
release:
	$(CARGO) build --release -p veil-cli -p veil-host
	@echo "[veil] release build done → target/release/{veil,veil-host}"

# Install to system (default: /usr/local). Build as your user first:
#   make release && sudo make install
install:
	@test -f target/release/veil      || (echo "[veil] run 'make release' first (as your user, not root)"; exit 1)
	@test -f target/release/veil-host || (echo "[veil] run 'make release' first (as your user, not root)"; exit 1)
	install -Dm755 target/release/veil      $(DESTDIR)$(BINDIR)/veil
	install -Dm755 target/release/veil-host $(DESTDIR)$(BINDIR)/veil-host
	@echo "[veil] installed to $(DESTDIR)$(PREFIX)"
	@echo "         $(DESTDIR)$(BINDIR)/veil"
	@echo "         $(DESTDIR)$(BINDIR)/veil-host"

# Install to ~/.local (no sudo needed)
install-user: release
	$(MAKE) install PREFIX=$(HOME)/.local

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/veil
	rm -f $(DESTDIR)$(BINDIR)/veil-host
	@echo "[veil] uninstalled from $(DESTDIR)$(PREFIX)"

clean:
	$(CARGO) clean
	@echo "[veil] cleaned"
