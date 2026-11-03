//! アドオン(ReaCastを改造したい人向け。作り方は リポジトリの「アドオンの作り方.md」)。
//!
//! exeを書き換える代わりに、設定のフォルダの addons\<フォルダ名>\addon.json に書いておくと、
//! ReaCastが読み込む。exeの外に置くので、ReaCastを更新してもそのまま使える。
//! アドオンでできることは決まっていて、プログラムは動かさない(安全のため):
//! - css: 画面ごとの見た目(OBSの画面・デスクトップに出す窓に /addon.css?page=… で配る)
//! - names: 名前欄を、名前の一覧からランダムに置き換える(同じ人はずっと同じ名前)
//! - tts: 「コマンドで渡す」の「よく使う設定」に、読み上げソフトのボタンを足す
//!   (押した時に欄に入るだけで、勝手には動かさない)
//!
//! どれを使うかは、コメント設定の「アドオン」タブで1つずつON/OFFする(最初はOFF)。
//! 「アドオンを使わない」をONにすると、全部まとめて止める(不具合の切り分け用)。

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::config_store::ConfigStore;

const ENABLED_KEY: &str = "addonsEnabled";
const SAFE_KEY: &str = "addonsSafeMode";
/// アドオンが見た目を変えられる画面(addon.jsonの "css" のキー)
pub const CSS_PAGES: &[&str] = &["subtitle", "comments", "nico", "voice", "audience", "poll", "overlay"];
const MAX_CSS_BYTES: u64 = 512 * 1024;
const MAX_NAMES: usize = 10_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsPreset {
    pub label: String,
    pub command_path: String,
    pub command_args: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonInfo {
    /// フォルダ名
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    /// 必要なReaCastの版(空なら指定なし)
    pub min_reacast: String,
    /// 今のReaCastで使えるか(min_reacastより古いと使えない)
    pub compatible: bool,
    pub enabled: bool,
    /// 見た目を変える画面
    pub css_pages: Vec<String>,
    /// 名前の一覧の数(名前を変えないなら0)
    pub names: usize,
    pub tts: Vec<TtsPreset>,
    /// 読み込めなかった理由(addon.jsonが壊れている等)
    pub error: Option<String>,
}

struct NameRule {
    /// 置き換える名前(空なら全部の名前)
    replace: Vec<String>,
    /// 置き換える取得元("bbs" / "youtube" / "twitch")
    sources: Vec<String>,
    list: Vec<String>,
}

#[derive(Default)]
struct Loaded {
    css: HashMap<String, String>,
    names: Vec<NameRule>,
    tts: Vec<TtsPreset>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

pub fn dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("addons"))
}

/// "1.7.20" を比べられる形に("v"や後ろの文字は無視)
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let mut it = v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

fn app_version() -> &'static str {
    option_env!("APP_VERSION").unwrap_or("dev")
}

fn is_compatible(min: &str) -> bool {
    let (Some(need), Some(have)) = (parse_version(min), parse_version(app_version())) else {
        // 指定なし・開発版は使える
        return true;
    };
    have >= need
}

/// アドオンのフォルダの中のファイルだけを読む(「..」で外に出るのは読まない)
fn inside(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel.trim());
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    Some(base.join(rel))
}

fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_CSS_BYTES {
        return None;
    }
    let s = std::fs::read_to_string(path).ok()?;
    Some(s.trim_start_matches('\u{feff}').to_string())
}

fn str_of(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().trim().to_string()
}

