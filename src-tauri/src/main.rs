#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Fix white screen / EGL_BAD_PARAMETER on headless or no-GPU Linux.
    // Must be set before webkit2gtk initializes (i.e. here, in main()).
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    std::env::set_var("WEBKIT_DISABLE_ACCELERATED_2D_CANVAS", "1");
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("GSK_RENDERER", "cairo");
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }
    slums_launcher_lib::run()
}
