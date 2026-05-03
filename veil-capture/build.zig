const std = @import("std");

pub fn build(b: *std.Build) void {
    const target   = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const root_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target           = target,
        .optimize         = optimize,
    });

    const lib = b.addLibrary(.{
        .name        = "veil_capture",
        .linkage     = .dynamic,
        .root_module = root_mod,
    });
    lib.linkLibC();
    lib.linkSystemLibrary("wayland-client");
    lib.linkSystemLibrary("EGL");
    lib.linkSystemLibrary("GL");
    lib.linkSystemLibrary("rt");

    b.installArtifact(lib);

    // Copy to project root for easy LD_PRELOAD reference
    const copy = b.addInstallFile(lib.getEmittedBin(), "../../libveil_capture.so");
    b.getInstallStep().dependOn(&copy.step);
}
