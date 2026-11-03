//! The client's own texts (dialogs, loading screen, offline page, Discord button) in the game's 10 languages.
//! texts.json was exported from the Electron client's text.ts, so both say the same thing.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Texts {
    strings: HashMap<String, HashMap<String, String>>,
    tips: HashMap<String, Vec<String>>,
}

static TEXTS: LazyLock<Texts> =
    LazyLock::new(|| serde_json::from_str(include_str!("texts.json")).expect("texts.json is valid"));

const LANGS: [&str; 10] = ["es", "en", "pt", "fr", "ja", "zh", "zht", "ko", "ru", "de"];

/// Same rule as the game's detectLang: Taiwan, Hong Kong, Macao and Hant read Traditional Chinese.
pub fn lang_of(tag: &str) -> String {
    let l = tag.to_lowercase().replace('_', "-");
    let base = l.split('-').next().unwrap_or("");
    if base == "zh" {
        let trad = l.split('-').skip(1).any(|p| matches!(p, "tw" | "hk" | "mo" | "hant"));
        return if trad { "zht" } else { "zh" }.into();
    }
    if LANGS.contains(&base) { base.into() } else { "en".into() }
}

pub fn system_lang() -> String {
    lang_of(&sys_locale::get_locale().unwrap_or_default())
}

pub fn tr(lang: &str, key: &str) -> String {
    let get = |l: &str| TEXTS.strings.get(l).and_then(|m| m.get(key)).cloned();
    get(lang).or_else(|| get("en")).unwrap_or_else(|| key.to_string())
}

pub fn tips(lang: &str) -> Vec<String> {
    TEXTS.tips.get(lang).or_else(|| TEXTS.tips.get("en")).cloned().unwrap_or_default()
}

/// Menu labels (macOS), which the Electron client left to Electron's own roles.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn menu(lang: &str, key: &str) -> &'static str {
    let fr = lang == "fr";
    let es = lang == "es";
    match key {
        "edit" if fr => "Édition",
        "edit" if es => "Edición",
        "edit" => "Edit",
        "view" if fr => "Présentation",
        "view" if es => "Ver",
        "view" => "View",
        "window" if fr => "Fenêtre",
        "window" if es => "Ventana",
        "window" => "Window",
        "reload" if fr => "Recharger",
        "reload" if es => "Recargar",
        "reload" => "Reload",
        "fullscreen" if fr => "Plein écran",
        "fullscreen" if es => "Pantalla completa",
        "fullscreen" => "Toggle Full Screen",
        "devtools" if fr => "Outils de développement",
        "devtools" if es => "Herramientas de desarrollo",
        "devtools" => "Developer Tools",
        _ => "",
    }
}
