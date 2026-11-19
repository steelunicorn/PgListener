//! Встраивает в exe значок приложения и сведения о версии (только Windows).
//!
//! Сам значок окна и трея меняется вместе с темой из кода, а этот ресурс
//! нужен проводнику, панели задач и Alt+Tab: там иконка у файла одна.

#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("icons/dark.ico");
    res.set("FileDescription", "PgListener");
    res.set("ProductName", "PgListener");
    res.set("LegalCopyright", "MIT OR Apache-2.0");
    if let Err(e) = res.compile() {
        // Без rc.exe сборка всё равно возможна: останется значок по умолчанию.
        println!("cargo:warning=не удалось встроить значок exe: {e}");
    }
}

#[cfg(not(windows))]
fn main() {}
