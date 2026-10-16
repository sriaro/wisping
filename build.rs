fn main() {
    slint_build::compile("ui/main.slint").unwrap();

    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico");
    res.set_manifest_file("assets/app.manifest");
    res.set("ProductName", "Wisp");
    res.set("FileDescription", "Wisp");
    res.set("OriginalFilename", "Wisp.exe");
    res.set("FileVersion", "1.0.0.0");
    res.set("ProductVersion", "1.0.0.0");
    res.compile().unwrap();
}
