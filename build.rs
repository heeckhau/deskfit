fn main() {
    // Embed Info.plist so macOS finds the Bluetooth usage description in the
    // bare executable.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = format!("{}/macos/Info.plist", env!("CARGO_MANIFEST_DIR"));
        println!("cargo:rerun-if-changed={plist}");
        println!("cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
}
