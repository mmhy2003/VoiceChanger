//! Embeds the Windows resources: the app icon (`assets/icon.ico`) and the version resource,
//! FileVersion/ProductVersion `X.Y.Z.0` from the workspace version.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let icon = std::path::Path::new(&manifest).join("../../assets/icon.ico");
    assert!(
        icon.is_file(),
        "{} is missing; run `cargo xtask gen-icon`",
        icon.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by cargo");
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    let numeric = (major << 48) | (minor << 32) | (patch << 16);
    let text = format!("{major}.{minor}.{patch}.0");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path is UTF-8"))
        .set("ProductName", "VoiceChanger")
        .set("FileDescription", "VoiceChanger")
        .set("CompanyName", "VoiceChanger contributors")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 VoiceChanger contributors",
        )
        .set("OriginalFilename", "voicechanger.exe")
        .set("FileVersion", &text)
        .set("ProductVersion", &text)
        .set_version_info(winresource::VersionInfo::FILEVERSION, numeric)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    res.compile()
        .expect("embedding the Windows version resource");
}
