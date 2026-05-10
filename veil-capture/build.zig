const std = @import("std");

pub fn build(b: *std.Build) void {
    _ = b;
    // For now: `zig build-lib src/main_minimal.zig -lc -lwayland-client -lEGL -dynamic`
    // TODO: wire up proper build.zig
}
