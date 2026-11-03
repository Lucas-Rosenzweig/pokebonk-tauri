//! PokéBonk desktop client: a Tauri window around https://pokebonk.com, with the same bridge
//! (`window.pokebonkDesktop`) as the Electron client it replaces.

mod config;
mod discord;
mod display;
mod splash;
mod texts;

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{NewWindowResponse, PageLoadEvent, PageLoadPayload, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, LogicalPosition, Manager, RunEvent, Url, Webview, WebviewUrl, Window,
    WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use texts::tr;

/// The bridge API level of the Electron client we mirror; the game compares it.
const BRIDGE_VERSION: &str = "1.4.0";
const SITE: &str = "https://pokebonk.com";
const BG: tauri::window::Color = tauri::window::Color(0x1a, 0x16, 0x30, 0xff);
const APP_ID_DOMAIN: &str = "io.github.lucas-rosenzweig.pokebonk-tauri";
const COMMANDS: [&str; 9] = [
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

// ---- shared state ----

pub struct Shared {
    pub game: Url,
    pub devtools: bool,
    pub launched_at: i64,
    /// `--selftest=<dir>`: load the game, write a report there and quit; touches no saved state.
    pub selftest: Option<std::path::PathBuf>,
    /// How long the self-test waits before reporting, in ms.
    pub selftest_wait: u64,
    pub presence: discord::Presence,
    pub quitting: AtomicBool,
    /// A leave check or dialog is in progress: new requests are ignored.
    pub leaving: AtomicBool,
    /// Generation of the boot watcher thread.
    pub boot_gen: AtomicU64,
    /// Debounce of window-state saves.
    pub save_gen: AtomicU64,
    /// Unique splash webview labels.
    pub splash_n: AtomicU64,
    pub st: Mutex<St>,
}

#[derive(Default)]
pub struct St {
    /// The game's language (the system's until the page says).
    pub lang: String,
    pub splash: Option<splash::Splash>,
    /// Last fullscreen state sent to the page.
    pub fullscreen: bool,
    pub awake: Option<keepawake::KeepAwake>,
    pub shown: bool,
    /// Applied once, in show_main.
    pub restore: Option<config::WinState>,
}

// ---- args ----

fn arg(name: &str) -> Option<String> {
    let prefix = format!("--{name}=");
    std::env::args().find_map(|a| a.strip_prefix(&prefix).map(String::from))
}

fn flag(name: &str) -> bool {
    let f = format!("--{name}");
    std::env::args().any(|a| a == f)
}

fn game_url() -> Url {
    let raw = arg("url").or_else(|| std::env::var("POKEBONK_URL").ok()).unwrap_or_else(|| SITE.into());
    Url::parse(raw.trim_end_matches('/')).unwrap_or_else(|_| Url::parse(SITE).unwrap())
}

// ---- URL helpers ----

fn base_host(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// The game's domain, its www and its subdomains.
pub fn same_site(game: &Url, u: &Url) -> bool {
    let host = game.host_str().unwrap_or("");
    let base = base_host(host);
    let uh = u.host_str().unwrap_or("");
    (u.scheme() == game.scheme() && (uh == host || uh.ends_with(&format!(".{base}")))) || u.origin() == game.origin()
}

pub fn is_local(u: &Url) -> bool {
    match u.scheme() {
        "tauri" => u.host_str() == Some("localhost"),
        "http" | "https" => u.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

pub fn local_url(path: &str, query: &[(&str, String)]) -> Url {
    let base = if cfg!(windows) { "http://tauri.localhost/" } else { "tauri://localhost/" };
    let mut u = Url::parse(base).unwrap().join(path).unwrap();
    u.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
    u
}

fn open_outside(app: &AppHandle, u: &Url) {
    if matches!(u.scheme(), "http" | "https" | "mailto") {
        let _ = app.opener().open_url(u.as_str(), None::<&str>);
    }
}

fn main_window(app: &AppHandle) -> Option<Window> {
    app.get_window("main")
}

fn game_webview(app: &AppHandle) -> Option<Webview> {
    app.get_webview("game")
}

/// Runs JS in the webview and waits for its result. BLOCKING: never call it on the main thread.
/// WebKit returns results JSON-encoded, so expressions returning objects must `JSON.stringify` them.
pub fn eval_json(wv: &Webview, js: &str, timeout: Duration) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    wv.eval_with_callback(js, move |r: String| {
        let _ = tx.send(r);
    })
    .ok()?;
    let v: Value = serde_json::from_str(&rx.recv_timeout(timeout).ok()?).ok()?;
    if let Value::String(s) = &v {
        if let Ok(inner) = serde_json::from_str::<Value>(s) {
            return Some(inner);
        }
    }
    Some(v)
}

// ---- run ----

pub fn run() {
    let game = game_url();
    let app_id = std::env::var("POKEBONK_DISCORD_ID").unwrap_or_else(|_| discord::DEFAULT_APP_ID.into());
    let shared = Shared {
        game,
        devtools: cfg!(debug_assertions) || flag("devtools"),
        launched_at: discord::now_ms(),
        selftest: arg("selftest").map(Into::into),
        selftest_wait: arg("selftest-wait").and_then(|v| v.parse().ok()).unwrap_or(15_000),
        presence: discord::Presence::new(app_id),
        quitting: AtomicBool::new(false),
        leaving: AtomicBool::new(false),
        boot_gen: AtomicU64::new(0),
        save_gen: AtomicU64::new(0),
        splash_n: AtomicU64::new(0),
        st: Mutex::new(St { lang: texts::system_lang(), ..Default::default() }),
    };

    let mut builder = tauri::Builder::default();
    if shared.selftest.is_none() {
        // Must be the first plugin.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| focus_main(app)));
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            pb_display,
            pb_set_display,
            pb_fullscreen,
            pb_relaunch,
            pb_quit,
            pb_retry,
            pb_presence,
            pb_key,
            pb_open
        ])
        .on_menu_event(|app, e| match e.id().as_ref() {
            "pb-quit" => start_leave(app, Leave::Close),
            "pb-reload" => start_leave(app, Leave::Reload),
            "pb-fullscreen" => toggle_fullscreen(app),
            "pb-devtools" => {
                if app.state::<Shared>().devtools {
                    if let Some(wv) = game_webview(app) {
                        wv.open_devtools();
                    }
                }
            }
            _ => {}
        })
        .setup(|app| {
            let handle = app.handle();
            #[cfg(target_os = "macos")]
            app.set_menu(build_menu(handle)?)?;
            add_custom_capability(handle)?;
            create_main(handle)?;
            start_presence(handle);
            if handle.state::<Shared>().selftest.is_some() {
                start_selftest(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the PokéBonk client")
        .run(|app, ev| match ev {
            RunEvent::ExitRequested { api, .. } if !app.state::<Shared>().quitting.load(Ordering::SeqCst) => {
                api.prevent_exit();
                start_leave(app, Leave::Close);
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => focus_main(app),
            _ => {}
        });
}

/// With `--url=http://localhost:5173` the game is not on pokebonk.com: give that origin the bridge too.
fn add_custom_capability(app: &AppHandle) -> tauri::Result<()> {
    let sh = app.state::<Shared>();
    let host = sh.game.host_str().unwrap_or("");
    if host == "pokebonk.com" || host.ends_with(".pokebonk.com") {
        return Ok(());
    }
    let mut cap = tauri::ipc::CapabilityBuilder::new("game-custom-url")
        .remote(format!("{}/*", sh.game.origin().ascii_serialization()))
        .webview("game");
    for c in COMMANDS {
        cap = cap.permission(format!("allow-{}", c.replace('_', "-")));
    }
    app.add_capability(cap)
}

// ---- Discord presence ----

fn start_presence(app: &AppHandle) {
    let sh = app.state::<Shared>();
    if sh.selftest.is_some() {
        return;
    }
    let cfg = config::read_cfg(app);
    sh.presence.set(Some(discord_activity(&base_activity(&sh, &texts::system_lang()))));
    sh.presence.enable(cfg.discord);
    if std::env::var_os("POKEBONK_DISCORD_ID").is_none() {
        let app = app.clone();
        std::thread::spawn(move || {
            if let Some(id) = discord::remote_app_id() {
                app.state::<Shared>().presence.use_app(id);
            }
        });
    }
}

#[derive(Default)]
struct Act {
    details: Option<String>,
    state: Option<String>,
    large_image: Option<String>,
    large_text: Option<String>,
    small_image: Option<String>,
    small_text: Option<String>,
    start: Option<i64>,
    buttons: Vec<(String, String)>,
}

fn base_activity(sh: &Shared, lang: &str) -> Act {
    Act {
        start: Some(sh.launched_at),
        large_image: Some(format!("{SITE}/assets/ui/desktop/app.png")),
        large_text: Some("PokéBonk".into()),
        buttons: vec![(tr(lang, "rpcPlay"), SITE.into())],
        ..Default::default()
    }
}

fn clean(v: &Value) -> Option<String> {
    let s = v.as_str()?.trim();
    (s.chars().count() >= 2).then(|| s.chars().take(128).collect())
}

fn img(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    let u = Url::parse(s).ok()?;
    let host = u.host_str()?;
    (u.scheme() == "https" && (host == "pokebonk.com" || host.ends_with(".pokebonk.com"))).then(|| s.to_string())
}

fn discord_activity(a: &Act) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("type".into(), json!(0));
    if let Some(d) = &a.details {
        o.insert("details".into(), json!(d));
    }
    if let Some(s) = &a.state {
        o.insert("state".into(), json!(s));
    }
    if let Some(t) = a.start {
        o.insert("timestamps".into(), json!({ "start": t }));
    }
    let mut assets = serde_json::Map::new();
    for (k, v) in [
        ("large_image", &a.large_image),
        ("large_text", &a.large_text),
        ("small_image", &a.small_image),
        ("small_text", &a.small_text),
    ] {
        if let Some(v) = v {
            assets.insert(k.into(), json!(v));
        }
    }
    if !assets.is_empty() {
        o.insert("assets".into(), Value::Object(assets));
    }
    let buttons: Vec<Value> = a
        .buttons
        .iter()
        .filter(|(l, u)| !l.is_empty() && !u.is_empty())
        .take(2)
        .map(|(l, u)| json!({ "label": l, "url": u }))
        .collect();
    if !buttons.is_empty() {
        o.insert("buttons".into(), Value::Array(buttons));
    }
    Value::Object(o)
}

// ---- macOS menu ----

#[cfg(target_os = "macos")]
fn build_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
    let l = texts::system_lang();
    let sh = app.state::<Shared>();
    let sep = || PredefinedMenuItem::separator(app);

    let app_menu = Submenu::with_items(
        app,
        "PokéBonk",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &sep()?,
            &PredefinedMenuItem::services(app, None)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &sep()?,
            // Custom, not the predefined quit: it must go through the leave check.
            &MenuItem::with_id(app, "pb-quit", format!("{} PokéBonk", tr(&l, "quit")), true, Some("Cmd+Q"))?,
        ],
    )?;
    // Needed for ⌘C / ⌘V in the login fields.
    let edit = Submenu::with_items(
        app,
        texts::menu(&l, "edit"),
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let reload = MenuItem::with_id(app, "pb-reload", texts::menu(&l, "reload"), true, Some("Cmd+R"))?;
    let full = MenuItem::with_id(app, "pb-fullscreen", texts::menu(&l, "fullscreen"), true, Some("Ctrl+Cmd+F"))?;
    let view = Submenu::new(app, texts::menu(&l, "view"), true)?;
    view.append(&reload)?;
    view.append(&full)?;
    if sh.devtools {
        view.append(&sep()?)?;
        view.append(&MenuItem::with_id(app, "pb-devtools", texts::menu(&l, "devtools"), true, Some("Alt+Cmd+I"))?)?;
    }
    let window = Submenu::with_items(
        app,
        texts::menu(&l, "window"),
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &sep()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &window])
}

// ---- window and game webview ----

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "linux"
    }
}

fn bridge_script(game: &Url) -> String {
    let host = game.host_str().unwrap_or("");
    let cfg = json!({
        "version": BRIDGE_VERSION,
        "platform": platform(),
        "protocol": format!("{}:", game.scheme()),
        "host": host,
        "base": base_host(host),
        "origin": game.origin().ascii_serialization(),
        "mac": cfg!(target_os = "macos"),
    });
    include_str!("bridge.js").replace("__PB_CONFIG__", &cfg.to_string())
}

/// Safari's version, so the page sees the browser it expects; the client's name is added at the end.
#[cfg(target_os = "macos")]
fn mac_user_agent() -> String {
    let v = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist")
        .ok()
        .and_then(|t| {
            let rest = &t[t.find("<key>CFBundleShortVersionString</key>")?..];
            let rest = &rest[rest.find("<string>")? + 8..];
            Some(rest[..rest.find("</string>")?].trim().to_string())
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "26.0".into());
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{v} Safari/605.1.15 PokeBonkDesktop/{BRIDGE_VERSION}"
    )
}

/// The menu music must autoplay: Tauri does not expose wry's autoplay option.
#[cfg(target_os = "macos")]
fn wk_config() -> objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration> {
    use objc2_web_kit::{WKAudiovisualMediaTypes, WKWebViewConfiguration};
    let mtm = objc2::MainThreadMarker::new().expect("the webview is created on the main thread");
    let c = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { c.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None) };
    c
}

fn game_builder(app: &AppHandle) -> WebviewBuilder<tauri::Wry> {
    let sh = app.state::<Shared>();
    let (nav_app, win_app) = (app.clone(), app.clone());
    let b = WebviewBuilder::new("game", WebviewUrl::External("about:blank".parse().unwrap()))
        .auto_resize()
        .initialization_script(bridge_script(&sh.game))
        .background_color(BG)
        // The co-op host must not be throttled in the background.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        .devtools(sh.devtools)
        .accept_first_mouse(true)
        .zoom_hotkeys_enabled(false)
        // On WebKit this is called for iframes too (captcha): only odd schemes are handled here.
        .on_navigation(move |u| {
            if matches!(u.scheme(), "http" | "https" | "about" | "data" | "blob" | "tauri")
                || u.host_str() == Some("tauri.localhost")
            {
                return true;
            }
            open_outside(&nav_app, u);
            false
        })
        // Every popup opens in the system browser.
        .on_new_window(move |u, _features| {
            open_outside(&win_app, &u);
            NewWindowResponse::Deny
        })
        .on_page_load(on_game_load);
    #[cfg(target_os = "macos")]
    let b = b.user_agent(&mac_user_agent()).with_webview_configuration(wk_config());
    b
}

fn first_launch(app: &AppHandle) -> config::WinState {
    let mut s = config::WinState { fullscreen: true, ..Default::default() };
    match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let k = m.scale_factor();
            let size: tauri::LogicalSize<f64> = m.work_area().size.to_logical(k);
            s.width = Some((size.width * 0.85).round().min(1600.0));
            s.height = Some((size.height * 0.85).round().min(900.0));
        }
        None => {
            s.width = Some(1280.0);
            s.height = Some(720.0);
        }
    }
    s
}

