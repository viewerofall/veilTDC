WP := /usr/share/wayland-protocols
GEN := gen

SRCS := screencopy.c \
	$(GEN)/ext-foreign-toplevel-list-v1.c \
	$(GEN)/ext-image-capture-source-v1.c \
	$(GEN)/ext-image-copy-capture-v1.c

CFLAGS := -O2 -Wall -Wextra -I$(GEN) $(shell pkg-config --cflags wayland-client)
LDFLAGS := $(shell pkg-config --libs wayland-client)

.PHONY: all clean

all: veil-screencopy

$(GEN):
	mkdir -p $@

$(GEN)/ext-foreign-toplevel-list-v1.h: $(WP)/staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml | $(GEN)
	wayland-scanner client-header $< $@

$(GEN)/ext-foreign-toplevel-list-v1.c: $(WP)/staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml | $(GEN)
	wayland-scanner private-code $< $@

$(GEN)/ext-image-capture-source-v1.h: $(WP)/staging/ext-image-capture-source/ext-image-capture-source-v1.xml | $(GEN)
	wayland-scanner client-header $< $@

$(GEN)/ext-image-capture-source-v1.c: $(WP)/staging/ext-image-capture-source/ext-image-capture-source-v1.xml | $(GEN)
	wayland-scanner private-code $< $@

$(GEN)/ext-image-copy-capture-v1.h: $(WP)/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml | $(GEN)
	wayland-scanner client-header $< $@

$(GEN)/ext-image-copy-capture-v1.c: $(WP)/staging/ext-image-copy-capture/ext-image-copy-capture-v1.xml | $(GEN)
	wayland-scanner private-code $< $@

$(GEN)/ext-foreign-toplevel-list-v1.c: $(GEN)/ext-foreign-toplevel-list-v1.h
$(GEN)/ext-image-capture-source-v1.c: $(GEN)/ext-image-capture-source-v1.h
$(GEN)/ext-image-copy-capture-v1.c: $(GEN)/ext-image-copy-capture-v1.h
screencopy.c: $(GEN)/ext-foreign-toplevel-list-v1.h $(GEN)/ext-image-capture-source-v1.h $(GEN)/ext-image-copy-capture-v1.h

veil-screencopy: $(SRCS)
	$(CC) $(CFLAGS) -o $@ $^ $(LDFLAGS)

clean:
	rm -rf $(GEN) veil-screencopy
