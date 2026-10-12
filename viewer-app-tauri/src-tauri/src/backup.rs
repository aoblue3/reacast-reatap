//! 設定のバックアップ・引っ越し。設定ファイルの中身をまるごと1つのファイルに
//! 書き出し、別のPC(または入れ直した後)で読み込む。読み込んだらアプリを
//! 再起動して、すべての設定を反映する。
//! broadcaster-app-tauri/src-tauri/src/backup.rs と同じ内容(APPだけ違う)。

use crate::config_store::ConfigStore;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

const APP: &str = "ReaTap";

/// 書き出したファイルの場所を返す(キャンセルしたらNone)。
#[tauri::command(async)]
pub fn export_settings(app: AppHandle) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_title("設定の書き出し先")
        .set_file_name(format!("{APP}の設定.json"))
        .add_filter("設定ファイル", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let json = serde_json::json!({
        "app": APP,
        "version": option_env!("APP_VERSION").unwrap_or("開発版"),
        "settings": app.state::<ConfigStore>().snapshot(),
    });
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("書き出せませんでした: {e}"))?;
    log::info!("設定を書き出しました: {}", path.display());
    Ok(Some(path.display().to_string()))
}

/// 設定を読み込んでアプリを再起動する(キャンセルしたらfalse)。
#[tauri::command(async)]
pub fn import_settings(app: AppHandle) -> Result<bool, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_title("読み込む設定ファイル")
        .add_filter("設定ファイル", &["json"])
        .blocking_pick_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("読み込めませんでした: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|_| "設定ファイルの形式ではありません".to_string())?;
    match json.get("app").and_then(|v| v.as_str()) {
        Some(a) if a == APP => {}
        Some(other) => return Err(format!("{other}の設定ファイルです({APP}では読み込めません)")),
        None => return Err("設定ファイルの形式ではありません".into()),
    }
    let settings = json
        .get("settings")
        .and_then(|v| v.as_object())
        .cloned()
        .ok_or("設定ファイルの中身がありません")?;
    app.state::<ConfigStore>()
        .replace_all(settings)
        .map_err(|e| format!("設定を保存できませんでした: {e}"))?;
    log::info!("設定を読み込みました: {}。再起動します", path.display());
    app.restart();
}
