//! Googleの絵文字(使いたい人だけ、設定画面からダウンロードする)。
//!
//! 配信者のReaCastで「絵文字をすべてGoogleの絵文字で表示する」を使っていると、配信画面の
//! 絵文字はGoogleの絵になるが、ReaTapのボタンはWindowsの絵文字のままで絵が違って見える。
//! ReaCastと同じNoto Color Emoji(OFL-1.1。約3MB)をダウンロードしてONにすると、
//! メイン画面とリアクションバーの絵文字をそれで表示する(shared/google-emoji.js)。
//!
//! ダウンロードするファイルはReaCast(broadcaster-app-tauri/src-tauri/src/emoji_font.rs)と同じ。
//! GitHubの決まった版(コミット)に固定して、中身がその版と同じか(gitのblobのSHA-1)を
//! 確かめてから使う。

use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager};

use crate::config_store::ConfigStore;

const FONT_URL: &str =
    "https://raw.githubusercontent.com/googlefonts/noto-emoji/99701e07d3d1fe1bdbcdb14964547f0fd592966f/2D/fonts/Noto-COLRv1-noflags.ttf";
/// 上の版のファイルの、gitのblobのSHA-1と大きさ
const FONT_BLOB_SHA1: &str = "a88cf77a3b73f883f5c9e860ddc9de5f5de32cf0";
const FONT_SIZE: usize = 3_038_756;
const FILE_NAME: &str = "Noto-COLRv1-noflags.ttf";
/// 「Googleの絵文字で表示する」(設定ファイルのキー)
const ON_KEY: &str = "googleEmoji";

static FONT_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 起動時に1回だけ呼ぶ(フォントを置く場所を決める)
pub fn init(app: &AppHandle) {
    if let Ok(dir) = app.path().app_local_data_dir() {
        let _ = FONT_PATH.set(dir.join("emoji-font").join(FILE_NAME));
    }
}

fn installed_path() -> Option<PathBuf> {
    FONT_PATH.get().filter(|p| p.exists()).cloned()
}

fn status(store: &ConfigStore) -> serde_json::Value {
    let on = store.get(ON_KEY).and_then(|v| v.as_bool()).unwrap_or(false);
    serde_json::json!({ "installed": installed_path().is_some(), "on": on, "sizeBytes": FONT_SIZE })
}

/// gitのblobのSHA-1("blob <大きさ>\0" + 中身)
fn git_blob_sha1(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", data.len()).as_bytes());
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// メイン画面と、開いているリアクションバー全部に知らせる
fn notify(app: &AppHandle) {
    let _ = app.emit("emoji-font:changed", status(&app.state::<ConfigStore>()));
}

#[tauri::command]
pub fn emoji_font_status(store: tauri::State<ConfigStore>) -> serde_json::Value {
    status(&store)
}

/// 「Googleの絵文字で表示する」のON/OFF
#[tauri::command]
pub fn emoji_font_set_on(app: AppHandle, store: tauri::State<ConfigStore>, on: bool) -> Result<(), String> {
    store.set(ON_KEY.to_string(), serde_json::Value::Bool(on)).map_err(|e| e.to_string())?;
    log::info!("Googleの絵文字で表示する: {}", if on { "ON" } else { "OFF" });
    notify(&app);
    Ok(())
}

/// ダウンロードしたフォントの中身(画面がFontFaceにして使う)
#[tauri::command(async)]
pub fn emoji_font_data() -> Result<tauri::ipc::Response, String> {
    let path = installed_path().ok_or("ダウンロードしていません")?;
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(data))
}

#[tauri::command]
pub async fn emoji_font_download(app: AppHandle) -> Result<(), String> {
    let path = FONT_PATH.get().cloned().ok_or("フォントを置く場所が分かりません")?;
    let resp = reqwest::Client::new()
        .get(FONT_URL)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("ダウンロードできませんでした: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ダウンロードできませんでした(HTTP {})", resp.status()));
    }
    let data = resp.bytes().await.map_err(|e| format!("ダウンロードできませんでした: {e}"))?;
    if data.len() != FONT_SIZE || git_blob_sha1(&data) != FONT_BLOB_SHA1 {
        return Err("ダウンロードしたファイルが、決まったものと違うので使いませんでした".into());
    }
    let dir = path.parent().ok_or("フォントを置く場所が分かりません")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("download");
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    log::info!("Googleの絵文字のフォントをダウンロードしました({} バイト)", data.len());
    notify(&app);
    Ok(())
}

#[tauri::command]
pub fn emoji_font_remove(app: AppHandle, store: tauri::State<ConfigStore>) -> Result<(), String> {
    if let Some(p) = installed_path() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        log::info!("Googleの絵文字のフォントを削除しました");
    }
    let _ = store.set(ON_KEY.to_string(), serde_json::Value::Bool(false));
    notify(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_git_blob_sha1() {
        // `git hash-object` と同じ値
        assert_eq!(git_blob_sha1(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(git_blob_sha1(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    }
}
