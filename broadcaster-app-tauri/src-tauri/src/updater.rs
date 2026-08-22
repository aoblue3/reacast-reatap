//! 単体exe配布のまま(インストーラ形式に変えずに)アプリ自身を更新する機能。
//!
//! Tauri公式のupdaterプラグインはWindows上ではNSIS/MSIインストーラでの配布が
//! 前提になっており、「ダブルクリックでそのまま起動できる単体exe」というこの
//! アプリの配布方針とは相容れない(公式ドキュメント: インストーラ形式の
//! アップデート成果物が無いと動かない)。そのため、ここでは昔ながらの
//! 「新しいexeを一旦別名でダウンロード→自分自身が終了→ヘルパースクリプトが
//! ファイルを差し替えて再起動」という手作りの方式で実現している。
//!
//! 全体の流れ:
//! 1. `check_for_update` - GitHub Releasesの最新リリースAPIを叩き、
//!    現在のバージョンより新しいものがあれば情報を返す。
//! 2. `download_and_apply_update` - 新しいexeをダウンロードし(SHA256が
//!    取得できていれば検証し)、PowerShellの小さなヘルパースクリプトを
//!    起動してから自分自身は終了する。ヘルパースクリプトが、このプロセスの
//!    終了を待ってからexeを差し替え、再起動する。
//!
//! 注意点(使い方ガイドにも記載):
//! - Program Filesのような書き込み権限が無い場所に置いて実行していると、
//!   差し替えに失敗する(そのexeはユーザーが好きな場所に置いて使う設計なので、
//!   通常は問題にならない想定)。
//! - コード署名はしていないため、差し替え後に再起動したexeもWindows
//!   SmartScreenの警告対象になりうる(初回ダウンロード時と同様)。
//! - ビルド時に`APP_VERSION`/`UPDATE_CHECK_REPO`環境変数が設定されていない
//!   場合(ローカルでの開発ビルドなど)は、アップデート確認機能そのものを
//!   無効化する(常に「更新なし」を返す)。

use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use sha2::{Digest, Sha256};

/// このバイナリがビルドされた時のGitHubタグ(例: "v1.0.0")。
/// GitHub Actions側で `APP_VERSION: ${{ github.ref_name }}` として渡す想定
/// (詳しくは.github/workflows/build-release.yml参照)。ローカルビルド等で
/// 未設定の場合は空文字列になり、アップデート確認は常に「なし」を返す。
fn current_version() -> &'static str {
    option_env!("APP_VERSION").unwrap_or("")
}

/// アップデート確認先のGitHubリポジトリ("owner/repo"形式)。GitHub Actions側で
/// `UPDATE_CHECK_REPO: ${{ github.repository }}` として自動的に渡される。
fn update_repo() -> &'static str {
    option_env!("UPDATE_CHECK_REPO").unwrap_or("")
}

/// このアプリがGitHub Releasesに公開するexeのファイル名。
/// .github/workflows/build-release.yml のアップロード対象ファイル名と
/// 必ず一致させること。
const ASSET_NAME: &str = "ReaCast.exe";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    pub download_url: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

/// "v1.2.3" のようなタグ名を(major, minor, patch)へパースする。
/// 想定外の形式は全て0扱いにする(比較を安全側に倒すだけなので、
/// 万一パースに失敗しても「更新なしと誤判定される」以上の実害は無い)。
fn parse_version(v: &str) -> (u32, u32, u32) {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

#[cfg(target_os = "windows")]
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 現在起動しているバージョンより新しいリリースがGitHub上にあるか確認する。
/// APP_VERSION/UPDATE_CHECK_REPOのどちらかが未設定(=開発ビルド)の場合は
/// 常にNoneを返す。
#[tauri::command]
pub async fn check_for_update() -> Result<Option<UpdateInfo>, String> {
    let repo = update_repo();
    let current = current_version();
    if repo.is_empty() || current.is_empty() {
        return Ok(None);
    }

    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent("ReaCast-Updater")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHubへの問い合わせに失敗しました: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "GitHubへの問い合わせに失敗しました(HTTP {})",
            resp.status()
        ));
    }
    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("応答の解析に失敗しました: {e}"))?;

    if !is_newer(&release.tag_name, current) {
        return Ok(None);
    }

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == ASSET_NAME)
        .ok_or_else(|| format!("最新リリースに{ASSET_NAME}が見つかりませんでした"))?;

    // {ASSET_NAME}.sha256 という名前の付随ファイルがあれば、差し替え前の
    // 整合性チェックに使う(無くてもアップデート自体は続行する)。
    let sha256_name = format!("{ASSET_NAME}.sha256");
    let sha256 = if let Some(sha_asset) = release.assets.iter().find(|a| a.name == sha256_name) {
        match client.get(&sha_asset.browser_download_url).send().await {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(s) => {
                    let hash = s.trim().split_whitespace().next().unwrap_or("").to_string();
                    if hash.is_empty() {
                        None
                    } else {
                        Some(hash)
                    }
                }
                Err(_) => None,
            },
            _ => None,
        }
    } else {
        None
    };

    Ok(Some(UpdateInfo {
        version: release.tag_name,
        notes: release.body,
        download_url: asset.browser_download_url.clone(),
        sha256,
    }))
}

