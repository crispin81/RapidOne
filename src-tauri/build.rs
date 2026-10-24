fn main() {
    // Linux: ONNX Runtime's WebGPU build needs libwebgpu_dawn.so, copied beside the program
    // (ort's copy-dylibs); look for it there.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    tauri_build::build()
}
