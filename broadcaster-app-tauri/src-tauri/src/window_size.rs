//! 窓の大きさと位置を覚えておいて、次に開いた時も同じ所に同じ大きさで出す。
//! 文字の大きさを「大」にしている人が、毎回窓を広げ直さなくていいように。
//! appデータのフォルダの window-sizes.json に、窓のラベルごとに入れる
//! (大きさは論理サイズ、位置は画面の座標(物理))。
//! 覚えた位置がどのモニターにも入っていない時(モニターを外した時など)は、今まで通り真ん中に出す。
//! 一部だけはみ出している時は、全部見えるように内側に寄せる。
//! ReaCast・ReaTapの両方で同じファイルを使う(src/window_size.rs)。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, PhysicalPosition, WebviewWindow};

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Saved {
    w: f64,
    h: f64,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
}

static SIZES: Mutex<Option<BTreeMap<String, Saved>>> = Mutex::new(None);
/// 窓ごとの最初の大きさ(「元に戻す」で使う)
static DEFAULTS: Mutex<BTreeMap<String, (f64, f64)>> = Mutex::new(BTreeMap::new());

fn path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("window-sizes.json"))
}

fn with_sizes<R>(app: &tauri::AppHandle, f: impl FnOnce(&mut BTreeMap<String, Saved>) -> R) -> R {
    let mut guard = SIZES.lock().unwrap();
    let sizes = guard.get_or_insert_with(|| {
        path(app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    f(sizes)
}

/// 前に閉じた時の大きさ(覚えていなければdefault)。小さすぎ・大きすぎる値は使わない
pub fn saved(app: &tauri::AppHandle, label: &str, default: (f64, f64), min: (f64, f64)) -> (f64, f64) {
    DEFAULTS.lock().unwrap().insert(label.to_string(), default);
    match with_sizes(app, |s| s.get(label).copied()) {
        Some(s) if s.w.is_finite() && s.h.is_finite() && s.w >= min.0 && s.h >= min.1 && s.w <= 8000.0 && s.h <= 8000.0 => (s.w, s.h),
        _ => default,
    }
}

fn save(app: &tauri::AppHandle) {
    let Some(p) = path(app) else { return };
    let text = with_sizes(app, |s| serde_json::to_string_pretty(s).unwrap_or_default());
    if let Err(e) = std::fs::write(&p, text) {
        log::warn!("窓の大きさを保存できませんでした: {e}");
    }
}

/// 覚えた位置に動かす。どのモニターの作業領域にも、窓の上の端(タイトルバーのあたり)が
/// 入っていなければ動かさない(=真ん中のまま)。はみ出す分は、そのモニターの内側に寄せる
fn place(win: &WebviewWindow) {
    let Some((x, y)) = with_sizes(win.app_handle(), |s| s.get(win.label()).and_then(|v| Some((v.x?, v.y?)))) else {
        return;
    };
    let Ok(size) = win.outer_size() else { return };
    let (w, h) = (size.width as i32, size.height as i32);
    let Ok(monitors) = win.available_monitors() else { return };
    // 上の端の帯(高さ30px)と一番広く重なるモニター
    let mut best: Option<(i64, tauri::PhysicalRect<i32, u32>)> = None;
    for m in monitors {
        let a = *m.work_area();
        let (ax, ay, aw, ah) = (a.position.x, a.position.y, a.size.width as i32, a.size.height as i32);
        let ow = (x + w).min(ax + aw) - x.max(ax);
        let oh = (y + 30).min(ay + ah) - y.max(ay);
        if ow > 40 && oh > 0 {
            let area = ow as i64 * oh as i64;
            if best.map_or(true, |(b, _)| area > b) {
                best = Some((area, a));
            }
        }
    }
    let Some((_, a)) = best else { return };
    let (ax, ay, aw, ah) = (a.position.x, a.position.y, a.size.width as i32, a.size.height as i32);
    let nx = if w >= aw { ax } else { x.clamp(ax, ax + aw - w) };
    let ny = if h >= ah { ay } else { y.clamp(ay, ay + ah - h) };
    let _ = win.set_position(PhysicalPosition::new(nx, ny));
}

/// 覚えた位置に動かして、大きさ・位置を変えたら覚える(閉じる時にファイルに書く)。
/// 最小化・最大化・隠している時の大きさ・位置は覚えない。
/// 窓を閉じるとアプリが終わる窓は、終了の処理より前に呼ぶこと(書く前に終わらないように)
pub fn track(win: &WebviewWindow) {
    place(win);
    let w = win.clone();
    let app = win.app_handle().clone();
    let label = win.label().to_string();
    let usable = move |w: &WebviewWindow| !w.is_minimized().unwrap_or(false) && !w.is_maximized().unwrap_or(false) && w.is_visible().unwrap_or(false);
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(size) => {
            if size.width == 0 || size.height == 0 || !usable(&w) {
                return;
            }
            let logical = size.to_logical::<f64>(w.scale_factor().unwrap_or(1.0));
            with_sizes(&app, |s| {
                let e = s.entry(label.clone()).or_insert(Saved { w: 0.0, h: 0.0, x: None, y: None });
                e.w = logical.width.round();
                e.h = logical.height.round();
            });
        }
        tauri::WindowEvent::Moved(_) => {
            if !usable(&w) {
                return;
            }
            let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) else { return };
            let logical = size.to_logical::<f64>(w.scale_factor().unwrap_or(1.0));
            with_sizes(&app, |s| {
                let e = s.entry(label.clone()).or_insert(Saved { w: logical.width.round(), h: logical.height.round(), x: None, y: None });
                e.x = Some(pos.x);
                e.y = Some(pos.y);
            });
        }
        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed => save(&app),
        _ => {}
    });
}

/// 覚えた大きさ・位置を消して、開いている窓を最初の大きさで真ん中に戻す
#[tauri::command]
pub fn window_layout_reset(app: tauri::AppHandle) {
    with_sizes(&app, |s| s.clear());
    save(&app);
    let defaults = DEFAULTS.lock().unwrap().clone();
    for (label, (w, h)) in defaults {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.unmaximize();
            let _ = win.set_size(tauri::LogicalSize::new(w, h));
            let _ = win.center();
        }
    }
}