#[cfg(target_os = "windows")]
mod apply {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    /// 新exe(new_exe_path)を現在のexe(current_exe_path)へ差し替えて再起動する
    /// ヘルパー(PowerShellスクリプト)を書き出し、デタッチした状態で起動する。
    /// 呼び出し元は、この関数が成功を返した直後に自分自身を終了させること
    /// (このプロセスが終了してファイルの占有が外れないと差し替えができない)。
    pub fn spawn_swap_and_relaunch_helper(
        current_pid: u32,
        new_exe_path: &std::path::Path,
        current_exe_path: &std::path::Path,
    ) -> std::io::Result<()> {
        let script_path = std::env::temp_dir().join(format!("reacast-update-{current_pid}.ps1"));
        let script = format!(
            r#"
$ErrorActionPreference = 'SilentlyContinue'
while (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{
    Start-Sleep -Milliseconds 300
}}
for ($i = 0; $i -lt 20; $i++) {{
    try {{
        Copy-Item -Path '{new_exe}' -Destination '{current_exe}' -Force -ErrorAction Stop
        break
    }} catch {{
        Start-Sleep -Milliseconds 500
    }}
}}
Remove-Item -Path '{new_exe}' -Force -ErrorAction SilentlyContinue
Start-Process -FilePath '{current_exe}'
Remove-Item -Path $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
"#,
            pid = current_pid,
            new_exe = new_exe_path.display(),
            current_exe = current_exe_path.display(),
        );
        std::fs::write(&script_path, script)?;

        Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script_path)
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()?;
        Ok(())
    }
}

/// ダウンロード→(可能なら)検証→差し替えヘルパー起動→自分は終了、を行う。
/// 呼び出し成功後、このプロセスは`app.exit(0)`されるためJS側へは
/// 応答が返らない(正常な動作)。
#[tauri::command]
pub async fn download_and_apply_update(
    app: tauri::AppHandle,
    download_url: String,
    sha256: Option<String>,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, download_url, sha256);
        return Err("この機能はWindows専用です".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;

        let client = reqwest::Client::builder()
            .user_agent("ReaCast-Updater")
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| format!("ダウンロードに失敗しました: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("ダウンロードに失敗しました(HTTP {})", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("ダウンロード内容の取得に失敗しました: {e}"))?;

        if let Some(expected) = sha256.as_ref() {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let actual = hex_encode(&hasher.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(
                    "ダウンロードしたファイルの検証に失敗しました(ハッシュ値が一致しません)。もう一度お試しください".to_string(),
                );
            }
        }

        let current_exe =
            std::env::current_exe().map_err(|e| format!("実行ファイルの場所を取得できませんでした: {e}"))?;
        let new_exe_path = std::env::temp_dir().join(format!("{ASSET_NAME}.new"));
        std::fs::write(&new_exe_path, &bytes)
            .map_err(|e| format!("一時ファイルの書き込みに失敗しました: {e}"))?;

        let pid = std::process::id();
        apply::spawn_swap_and_relaunch_helper(pid, &new_exe_path, &current_exe)
            .map_err(|e| format!("更新用ヘルパーの起動に失敗しました: {e}"))?;

        // ヘルパーが自分の終了を待っているので、ここで実際に終了する。
        app.exit(0);
        Ok(())
    }
}
