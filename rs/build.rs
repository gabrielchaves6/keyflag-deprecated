// Embed the app icon (app.rc -> assets/keyflag.ico) into the executable so the taskbar,
// Alt+Tab, the About window and Explorer all show KeyFlag's icon instead of a generic glyph.
//
// Only attempted for the MSVC target: the release CI builds with MSVC (rc.exe is available),
// while local builds use the GNU toolchain on a box without windres — there the embed is
// skipped and the icon falls back to the runtime-drawn one. The shipped installer is the
// MSVC build, so end users get the embedded icon.
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/keyflag.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_resource::compile("app.rc", embed_resource::NONE);
    }
}
