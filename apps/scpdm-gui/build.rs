//! For Windows: the .exe's icon and version information (Explorer, taskbar, installer), also when
//! built on Linux for Windows (MinGW's windres; the target is read at run time, as `cfg(windows)`
//! here would mean the machine building).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/solidcraft-pdm.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/solidcraft-pdm.ico")
            .set("ProductName", "SolidCraft PDM")
            .set("FileDescription", "SolidCraft PDM: manage SolidCraft designs")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            panic!("Windows resources (icon, version info): {e}");
        }
    }
}
