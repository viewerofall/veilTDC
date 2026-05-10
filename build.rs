use std::process::Command;
use std::path::PathBuf;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = PathBuf::from(&out_dir);
    let profile = std::env::var("PROFILE").unwrap();

    // Determine optimization level for Zig
    let zig_opt = match profile.as_str() {
        "release" => "ReleaseFast",
        _ => "Debug",
    };

    // Compile Zig library using zig build-lib
    let zig_status = Command::new("zig")
        .args(&["build-lib"])
        .args(&["-lc", "-lwayland-client", "-lEGL", "-dynamic"])
        .args(&["-O", zig_opt])
        .arg("veil-capture/src/main.zig")
        .arg(format!("-femit-bin={}/libveil_capture.so", out_dir))
        .status()
        .expect("Failed to compile Zig library");

    if !zig_status.success() {
        panic!("Zig compilation failed");
    }

    // Tell Cargo to link against the compiled library
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=dylib=veil_capture");

    // Rebuild if Zig source changes
    println!("cargo:rerun-if-changed=veil-capture/src/main_minimal.zig");
    println!("cargo:rerun-if-changed=veil-capture/src/convert.zig");
    println!("cargo:rerun-if-changed=veil-capture/src/shm_ring.zig");
}
