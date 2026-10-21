fn main() {
    let mut attrs = tauri_build::Attributes::new();
    // Release builds always run elevated: raw writes to a physical disk need admin.
    // Dev builds stay unelevated so `tauri dev` can launch them.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        attrs = attrs.windows_attributes(
            tauri_build::WindowsAttributes::new().app_manifest(include_str!("admin.manifest")),
        );
    }
    tauri_build::try_build(attrs).expect("failed to run build script");
}