/// Is a window at (x, y) reachable on some monitor?
fn on_screen(app: &AppHandle, x: f64, y: f64) -> bool {
    app.available_monitors().unwrap_or_default().iter().any(|m| {
        let k = m.scale_factor();
        let p: LogicalPosition<f64> = m.work_area().position.to_logical(k);
        let s: tauri::LogicalSize<f64> = m.work_area().size.to_logical(k);
        x + 100.0 > p.x && x < p.x + s.width - 100.0 && y >= p.y - 10.0 && y < p.y + s.height - 100.0
    })
}

fn create_main(app: &AppHandle) -> tauri::Result<()> {
    let state = config::read_win(app).unwrap_or_else(|| first_launch(app));
    let (w, h) = (state.width.unwrap_or(1280.0), state.height.unwrap_or(720.0));
    let mut wb = WindowBuilder::new(app, "main")
        .title("PokéBonk")
        .inner_size(w.max(960.0), h.max(540.0))
        .min_inner_size(960.0, 540.0)
        .visible(false)
        .background_color(BG);
    wb = match (state.x, state.y) {
        (Some(x), Some(y)) if on_screen(app, x, y) => wb.position(x, y),
        _ => wb.center(),
    };
    let window = wb.build()?;
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    window.add_child(game_builder(app), LogicalPosition::new(0.0, 0.0), size)?;

    let sh = app.state::<Shared>();
    sh.st.lock().unwrap().restore = Some(state);
    splash::show(app, &window, texts::system_lang());

    // If the splash never finishes loading, show the window anyway.
    let fallback = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        show_main(&fallback);
    });

    let ev_app = app.clone();
    window.on_window_event(move |ev| match ev {
        WindowEvent::CloseRequested { api, .. } => {
            if !ev_app.state::<Shared>().quitting.load(Ordering::SeqCst) {
                api.prevent_close();
                start_leave(&ev_app, Leave::Close);
            }
        }
        WindowEvent::Resized(_) => {
            sync_fullscreen(&ev_app, false);
            schedule_save(&ev_app);
            keep_awake(&ev_app);
        }
        WindowEvent::Moved(_) => schedule_save(&ev_app),
        WindowEvent::Focused(_) => {
            keep_awake(&ev_app);
            read_lang(&ev_app);
        }
        _ => {}
    });

    load_game(app);
    Ok(())
}

