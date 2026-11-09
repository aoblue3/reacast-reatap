//! 窓の大きさを覚えておいて、次に開いた時も同じ大きさで出す(位置は覚えない)。
//! 文字の大きさを「大」にしている人が、毎回窓を広げ直さなくていいように。
//! appデータのフォルダの window-sizes.json に、窓のラベルごとの論理サイズ(幅・高さ)を入れる。
//! ReaCast・ReaTapの両方で同じファイルを使う(src/window_size.rs)。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, WebviewWindow};

static SIZES: Mutex<Option<BTreeMap<String, (f64, f64)>>> = Mutex::new(None);

fn path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("window-sizes.json"))
}

fn with_sizes<R>(app: &tauri::AppHandle, f: impl FnOnce(&mut BTreeMap<String, (f64, f64)>) -> R) -> R {
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
    match with_sizes(app, |s| s.get(label).copied()) {
        Some((w, h)) if w.is_finite() && h.is_finite() && w >= min.0 && h >= min.1 && w <= 8000.0 && h <= 8000.0 => (w, h),
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

/// 大きさを変えたら覚えて、閉じる時にファイルに書く(最小化・最大化の時の大きさは覚えない)。
/// 窓を閉じるとアプリが終わる窓は、終了の処理より前に呼ぶこと(書く前に終わらないように)
pub fn track(win: &WebviewWindow) {
    let w = win.clone();
    let app = win.app_handle().clone();
    let label = win.label().to_string();
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(size) => {
            if size.width == 0 || size.height == 0 || w.is_minimized().unwrap_or(false) || w.is_maximized().unwrap_or(false) {
                return;
            }
            let scale = w.scale_factor().unwrap_or(1.0);
            let logical = size.to_logical::<f64>(scale);
            with_sizes(&app, |s| s.insert(label.clone(), (logical.width.round(), logical.height.round())));
        }
        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed => save(&app),
        _ => {}
    });
}