fn str_list(v: Option<&serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::String(s)) => vec![s.trim().to_string()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// 1つのアドオンを読む。返すのは一覧に出す情報と、使う時の中身
fn load_one(base: &Path, id: &str, enabled: bool) -> (AddonInfo, Loaded) {
    let mut info = AddonInfo {
        id: id.to_string(),
        name: id.to_string(),
        author: String::new(),
        version: String::new(),
        description: String::new(),
        min_reacast: String::new(),
        compatible: true,
        enabled,
        css_pages: Vec::new(),
        names: 0,
        tts: Vec::new(),
        error: None,
    };
    let mut loaded = Loaded::default();
    let json = match read_text(&base.join("addon.json")) {
        Some(t) => t,
        None => {
            info.error = Some("addon.json がありません".into());
            return (info, loaded);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            info.error = Some(format!("addon.json の書き方が正しくありません({e})"));
            return (info, loaded);
        }
    };
    let name = str_of(&v, "name");
    if !name.is_empty() {
        info.name = name;
    }
    info.author = str_of(&v, "author");
    info.version = str_of(&v, "version");
    info.description = str_of(&v, "description");
    info.min_reacast = str_of(&v, "minReaCast");
    info.compatible = is_compatible(&info.min_reacast);

    if let Some(css) = v.get("css").and_then(|x| x.as_object()) {
        for (page, file) in css {
            if !CSS_PAGES.contains(&page.as_str()) {
                continue;
            }
            let text = file.as_str().and_then(|f| inside(base, f)).and_then(|p| read_text(&p));
            match text {
                Some(t) => {
                    info.css_pages.push(page.clone());
                    loaded.css.insert(page.clone(), t);
                }
                None => info.error = Some(format!("css の {page} のファイルを読めません")),
            }
        }
    }

    if let Some(n) = v.get("names") {
        let mut list = str_list(n.get("list"));
        if let Some(text) = n.get("file").and_then(|x| x.as_str()).and_then(|f| inside(base, f)).and_then(|p| read_text(&p)) {
            list.extend(text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')));
        }
        list.truncate(MAX_NAMES);
        let mut sources = str_list(n.get("sources"));
        if sources.is_empty() {
            sources.push("bbs".into());
        }
        if !list.is_empty() {
            info.names = list.len();
            loaded.names.push(NameRule { replace: str_list(n.get("replace")), sources, list });
        }
    }

    if let Some(arr) = v.get("tts").and_then(|x| x.as_array()) {
        let dir = base.to_string_lossy().to_string();
        for t in arr {
            let label = str_of(t, "label");
            let path = str_of(t, "commandPath").replace("{addon}", &dir);
            if label.is_empty() || path.is_empty() {
                continue;
            }
            let args = t.get("commandArgs").and_then(|x| x.as_str()).unwrap_or("#Res#").replace("{addon}", &dir);
            info.tts.push(TtsPreset { label, command_path: path, command_args: args });
        }
        loaded.tts = info.tts.clone();
    }
    (info, loaded)
}

fn enabled_ids(store: &ConfigStore) -> Vec<String> {
    store
        .get(ENABLED_KEY)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(String::from))
        .collect()
}

fn safe_mode(store: &ConfigStore) -> bool {
    store.get(SAFE_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// フォルダの中のアドオンを全部読む(名前順)。使うものはLOADEDに入れる
fn scan(app: &AppHandle) -> Vec<AddonInfo> {
    let store = app.state::<ConfigStore>();
    let enabled = enabled_ids(&store);
    let safe = safe_mode(&store);
    let mut out = Vec::new();
    let mut merged = Loaded::default();
    if let Some(root) = dir(app) {
        let _ = std::fs::create_dir_all(&root);
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        dirs.sort();
        for d in dirs {
            let id = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let on = enabled.contains(&id);
            let (info, loaded) = load_one(&d, &id, on);
            if on && !safe && info.compatible && info.error.is_none() {
                for (page, css) in loaded.css {
                    let e = merged.css.entry(page).or_default();
                    e.push_str(&format!("\n/* ---- アドオン: {} ---- */\n{}\n", info.name, css));
                }
                merged.names.extend(loaded.names);
                merged.tts.extend(loaded.tts);
            }
            out.push(info);
        }
    }
    let used = out.iter().filter(|a| a.enabled && a.compatible && a.error.is_none()).map(|a| a.name.as_str()).collect::<Vec<_>>();
    if safe {
        log::info!("アドオン: 使わない設定なので、全部止めています");
    } else if !used.is_empty() {
        log::info!("アドオン: {}", used.join("、"));
    }
    *LOADED.lock().unwrap() = Some(merged);
    out
}

/// 起動時に1回読む
pub fn init(app: &AppHandle) {
    scan(app);
}

/// 読み込み直して、開いている画面に知らせる
fn reload(app: &AppHandle) -> Vec<AddonInfo> {
    let list = scan(app);
    app.state::<crate::obs_bridge::ObsBridge>().push_voice(serde_json::json!({ "type": "addons" }));
    let _ = app.emit("addons:changed", ());
    list
}

/// /addon.css?page=… で配るCSS(使っているアドオンが無ければ空)
pub fn css(page: &str) -> String {
    LOADED.lock().unwrap().as_ref().and_then(|l| l.css.get(page).cloned()).unwrap_or_default()
}

/// 名前が変わらないように、人(掲示板ならID)から決まった番号を作る
fn stable_index(key: &str, len: usize) -> usize {
    // FNV-1a(起動し直しても同じ値になる)
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h % len as u64) as usize
}

/// 名前欄の置き換え(アドオンの names)。person は同じ人を見分けるための文字(掲示板ならID)
pub fn rename(source: &str, name: &str, person: &str) -> Option<String> {
    let guard = LOADED.lock().unwrap();
    let rule = guard.as_ref()?.names.iter().find(|r| {
        r.sources.iter().any(|s| s == source) && (r.replace.is_empty() || r.replace.iter().any(|x| x == name.trim()))
    })?;
    Some(rule.list[stable_index(person, rule.list.len())].clone())
}

#[tauri::command]
pub fn addons_list(app: AppHandle) -> serde_json::Value {
    let list = scan(&app);
    let store = app.state::<ConfigStore>();
    serde_json::json!({
        "addons": list,
        "safeMode": safe_mode(&store),
        "folder": dir(&app).map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
        "appVersion": app_version(),
    })
}

#[tauri::command]
pub fn addons_set_enabled(app: AppHandle, store: tauri::State<ConfigStore>, id: String, on: bool) -> Result<(), String> {
    let mut ids = enabled_ids(&store);
    ids.retain(|x| x != &id);
    if on {
        ids.push(id.clone());
    }
    store.set(ENABLED_KEY.to_string(), serde_json::json!(ids)).map_err(|e| e.to_string())?;
    log::info!("アドオン「{id}」を{}", if on { "ON" } else { "OFF" });
    reload(&app);
    Ok(())
}

#[tauri::command]
pub fn addons_set_safe_mode(app: AppHandle, store: tauri::State<ConfigStore>, on: bool) -> Result<(), String> {
    store.set(SAFE_KEY.to_string(), serde_json::Value::Bool(on)).map_err(|e| e.to_string())?;
    reload(&app);
    Ok(())
}

#[tauri::command]
pub fn addons_reload(app: AppHandle) {
    reload(&app);
}

/// 使っているアドオンの「コマンドで渡す」のボタン
#[tauri::command]
pub fn addons_tts_presets() -> Vec<TtsPreset> {
    LOADED.lock().unwrap().as_ref().map(|l| l.tts.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn addons_open_folder(app: AppHandle) -> Result<(), String> {
    let d = dir(&app).ok_or("フォルダが分かりません")?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer").arg(&d).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v1.7.20"), Some((1, 7, 20)));
        assert_eq!(parse_version("1.8"), Some((1, 8, 0)));
        assert_eq!(parse_version(""), None);
        assert!(parse_version("1.7.20") < parse_version("1.8.0"));
    }

    #[test]
    fn stays_inside_the_addon_folder() {
        let base = Path::new(r"C:\addons\a");
        assert!(inside(base, "style.css").is_some());
        assert!(inside(base, r"css\list.css").is_some());
        assert!(inside(base, r"..\b\x.css").is_none());
        assert!(inside(base, r"C:\Windows\x.css").is_none());
    }

    #[test]
    fn loads_an_addon() {
        let dir = std::env::temp_dir().join(format!("reacast-addon-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("names.txt"), "# コメント\nたろう\nはなこ\n").unwrap();
        std::fs::write(dir.join("list.css"), ".item { color: red; }").unwrap();
        std::fs::write(
            dir.join("addon.json"),
            r#"{ "name": "テスト", "author": "だれか", "version": "1.0", "minReaCast": "1.0.0",
                 "css": { "comments": "list.css", "unknown": "x.css" },
                 "names": { "replace": ["名無しさん"], "file": "names.txt", "list": ["じろう"] },
                 "tts": [ { "label": "読子", "commandPath": "{addon}\\yomiko.exe", "commandArgs": "/T #Res#" } ] }"#,
        )
        .unwrap();
        let (info, loaded) = load_one(&dir, "test", true);
        assert_eq!(info.error, None);
        assert_eq!(info.name, "テスト");
        assert_eq!(info.css_pages, vec!["comments".to_string()]);
        assert_eq!(info.names, 3);
        assert!(info.tts[0].command_path.ends_with("yomiko.exe") && !info.tts[0].command_path.contains("{addon}"));
        assert_eq!(loaded.names[0].sources, vec!["bbs".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_the_same_name_for_the_same_person() {
        assert_eq!(stable_index("ID:abc", 7), stable_index("ID:abc", 7));
        assert!(stable_index("ID:abc", 7) < 7);
    }
}
