fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Windows-specific extras
        use winres::WindowsResource;
        use winapi::um::winnt;

        WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set_language(winnt::MAKELANGID(winnt::LANG_ENGLISH, winnt::SUBLANG_ENGLISH_US))
            .compile()?;
    }

    slint_build::compile("ui/appwindow.slint")?;
    
    Ok(())
}
