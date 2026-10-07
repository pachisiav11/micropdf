fn main() {
    // Debug builds keep element ids and types so the UI tests can query the window
    // (tests/ui.rs); release builds leave them out.
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_debug_info(debug);
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI failed to compile");
}