/// Shows the window (once) and applies the saved state. Fullscreen goes after show: on macOS the native
/// fullscreen of a hidden window misbehaves.
pub fn show_main(app: &AppHandle) {
    let sh = app.state::<Shared>();
    let restore = {
        let mut st = sh.st.lock().unwrap();
        if st.shown {
            return;
        }
        st.shown = true;
        st.restore.take()
    };
    let Some(w) = main_window(app) else { return };
    let _ = w.show();
    if let Some(s) = restore.filter(|_| sh.selftest.is_none()) {
        if s.maximized {
            let _ = w.maximize();
        }
        if s.fullscreen {
            set_fullscreen(app, &w, true);
        }
    }
    let _ = w.set_focus();
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ---- fullscreen, window state, keep-awake ----

pub fn set_fullscreen(app: &AppHandle, w: &Window, on: bool) {
    if !on {
        if let Some(wv) = game_webview(app) {
            let _ = wv.eval("document.fullscreenElement && document.exitFullscreen().catch(() => {})");
        }
    }
    let _ = w.set_fullscreen(on);
}

fn toggle_fullscreen(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let on = w.is_fullscreen().unwrap_or(false);
        set_fullscreen(app, &w, !on);
    }
}

/// Tells the page when the window's fullscreen state changed (or always, with `force`).
fn sync_fullscreen(app: &AppHandle, force: bool) {
    let Some(w) = main_window(app) else { return };
    let fs = w.is_fullscreen().unwrap_or(false);
    {
        let sh = app.state::<Shared>();
        let mut st = sh.st.lock().unwrap();
        if !force && st.fullscreen == fs {
            return;
        }
        st.fullscreen = fs;
    }
    if let Some(wv) = game_webview(app) {
        let _ = wv.eval(format!("window.__pb && window.__pb.fs({fs})"));
    }
}

