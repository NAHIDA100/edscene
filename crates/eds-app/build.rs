fn main() {
    println!("cargo:rustc-env=EDSCENE_TARGET={}", std::env::var("TARGET").unwrap_or_default());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ui");
    if std::path::Path::new("ui/settings.slint").exists() {
        slint_build::compile("ui/settings.slint").expect("slint compile");
    }
}
