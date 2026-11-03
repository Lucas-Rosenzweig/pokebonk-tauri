//! What the client remembers between launches, in its own data folder:
//! client.json (options) and window.json (where and how big the window was).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ClientCfg {
    /// Kept for the game's settings screen. WebKit always syncs to the display, so it changes nothing here.
    pub vsync: bool,
    /// Only "auto": WebKit has no choice of graphics backend (Electron had ANGLE's).
    pub angle: String,
    /// «Mostrar en Discord».
    pub discord: bool,
    /// The loading screen's last illustration, so the next launch shows another.
    #[serde(rename = "lastArt", skip_serializing_if = "Option::is_none")]
    pub last_art: Option<String>,
}

impl Default for ClientCfg {
    fn default() -> Self {
        Self { vsync: true, angle: "auto".into(), discord: true, last_art: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WinState {
    /// Logical (scaled) position and inner size of the window when it was last neither fullscreen nor maximized.
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub maximized: bool,
    pub fullscreen: bool,
}

fn file<R: Runtime>(app: &AppHandle<R>, name: &str) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(name))
}

fn read<T: Default + for<'de> Deserialize<'de>, R: Runtime>(app: &AppHandle<R>, name: &str) -> Option<T> {
    let text = std::fs::read_to_string(file(app, name)?).ok()?;
    serde_json::from_str(&text).ok()
}

fn write<T: Serialize, R: Runtime>(app: &AppHandle<R>, name: &str, v: &T) {
    let Some(path) = file(app, name) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside then renamed: a crash mid-write never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(v) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub fn read_cfg<R: Runtime>(app: &AppHandle<R>) -> ClientCfg {
    let mut c: ClientCfg = read(app, "client.json").unwrap_or_default();
    c.angle = "auto".into();
    c
}

pub fn update_cfg<R: Runtime>(app: &AppHandle<R>, f: impl FnOnce(&mut ClientCfg)) -> ClientCfg {
    let mut c = read_cfg(app);
    f(&mut c);
    write(app, "client.json", &c);
    c
}

/// None on the first launch.
pub fn read_win<R: Runtime>(app: &AppHandle<R>) -> Option<WinState> {
    read(app, "window.json")
}

pub fn write_win<R: Runtime>(app: &AppHandle<R>, s: &WinState) {
    write(app, "window.json", s);
}
