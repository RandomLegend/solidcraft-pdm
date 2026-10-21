//! On Windows: the .exe's icon and version information (Explorer, taskbar, installer).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/solidcraft-pdm.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/solidcraft-pdm.ico")
            .set("ProductName", "SolidCraft PDM")
            .set("FileDescription", "SolidCraft PDM: manage SolidCraft designs")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        res.compile().expect("Windows resources (icon, version info)");
    }
}
