fn main() {
    // English is the source language; lang/ru holds the Russian catalogue.
    println!("cargo:rerun-if-changed=lang");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
        .with_bundled_translations("lang");
    slint_build::compile_with_config("ui/app.slint", config).expect("compile Slint UI");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/images/app.ico");
        println!("cargo:rerun-if-changed=ui/images/app-muted.ico");
        let mut resource = winresource::WindowsResource::new();
        // 1: app and tray icon, 2: tray icon while the microphone is muted.
        resource.set_icon("ui/images/app.ico");
        resource.set_icon_with_id("ui/images/app-muted.ico", "2");
        // Shown in Explorer's file properties and in the Task Manager.
        resource.set("ProductName", "FIFINE Lite");
        resource.set("FileDescription", "FIFINE Lite");
        resource.set("CompanyName", "Holorigg");
        resource.set("LegalCopyright", "© 2026 Holorigg. GPL-3.0-only");
        resource.set("OriginalFilename", "fifine-lite.exe");
        resource.compile().expect("embed Windows icon");
    }
}
