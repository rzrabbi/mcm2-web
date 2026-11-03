//! Puts the OpenMCM2 tyre icon on the Windows .exe (Explorer, taskbar).
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/openmcm2.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/openmcm2.ico");
        res.compile().expect("Windows icon resource");
    }
}
