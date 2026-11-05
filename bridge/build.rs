//! Windows: give t3wow.exe a version resource, its icon and an application
//! manifest. An unsigned executable without any of these looks like malware
//! to Defender's machine-learning heuristics.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/t3wow.ico");
    println!("cargo:rerun-if-changed=assets/windows/t3wow.exe.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icon/t3wow.ico")
        .set_manifest_file("assets/windows/t3wow.exe.manifest")
        .set_language(0x0409)
        .set("ProductName", "T3 WoW")
        .set("FileDescription", "T3 WoW bridge")
        .set("CompanyName", "Dzmitry Safarau")
        .set("LegalCopyright", "Copyright (c) 2026 Dzmitry Safarau. MIT License.")
        .set("OriginalFilename", "t3wow.exe")
        .set("InternalName", "t3wow")
        .set(
            "Comments",
            "Drive a remote T3 Code environment from inside World of Warcraft. https://github.com/DSilence/t3wow",
        );
    match resource.compile_for("t3wow") {
        Ok(()) => {}
        // Release builds are MSVC; they must carry the resources.
        Err(error) if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") => {
            panic!("Could not compile the Windows resources: {error}")
        }
        // Cross-checks from macOS or Linux usually have no windres.
        Err(error) => println!("cargo:warning=Skipping the Windows resources: {error}"),
    }
}
