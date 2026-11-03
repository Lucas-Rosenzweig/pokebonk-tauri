//! Ajustes → Pantalla: what the game's display settings read (`pokebonkDesktop.display()`) and change
//! (`setDisplay`). Same shape as the Electron client's DisplayState, so the game's screen works unchanged.

use crate::{config, Shared};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, PhysicalSize, Window};

/// Sizes in real pixels (what players know: 1920×1080); the window works in scaled ones.
const SIZES: [(u32, u32); 6] = [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3200, 1800), (3840, 2160)];

fn same(a: &Monitor, b: &Monitor) -> bool {
    a.name() == b.name() && a.position() == b.position()
}

fn monitors(w: &Window) -> Vec<Monitor> {
    let mut v = w.available_monitors().unwrap_or_default();
    v.sort_by_key(|m| (m.position().x, m.position().y));
    v
}

fn current(w: &Window) -> Option<Monitor> {
    w.current_monitor().ok().flatten().or_else(|| w.primary_monitor().ok().flatten())
}

/// The work area of a monitor in logical units.
fn work_area(m: &Monitor) -> (f64, f64, f64, f64) {
    let k = m.scale_factor();
    let a = m.work_area();
    let p: LogicalPosition<f64> = a.position.to_logical(k);
    let s: LogicalSize<f64> = a.size.to_logical(k);
    (p.x, p.y, s.width, s.height)
}

pub fn state(app: &AppHandle, w: &Window) -> Value {
    let cfg = config::read_cfg(app);
    let list = monitors(w);
    let Some(on) = current(w) else { return Value::Null };
    let primary = w.primary_monitor().ok().flatten();
    let k = on.scale_factor();
    let (_, _, wa_w, wa_h) = work_area(&on);
    let inner = w.inner_size().unwrap_or(PhysicalSize::new(0, 0));
    let (cw, ch) = (inner.width as i64, inner.height as i64);
    let sizes: Vec<String> = SIZES
        .iter()
        .filter(|(sw, sh)| *sw as f64 / k <= wa_w && *sh as f64 / k <= wa_h - 40.0)
        .map(|(sw, sh)| format!("{sw}x{sh}"))
        .collect();
    let preset = SIZES
        .iter()
        .find(|(sw, sh)| sizes.contains(&format!("{sw}x{sh}")) && (*sw as i64 - cw).abs() <= 2 && (*sh as i64 - ch).abs() <= 2)
        .map(|(sw, sh)| format!("{sw}x{sh}"));
    let fullscreen = w.is_fullscreen().unwrap_or(false);
    let maximized = w.is_maximized().unwrap_or(false);
    json!({
        "mode": if fullscreen { "fullscreen" } else { "window" },
        "size": if maximized { "max".to_string() } else { preset.unwrap_or_else(|| "custom".into()) },
        "sizes": sizes,
        "monitor": list.iter().position(|m| same(m, &on)).unwrap_or(0),
        "monitors": list.iter().map(|m| json!({
            "label": format!("{}×{}", m.size().width, m.size().height),
            "primary": primary.as_ref().is_some_and(|p| same(p, m)),
        })).collect::<Vec<_>>(),
        "vsync": cfg.vsync,
        "vsyncNext": cfg.vsync,
        "angle": "auto",
        "angleNext": "auto",
        "angles": ["auto"],
        "discord": cfg.discord,
    })
}

/// Waits until the window is (or isn't) fullscreen and its size has stopped moving (macOS animates it), at most ~2.5 s.
async fn settle(w: &Window, on: bool) {
    let t0 = Instant::now();
    while w.is_fullscreen().unwrap_or(on) != on && t0.elapsed() < Duration::from_millis(1500) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut last = w.inner_size().ok();
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = w.inner_size().ok();
        if now == last {
            break;
        }
        last = now;
    }
}

async fn fullscreen(app: &AppHandle, w: &Window, on: bool) {
    if w.is_fullscreen().unwrap_or(false) == on {
        return;
    }
    crate::set_fullscreen(app, w, on);
    settle(w, on).await;
}

pub async fn set(app: &AppHandle, w: &Window, p: Value) -> Value {
    let sh = app.state::<Shared>();
    if let Some(on) = p.get("discord").and_then(Value::as_bool) {
        config::update_cfg(app, |c| c.discord = on);
        sh.presence.enable(on);
    }
    if let Some(on) = p.get("vsync").and_then(Value::as_bool) {
        config::update_cfg(app, |c| c.vsync = on);
    }
    if let Some(i) = p.get("monitor").and_then(Value::as_u64) {
        let list = monitors(w);
        if let (Some(d), Some(on)) = (list.get(i as usize), current(w)) {
            if !same(d, &on) {
                let was_fs = w.is_fullscreen().unwrap_or(false);
                fullscreen(app, w, false).await;
                if w.is_maximized().unwrap_or(false) {
                    let _ = w.unmaximize();
                }
                let (ax, ay, aw, ah) = work_area(d);
                let size: LogicalSize<f64> = w.outer_size().unwrap_or_default().to_logical(w.scale_factor().unwrap_or(1.0));
                let (ww, hh) = (size.width.min(aw), size.height.min(ah));
                let _ = w.set_position(LogicalPosition::new(ax + (aw - ww) / 2.0, ay + (ah - hh) / 2.0));
                if was_fs {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    fullscreen(app, w, true).await;
                }
            }
        }
    }
    let mode = p.get("mode").and_then(Value::as_str);
    let size = p.get("size").and_then(Value::as_str);
    if mode == Some("fullscreen") {
        fullscreen(app, w, true).await;
    }
    if mode == Some("window") || size.is_some() {
        fullscreen(app, w, false).await;
    }
    match size {
        Some("max") => {
            let _ = w.maximize();
        }
        Some(s) => {
            if let Some((sw, sh)) = s.split_once('x').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?))) {
                if w.is_maximized().unwrap_or(false) {
                    let _ = w.unmaximize();
                }
                let _ = w.set_size(PhysicalSize::new(sw.round() as u32, sh.round() as u32));
                let _ = w.center();
            }
        }
        None => {}
    }
    crate::save_state(app);
    // A beat so a resize has landed before the page reads it back.
    tokio::time::sleep(Duration::from_millis(60)).await;
    state(app, w)
}