fn schedule_save(app: &AppHandle) {
    let gen = app.state::<Shared>().save_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        if app.state::<Shared>().save_gen.load(Ordering::SeqCst) == gen {
            save_state(&app);
        }
    });
}

pub fn save_state(app: &AppHandle) {
    if app.state::<Shared>().selftest.is_some() {
        return;
    }
    let Some(w) = main_window(app) else { return };
    if w.is_minimized().unwrap_or(false) {
        return;
    }
    let fs = w.is_fullscreen().unwrap_or(false);
    let max = w.is_maximized().unwrap_or(false);
    let mut s = config::read_win(app).unwrap_or_else(|| first_launch(app));
    if !fs && !max {
        if let (Ok(k), Ok(p), Ok(size)) = (w.scale_factor(), w.outer_position(), w.inner_size()) {
            let p: LogicalPosition<f64> = p.to_logical(k);
            let size: tauri::LogicalSize<f64> = size.to_logical(k);
            s.x = Some(p.x);
            s.y = Some(p.y);
            s.width = Some(size.width);
            s.height = Some(size.height);
        }
    }
    s.fullscreen = fs;
    s.maximized = max;
    config::write_win(app, &s);
}

/// Players with a gamepad: the screen must not sleep while the window is up and focused.
fn keep_awake(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    let want = w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false) && w.is_visible().unwrap_or(false);
    let sh = app.state::<Shared>();
    let mut st = sh.st.lock().unwrap();
    if want && st.awake.is_none() {
        st.awake = keepawake::Builder::default()
            .display(true)
            .reason("Playing PokéBonk")
            .app_name("PokéBonk")
            .app_reverse_domain(APP_ID_DOMAIN)
            .create()
            .ok();
    } else if !want {
        st.awake = None;
    }
}

