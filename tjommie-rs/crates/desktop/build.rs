//! The app's main thread gets a 64 MiB stack instead of Windows' 1 MiB (address space reserved, not memory used). A
//! backstop: Markdown nested past what prose uses is shown as plain text before anything parses it (M4 review C1), and
//! this keeps any other deep layout from ending the app with a stack overflow, which no guard can catch.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:67108864");
    }
}
