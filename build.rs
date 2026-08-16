fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");

    #[cfg(windows)]
    {
        // Nothing fancy: the icon is embedded at runtime instead of via a .rc file,
        // so no extra build tooling is required on Windows.
    }
}