/// Follows the language the player picked in the game, for the client's own texts.
fn read_lang(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(wv) = game_webview(&app) else { return };
        let sh = app.state::<Shared>();
        if !wv.url().is_ok_and(|u| same_site(&sh.game, &u)) {
            return;
        }
        if let Some(Value::String(s)) = eval_json(&wv, "document.documentElement.lang", Duration::from_secs(1)) {
            if !s.trim().is_empty() {
                sh.st.lock().unwrap().lang = texts::lang_of(&s);
            }
        }
    });
}

// ---- page load ----

fn on_game_load(wv: Webview, payload: PageLoadPayload<'_>) {
    let app = wv.app_handle().clone();
    let sh = app.state::<Shared>();
    let url = payload.url();
    match payload.event() {
        PageLoadEvent::Started => {
            if same_site(&sh.game, url) {
                let (has_splash, lang) = {
                    let mut st = sh.st.lock().unwrap();
                    if let Some(s) = st.splash.as_mut() {
                        s.p = 0.0;
                    }
                    (st.splash.is_some(), st.lang.clone())
                };
                if has_splash {
                    splash::set(&app, 0.04, Some(tr(&lang, "connecting")));
                } else if let Some(w) = main_window(&app) {
                    splash::show(&app, &w, lang);
                }
            } else if matches!(url.scheme(), "http" | "https") && !is_local(url) {
                // A top-level jump to another site.
                open_outside(&app, url);
                let _ = wv.navigate(sh.game.clone());
            }
        }
        PageLoadEvent::Finished => {
            if same_site(&sh.game, url) {
                let lang = sh.st.lock().unwrap().lang.clone();
                splash::set(&app, 0.15, Some(tr(&lang, "loadingGame")));
                splash::watch_boot(&app);
                sync_fullscreen(&app, true);
                read_lang(&app);
            } else if is_local(url) {
                sync_fullscreen(&app, true);
            }
        }
    }
}

