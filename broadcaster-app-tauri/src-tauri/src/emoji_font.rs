//! 新しい絵文字のフォント(使いたい人だけ、設定画面からダウンロードする)。
//!
//! Windows 10の絵文字のフォント(Segoe UI Emoji)には、新しい絵文字(🫍 U+1FACDなど)が
//! 入っていないので、レスに書かれると□で表示される。GoogleのNoto Color Emoji
//! (OFL-1.1。約3MB)をダウンロードしておくと、Windowsのフォントに無い絵文字だけ
//! それで表示する(shared/comment-common.jsの"ReaCast Emoji")。字幕・レス一覧・OBSの
//! 画面には、obs_bridgeのHTTP(/emoji-font.ttf)から配る。
//!
//! ダウンロードするファイルは、GitHubの決まった版(コミット)に固定して、中身がその版と
//! 同じか(gitのblobのSHA-1)を確かめてから使う。

use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager};

const FONT_URL: &str =
    "https://raw.githubusercontent.com/googlefonts/noto-emoji/99701e07d3d1fe1bdbcdb14964547f0fd592966f/2D/fonts/Noto-COLRv1-noflags.ttf";
/// 上の版のファイルの、gitのblobのSHA-1と大きさ(GitHubのAPIで確認。Unicode 18まで入っている)
const FONT_BLOB_SHA1: &str = "a88cf77a3b73f883f5c9e860ddc9de5f5de32cf0";
const FONT_SIZE: usize = 3_038_756;
const FILE_NAME: &str = "Noto-COLRv1-noflags.ttf";

static FONT_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 起動時に1回だけ呼ぶ(フォントを置く場所を決める)
pub fn init(app: &AppHandle) {
    if let Ok(dir) = app.path().app_local_data_dir() {
        let _ = FONT_PATH.set(dir.join("emoji-font").join(FILE_NAME));
    }
}

/// ダウンロード済みならその場所(obs_bridgeが配る)
pub(crate) fn installed_path() -> Option<PathBuf> {
    FONT_PATH.get().filter(|p| p.exists()).cloned()
}

/// gitのblobのSHA-1("blob <大きさ>\0" + 中身)
fn git_blob_sha1(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", data.len()).as_bytes());
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 表示中の画面(OBS・ReaCastの字幕など)に、フォントが入った・消えたことを知らせる
fn notify(app: &AppHandle) {
    app.state::<crate::obs_bridge::ObsBridge>()
        .push_voice(serde_json::json!({ "type": "emojiFont", "installed": installed_path().is_some() }));
    let _ = app.emit("emoji-font:changed", installed_path().is_some());
}

#[tauri::command]
pub fn emoji_font_status() -> serde_json::Value {
    serde_json::json!({ "installed": installed_path().is_some(), "sizeBytes": FONT_SIZE })
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
    log::info!("新しい絵文字のフォントをダウンロードしました({} バイト)", data.len());
    notify(&app);
    Ok(())
}

#[tauri::command]
pub fn emoji_font_remove(app: AppHandle) -> Result<(), String> {
    if let Some(p) = installed_path() {
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        log::info!("新しい絵文字のフォントを削除しました");
    }
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
