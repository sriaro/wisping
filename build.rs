fn main() {
    slint_build::compile("ui/main.slint").unwrap();

    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico");
    res.set_manifest_file("assets/app.manifest");
    res.set("ProductName", "Wisping");
    res.set("FileDescription", "Wisping");
    res.set("OriginalFilename", "Wisping.exe");
    res.set("FileVersion", "1.0.1.0");
    res.set("ProductVersion", "1.0.1.0");
    res.compile().unwrap();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