// ---- loading and offline ----

fn load_game(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let sh = app.state::<Shared>();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
            .into();
        // Any HTTP answer means online; only network errors mean offline.
        let online = agent.head(sh.game.as_str()).call().is_ok();
        if online {
            if let Some(wv) = game_webview(&app) {
                let _ = wv.navigate(sh.game.clone());
            }
        } else {
            show_main(&app);
            splash::hide(&app, false);
            show_offline(&app, "off");
        }
    });
}

fn show_offline(app: &AppHandle, why: &str) {
    let sh = app.state::<Shared>();
    let l = sh.st.lock().unwrap().lang.clone();
    let url = local_url(
        "offline.html",
        &[
            ("title", tr(&l, "offTitle")),
            ("body", tr(&l, if why == "crash" { "crashed" } else { "offBody" })),
            ("retry", tr(&l, "retry")),
            ("url", sh.game.to_string()),
            ("lang", l.clone()),
        ],
    );
    if let Some(wv) = game_webview(app) {
        let _ = wv.navigate(url);
    }
}

// ---- leaving ----

#[derive(Clone, Copy)]
enum Leave {
    Close,
    Reload,
    Relaunch,
}

/// The Electron client asked before leaving a run: so do we (close, ⌘Q, reload, relaunch).
fn start_leave(app: &AppHandle, what: Leave) {
    let sh = app.state::<Shared>();
    if sh.quitting.load(Ordering::SeqCst) {
        perform(app, what);
        return;
    }
    if sh.leaving.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let sh = app.state::<Shared>();
        let asks = game_webview(&app)
            .filter(|wv| wv.url().is_ok_and(|u| same_site(&sh.game, &u)))
            .and_then(|wv| {
                eval_json(
                    &wv,
                    "(() => { try { return !!(window.__pb && window.__pb.askLeave()); } catch (e) { return false; } })()",
                    Duration::from_secs(1),
                )
            })
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !asks {
            sh.leaving.store(false, Ordering::SeqCst);
            perform(&app, what);
            return;
        }
        let l = sh.st.lock().unwrap().lang.clone();
        let done = app.clone();
        // "Keep playing" is the first (default) button, so Enter never quits.
        app.dialog()
            .message(tr(&l, "quitRun"))
            .title(tr(&l, "quitTitle"))
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(tr(&l, "keep"), tr(&l, "quit")))
            .show(move |keep| {
                done.state::<Shared>().leaving.store(false, Ordering::SeqCst);
                if !keep {
                    perform(&done, what);
                }
            });
    });
}

