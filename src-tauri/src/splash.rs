//! The loading screen: a second webview laid over the game's, in the same window, from the first frame
//! until the game's main menu is up. It reads the game's own boot DOM to fill its bar.

use crate::{config, eval_json, texts, Shared};
use rand::seq::IndexedRandom;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, Manager, WebviewUrl, Window};

const ART: [&str; 5] = ["forest", "coast", "glacier", "desert", "horde"];

pub struct Splash {
    pub label: String,
    pub p: f64,
    pub lang: String,
    pub since: Instant,
}

/// Reads the game's boot DOM (the `.loading` block of `#ui`); returns a JSON string.
const WATCH_JS: &str = r#"(() => { const ui = document.getElementById('ui');
  const ld = ui && ui.querySelector(':scope > .loading');
  if (!ld) return JSON.stringify({ screen: !!(ui && ui.querySelector(':scope > .screen')) });
  const bar = ld.querySelector('.bar'), fill = bar && bar.querySelector('i'), label = bar && bar.querySelector('b');
  const m = fill && /scaleX\(([\d.]+)\)/.exec(fill.style.transform);
  return JSON.stringify({ p: m ? Number(m[1]) : 0, label: label ? label.textContent : '', asks: !bar || !!ld.querySelector('button, .gl-help, .panel') }); })()"#;

pub fn show(app: &AppHandle, window: &Window, lang: String) {
    let sh = app.state::<Shared>();
    if sh.st.lock().unwrap().splash.is_some() {
        return;
    }
    let last = config::read_cfg(app).last_art;
    let pool: Vec<&str> = ART.iter().copied().filter(|a| Some(*a) != last.as_deref()).collect();
    let img = pool.choose(&mut rand::rng()).copied().unwrap_or(ART[0]).to_string();
    config::update_cfg(app, |c| c.last_art = Some(img.clone()));

    let tips = serde_json::to_string(&texts::tips(&lang)).unwrap_or_else(|_| "[]".into());
    let url = crate::local_url(
        "splash.html",
        &[
            ("lang", lang.clone()),
            ("img", img),
            ("tips", tips),
            ("tip", texts::tr(&lang, "tip")),
            ("label", texts::tr(&lang, "connecting")),
            ("ver", app.package_info().version.to_string()),
        ],
    );
    let label = format!("splash{}", sh.splash_n.fetch_add(1, Ordering::SeqCst));
    let builder = WebviewBuilder::new(label.clone(), WebviewUrl::CustomProtocol(url))
        .auto_resize()
        .background_color(crate::BG)
        .devtools(sh.devtools)
        .focused(false)
        .on_page_load(|wv, p| {
            if p.event() == PageLoadEvent::Finished {
                crate::show_main(wv.app_handle());
            }
        });
    let size = window
        .inner_size()
        .and_then(|s| Ok(s.to_logical::<f64>(window.scale_factor()?)))
        .unwrap_or(tauri::LogicalSize::new(1280.0, 720.0));
    if window.add_child(builder, LogicalPosition::new(0.0, 0.0), size).is_ok() {
        sh.st.lock().unwrap().splash = Some(Splash { label, p: 0.0, lang, since: Instant::now() });
    }
}

pub fn set(app: &AppHandle, p: f64, label: Option<String>) {
    let sh = app.state::<Shared>();
    let name = {
        let mut st = sh.st.lock().unwrap();
        match st.splash.as_mut() {
            Some(s) if p >= s.p => {
                s.p = p;
                s.label.clone()
            }
            _ => return,
        }
    };
    if let Some(wv) = app.get_webview(&name) {
        let text = serde_json::to_string(&label.unwrap_or_default()).unwrap_or_else(|_| "\"\"".into());
        let _ = wv.eval(format!("window.pb && pb.set({p:.3}, {text})"));
    }
}

pub fn hide(app: &AppHandle, fade: bool) {
    let sh = app.state::<Shared>();
    let taken = sh.st.lock().unwrap().splash.take();
    let Some(s) = taken else { return };
    let label = s.label;
    let close = {
        let (app, label) = (app.clone(), label.clone());
        move || {
            if let Some(wv) = app.get_webview(&label) {
                let _ = wv.close();
            }
            if let Some(g) = app.get_webview("game") {
                let _ = g.set_focus();
            }
        }
    };
    if fade {
        if let Some(wv) = app.get_webview(&label) {
            let _ = wv.eval("window.pb && pb.out()");
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(480));
            close();
        });
    } else {
        close();
    }
}

/// Follows the game's boot until its first screen (or a question to the player) is up, then fades the splash out.
pub fn watch_boot(app: &AppHandle) {
    let sh = app.state::<Shared>();
    let gen = sh.boot_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        let sh = app.state::<Shared>();
        if sh.boot_gen.load(Ordering::SeqCst) != gen {
            return;
        }
        let (lang, since) = match sh.st.lock().unwrap().splash.as_ref() {
            Some(s) => (s.lang.clone(), s.since),
            None => return,
        };
        let Some(game) = app.get_webview("game") else { return };
        if !game.url().is_ok_and(|u| crate::same_site(&sh.game, &u)) {
            return;
        }
        let v = eval_json(&game, WATCH_JS, Duration::from_secs(1)).unwrap_or(Value::Null);
        if v.get("screen").and_then(Value::as_bool) == Some(true) {
            set(&app, 1.0, Some(texts::tr(&lang, "starting")));
            std::thread::sleep(Duration::from_millis(350));
            hide(&app, true);
            return;
        }
        if v.get("asks").and_then(Value::as_bool) == Some(true) {
            hide(&app, true);
            return;
        }
        if let Some(p) = v.get("p").and_then(Value::as_f64) {
            let label = v.get("label").and_then(Value::as_str).filter(|l| !l.is_empty()).map(String::from);
            set(&app, 0.2 + 0.72 * p, label);
        }
        if since.elapsed() > Duration::from_secs(90) {
            hide(&app, true);
            return;
        }
    });
}
