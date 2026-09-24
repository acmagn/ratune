fn main() {
    // Embed an Info.plist so the bare `ratune` binary still has a CFBundleIdentifier.
    // Without this, macOS MediaRemote / Control Center often ignores Now Playing updates
    // from a plain CLI process.
    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let plist = std::path::Path::new(&manifest_dir).join("macos/Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