fn perform(app: &AppHandle, what: Leave) {
    let sh = app.state::<Shared>();
    match what {
        Leave::Close => {
            sh.quitting.store(true, Ordering::SeqCst);
            save_state(app);
            app.exit(0);
        }
        Leave::Relaunch => {
            sh.quitting.store(true, Ordering::SeqCst);
            save_state(app);
            app.restart();
        }
        Leave::Reload => match game_webview(app) {
            Some(wv) if wv.url().is_ok_and(|u| is_local(&u)) => load_game(app),
            Some(wv) => {
                let _ = wv.reload();
            }
            None => {}
        },
    }
}

// ---- self-test ----

const REPORT_JS: &str = r#"(() => {
  let webgl = null;
  try {
    const gl = document.createElement('canvas').getContext('webgl2');
    if (gl) { const x = gl.getExtension('WEBGL_debug_renderer_info'); webgl = x ? gl.getParameter(x.UNMASKED_RENDERER_WEBGL) : 'webgl2'; }
  } catch (e) { webgl = 'error: ' + e; }
  const pd = typeof pokebonkDesktop === 'object' ? pokebonkDesktop : null;
  const ui = document.getElementById('ui');
  return JSON.stringify({
    href: location.href, title: document.title, ua: navigator.userAgent, lang: document.documentElement.lang,
    desktop: pd && { keys: Object.keys(pd), version: pd.version, platform: pd.platform, fullscreen: pd.fullscreen() },
    ui: !ui ? 'none' : ui.querySelector(':scope > .loading') ? 'loading' : ui.querySelector(':scope > .screen') ? 'screen' : 'other',
    askLeave: !!(window.__pb && window.__pb.askLeave()),
    ipc: window.__pbTest,
    screen: innerWidth + 'x' + innerHeight + '@' + devicePixelRatio,
    webgl,
  });
})()"#;

