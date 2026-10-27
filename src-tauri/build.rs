fn main() {
    // Linux: ONNX Runtime's WebGPU build needs libwebgpu_dawn.so, copied beside the program
    // (ort's copy-dylibs); look for it there.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    // macOS: ONNX Runtime's CoreML code asks `@available`, which needs ___isPlatformVersionAtLeast.
    // Rust's std has one, but the release build's LTO drops it (only C++ calls it), so the link
    // failed; clang's own runtime supplies it (only what's missing is taken from it).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        if let Ok(out) = std::process::Command::new("clang").arg("--print-runtime-dir").output() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() {
                println!("cargo:rustc-link-search=native={dir}");
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            }
        }
    }
    tauri_build::build()
}
