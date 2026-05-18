PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin

CARGO   ?= cargo

.PHONY: all release debug install install-user uninstall clean

all: debug

debug:
	$(CARGO) build -p veil-host
	@echo "[veil] debug build → target/debug/veil-host"

release:
	$(CARGO) build --release -p veil-host
	@echo "[veil] release build → target/release/veil-host"

# Build as your user first, then: sudo make install
install:
	@test -f target/release/veil-host || (echo "[veil] run 'make release' first (as your user, not root)"; exit 1)
	install -Dm755 target/release/veil-host $(DESTDIR)$(BINDIR)/veil-host
	@echo "[veil] installed → $(DESTDIR)$(BINDIR)/veil-host"

install-user: release
	$(MAKE) install PREFIX=$(HOME)/.local

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/veil-host
	@echo "[veil] uninstalled from $(DESTDIR)$(BINDIR)"

clean:
	$(CARGO) clean
	@echo "[veil] cleaned"