/// `--selftest`: waits for the game, checks the bridge's IPC from the page, writes `<dir>/report.json`, quits.
fn start_selftest(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let sh = app.state::<Shared>();
        let wait = Duration::from_millis(sh.selftest_wait);
        let t0 = Instant::now();
        let mut splash_gone_ms = None;
        let mut probed = false;
        while t0.elapsed() < wait {
            if splash_gone_ms.is_none() && sh.st.lock().unwrap().splash.is_none() {
                splash_gone_ms = Some(t0.elapsed().as_millis());
            }
            if !probed && t0.elapsed() + Duration::from_millis(1500) >= wait {
                probed = true;
                if let Some(wv) = game_webview(&app) {
                    let _ = wv.eval(
                        "window.__pbTest = null; (window.pokebonkDesktop ? window.pokebonkDesktop.display() : Promise.reject('no bridge')).then(d => { window.__pbTest = { ok: true, d }; }, e => { window.__pbTest = { ok: false, e: String(e) }; });",
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let wv = game_webview(&app);
        let page = wv.as_ref().and_then(|wv| eval_json(wv, REPORT_JS, Duration::from_secs(3)));
        let win = main_window(&app);
        let report = json!({
            "splashGoneMs": splash_gone_ms,
            "version": app.package_info().version.to_string(),
            "bridge": BRIDGE_VERSION,
            "url": wv.as_ref().and_then(|wv| wv.url().ok()).map(|u| u.to_string()),
            "fullscreen": win.as_ref().and_then(|w| w.is_fullscreen().ok()),
            "display": win.as_ref().map_or(Value::Null, |w| display::state(&app, w)),
            "page": page,
        });
        if let Some(dir) = &sh.selftest {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap_or_default());
        }
        sh.quitting.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

// ---- commands (the bridge) ----

#[tauri::command]
fn pb_display(app: AppHandle) -> Value {
    main_window(&app).map_or(Value::Null, |w| display::state(&app, &w))
}

#[tauri::command]
async fn pb_set_display(app: AppHandle, p: Value) -> Value {
    match main_window(&app) {
        Some(w) => display::set(&app, &w, p).await,
        None => Value::Null,
    }
}

#[tauri::command]
fn pb_fullscreen(app: AppHandle) -> bool {
    main_window(&app).is_some_and(|w| w.is_fullscreen().unwrap_or(false))
}

#[tauri::command]
fn pb_relaunch(app: AppHandle) {
    start_leave(&app, Leave::Relaunch);
}

#[tauri::command]
fn pb_quit(app: AppHandle) {
    start_leave(&app, Leave::Close);
}

#[tauri::command]
fn pb_retry(app: AppHandle) {
    load_game(&app);
}

#[tauri::command]
fn pb_presence(app: AppHandle, webview: Webview, p: Option<Value>) {
    let sh = app.state::<Shared>();
    if webview.label() != "game" || !webview.url().is_ok_and(|u| same_site(&sh.game, &u)) {
        return;
    }
    let lang = sh.st.lock().unwrap().lang.clone();
    let mut a = base_activity(&sh, &lang);
    if let Some(Value::Object(o)) = &p {
        a.details = o.get("details").and_then(clean);
        a.state = o.get("state").and_then(clean);
        if let Some(t) = o.get("start").and_then(Value::as_f64) {
            if t > 1.6e12 && t <= (discord::now_ms() + 60_000) as f64 {
                a.start = Some(t.round() as i64);
            }
        }
        if let Some(i) = o.get("largeImage").and_then(img) {
            a.large_image = Some(i);
        }
        if let Some(t) = o.get("largeText").and_then(clean) {
            a.large_text = Some(t);
        }
        a.small_image = o.get("smallImage").and_then(img);
        a.small_text = a.small_image.as_ref().and_then(|_| o.get("smallText").and_then(clean));
    }
    sh.presence.set(Some(discord_activity(&a)));
}

#[tauri::command]
fn pb_key(app: AppHandle, action: String) {
    match action.as_str() {
        "fullscreen" => toggle_fullscreen(&app),
        "reload" => start_leave(&app, Leave::Reload),
        "devtools" => {
            if app.state::<Shared>().devtools {
                if let Some(wv) = game_webview(&app) {
                    wv.open_devtools();
                }
            }
        }
        _ => {}
    }
}

#[tauri::command]
fn pb_open(app: AppHandle, url: String) {
    if let Ok(u) = Url::parse(&url) {
        if !same_site(&app.state::<Shared>().game, &u) {
            open_outside(&app, &u);
        }
    }
}
