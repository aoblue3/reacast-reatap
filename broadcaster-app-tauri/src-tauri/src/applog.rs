//! 不具合調査用のログファイル。
//!
//! 配布版は`windows_subsystem = "windows"`でコンソールを持たないため、
//! エラーを出力してもどこにも残らない。そこでtauri-plugin-logでログフォルダ
//! (`%LOCALAPPDATA%\<identifier>\logs`)にファイルとして書き残し、画面側
//! (shared/error-log.js)のエラーやRustのpanicもここに集める。
//! 利用者には設定画面の「ログフォルダを開く」から送ってもらう想定。
//! viewer-app-tauri/src-tauri/src/applog.rs と同じ内容。

use tauri::{plugin::TauriPlugin, AppHandle, Manager, Runtime};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

/// 1ファイルの上限(超えたら日付付きの名前に変えて新しいファイルにする)
const MAX_FILE_SIZE: u128 = 1_000_000;
/// 古いログを何個まで残すか
const KEEP_FILES: usize = 5;

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let mut targets = vec![Target::new(TargetKind::LogDir { file_name: None })];
    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
    }
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets(targets)
        .level(log::LevelFilter::Info)
        .max_file_size(MAX_FILE_SIZE)
        .rotation_strategy(RotationStrategy::KeepSome(KEEP_FILES))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .build()
}

/// panicをログに残す(既定の処理=標準エラー出力もそのまま行う)。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
}

/// 起動時に1行、バージョンとOSを書いておく(送ってもらったログがどの版か分かるように)。
pub fn log_startup() {
    let version = option_env!("APP_VERSION").unwrap_or("開発版");
    log::info!("起動: {version} ({} {})", std::env::consts::OS, std::env::consts::ARCH);
}

/// 画面側(shared/error-log.js)からのエラー・警告をログに書く。
#[tauri::command]
pub fn log_from_frontend(window: tauri::Window, level: String, message: String) {
    let label = window.label();
    match level.as_str() {
        "error" => log::error!(target: "frontend", "[{label}] {message}"),
        "warn" => log::warn!(target: "frontend", "[{label}] {message}"),
        _ => log::info!(target: "frontend", "[{label}] {message}"),
    }
}

/// ログフォルダをエクスプローラーで開く。
#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = dir;
        return Err("Windows以外では未対応です".into());
    }
    Ok(())
}
