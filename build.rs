fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile Slint UI");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/images/app.ico");
        println!("cargo:rerun-if-changed=ui/images/app-muted.ico");
        let mut resource = winresource::WindowsResource::new();
        // 1: app and tray icon, 2: tray icon while the microphone is muted.
        resource.set_icon("ui/images/app.ico");
        resource.set_icon_with_id("ui/images/app-muted.ico", "2");
        resource.compile().expect("embed Windows icon");
    }
}
