fn main() {
    compile_windows();

    slint_build::compile("ui/logic.slint").unwrap();
    slint_build::compile("ui/elements.slint").unwrap();
    slint_build::compile("ui/appwindow.slint").unwrap();
}

#[cfg(target_os = "windows")]
fn compile_windows() {
    use winres::WindowsResource;
        use winapi::um::winnt;

        WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set_language(winnt::MAKELANGID(winnt::LANG_ENGLISH, winnt::SUBLANG_ENGLISH_US))
            .compile()
            .unwrap();
}

#[cfg(not(target_os = "windows"))]
fn compile_windows() {} // No-op on non-Windows platforms
