fn main() {
    if cfg!(target_os = "windows") {
        use winres::WindowsResource;
        use winapi::um::winnt;

        WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set_language(winnt::MAKELANGID(winnt::LANG_ENGLISH, winnt::SUBLANG_ENGLISH_US))
            .compile()
            .unwrap();
    }

    slint_build::compile("ui/logic.slint").unwrap();
    slint_build::compile("ui/elements.slint").unwrap();
    slint_build::compile("ui/appwindow.slint").unwrap();
}
