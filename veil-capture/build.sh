#!/bin/bash
set -e
zig build-lib \
  -dynamic \
  -fPIC \
  src/main.zig \
  -lc \
  -lwayland-client \
  -lEGL \
  -lGL \
  -Doptimize=ReleaseFast \
  -femit-bin=libveil_capture.so
echo "Built: libveil_capture.so"
