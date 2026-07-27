//! Compiles the Windows resources into the executable: the application icon, and the version
//! information the file's Properties dialog shows.
//!
//! This exists because on Windows an icon is not a file sitting beside the binary the way it is
//! under a desktop entry on Linux -- it is a resource inside the `.exe`, put there at link time. So
//! it has to happen here, which is also what makes `cargo install --path .` produce an installed
//! binary that already carries it, with nothing else to copy or configure.
//!
//! The `.ico` is committed (see `assets/icon/README.md`), so this needs no image tooling.

fn main() {
    // The build script runs on the host, but what matters is what we are building *for*: this way a
    // Windows binary cross-compiled from elsewhere still gets its icon.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    const ICON: &str = "assets/icon/phonoscule.ico";
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON);
    // FileVersion and ProductVersion come from the package version; these are the strings Explorer
    // shows, which otherwise default to the crate name.
    res.set("ProductName", "Phonoscule");
    res.set("FileDescription", "Phonoscule");
    res.set("LegalCopyright", "Copyright (c) 2026 Jojo. Mozilla Public License 2.0.");

    // A missing resource compiler is not worth failing a build over: without this the binary is
    // exactly what it was before, minus the icon. `cargo:warning` says so where it will be seen.
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon and version info: {e}");
    }
}
