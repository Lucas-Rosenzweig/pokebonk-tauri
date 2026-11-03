/// The commands the game page may call (src/bridge.js). Listing them here puts them under Tauri's ACL:
/// only the webviews named in capabilities/ can reach them, and only from the origins listed there.
const COMMANDS: &[&str] = &[
    "pb_display",
    "pb_set_display",
    "pb_fullscreen",
    "pb_relaunch",
    "pb_quit",
    "pb_retry",
    "pb_presence",
    "pb_key",
    "pb_open",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
