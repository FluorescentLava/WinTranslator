fn main() {
    #[cfg(feature = "portable")]
    windows_reactor_setup::as_self_contained();
    #[cfg(not(feature = "portable"))]
    {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("app.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rerun-if-changed=app.manifest");
    }
}
