//! 単体exe配布のまま(インストーラ形式に変えずに)アプリ自身を更新する機能。
//!
//! Tauri公式のupdaterプラグインはWindows上ではNSIS/MSIインストーラでの配布が
//! 前提になっており、「ダブルクリックでそのまま起動できる単体exe」というこの
//! アプリの配布方針とは相容れない(公式ドキュメント: インストーラ形式の
//! アップデート成果物が無いと動かない)。そのため、ここでは昔ながらの
//! 「新しいexeを一旦別名でダウンロード→自分自身が終了→差し替え役が
//! ファイルを差し替えて再起動」という手作りの方式で実現している。
//!
//! 全体の流れ:
//! 1. `check_for_update` - GitHub Releasesの最新リリースAPIを叩き、
//!    現在のバージョンより新しいものがあれば情報を返す。
//! 2. `download_and_apply_update` - 新しいexeをダウンロードし(SHA256が
//!    取得できていれば検証し)、**ダウンロードした新exe自身**を特別な
//!    コマンドライン引数付きで起動してから自分自身は終了する。その新exeが
//!    (`handle_apply_update_cli_if_present`)、このプロセスの終了を待って
//!    からexeを差し替え、改めて自分自身(正しい場所にコピーされた後の版)を
//!    起動する。
//!
//! 過去にはこのステップ2をPowerShellの小さなヘルパースクリプトを書き出して
//! 実行する方式で実装していたが、テスターの環境でこのスクリプトが
//! (ログの1行目すら書き込まれないまま)一切実行された形跡が無いまま
//! アプリだけが終了する不具合が繰り返し発生した。「Tempに書き出した
//! スクリプトをpowershell -ExecutionPolicy Bypassで実行する」という
//! パターンはマルウェアの典型的な挙動とも重なるため、セキュリティソフトや
//! 組織のポリシーによってPowerShellの実行自体がブロックされていた可能性が
//! 高いと判断し、外部のスクリプトエンジンを一切使わない今の方式に変更した。
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
//! - この新しい方式は、ダウンロードした新exe自身に差し替えロジックが
//!   入っている前提のため、**この方式を含んだバージョンに一度手動で更新した
//!   後の、次回以降の自動アップデートから**有効になる(今動いている旧い
//!   バージョンには、この新しい差し替えロジック自体がまだ入っていないため)。

use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use sha2::{Digest, Sha256};
use std::time::Duration;

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
const ASSET_NAME: &str = "ReaTap.exe";

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
    // タイムアウトを指定していないと、通信が固まった場合に無期限に待ち続けて
    // しまい(reqwestは既定でタイムアウト無し)、ユーザーからは「OKを押しても
    // 何も起きない」ように見えてしまう不具合の原因になっていたため、明示的に
    // タイムアウトを設定する。
    let client = reqwest::Client::builder()
        .user_agent("ReaTap-Updater")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
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
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    // v1.3.3で追加したWindows Job Objectハードニング(pcwmp::harden_process_termination)
    // により、本体プロセス(ReaTap.exe)は「自分が終了したら子プロセスも道連れに
    // 終了する」Jobに所属するようになった。しかし、この差し替え役ヘルパー
    // プロセスは、呼び出し元(本体)がapp.exit(0)される直後まさにその瞬間も
    // 生き残っていなければならない(ファイル差し替え・再起動を行う役目のため)。
    // 何もしなければJobのKILL_ON_JOB_CLOSEに巻き込まれて一緒に終了してしまい、
    // 自動アップデート自体が機能しなくなる(元のバグより悪い退行になる)ため、
    // このヘルパーの起動時にだけ明示的にJobから離脱させるフラグを付ける。
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    /// このプロセスが「差し替え役」として起動された時に付ける特別な引数。
    /// `download_and_apply_update`がダウンロード後に新exe自身をこの引数
    /// 付きで起動し、main()側で`handle_apply_update_cli_if_present`を通して
    /// 検知する(検知したら通常のTauriアプリとしては起動しない)。
    pub const APPLY_UPDATE_FLAG: &str = "--reatap-apply-update";

    fn log_path_for(old_pid: u32) -> PathBuf {
        std::env::temp_dir().join(format!("reatap-update-{old_pid}.log"))
    }

    /// 差し替え処理の各ステップをログファイルに追記する。ログ自体の書き込みに
    /// 失敗しても(書き込み先の権限問題等)、差し替え処理そのものは継続する。
    fn log(old_pid: u32, msg: &str) {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path_for(old_pid))
        {
            let _ = writeln!(f, "[{millis}] {msg}");
        }
    }

    /// 現在のコマンドライン引数が「差し替え役」としての起動かどうかを調べる。
    /// 該当する場合、この関数の呼び出し元(main.rs)は通常のTauriアプリ起動を
    /// せず、代わりにこの関数が「待機→コピー→再起動」を全て行って
    /// プロセスごと終了する(戻ってこない)。該当しなければ何もせず戻る。
    pub fn handle_apply_update_cli_if_present() {
        let args: Vec<String> = std::env::args().collect();
        if args.len() < 4 || args[1] != APPLY_UPDATE_FLAG {
            return;
        }
        let old_pid: u32 = match args[2].parse() {
            Ok(v) => v,
            Err(_) => return,
        };
        let target_path = PathBuf::from(&args[3]);

        run_apply_update(old_pid, &target_path);
        std::process::exit(0);
    }

    fn run_apply_update(old_pid: u32, target_path: &Path) {
        log(old_pid, "apply-update helper started (native, no script engine)");

        wait_for_process_exit(old_pid, Duration::from_secs(30));
        log(
            old_pid,
            "old process exited (or wait timed out); pausing briefly before copy",
        );
        // 多重起動防止(single-instance)の仕組みが、旧プロセスの終了直後には
        // まだ完全に解放されていない可能性を考慮し、コピー前に少しだけ間を置く。
        std::thread::sleep(Duration::from_millis(800));

        let self_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                log(old_pid, &format!("failed to determine own exe path: {e}"));
                return;
            }
        };

        log(old_pid, "copying new exe into place");
        let mut copied = false;
        for i in 0..20 {
            match std::fs::copy(&self_path, target_path) {
                Ok(_) => {
                    copied = true;
                    log(old_pid, &format!("copy succeeded (attempt {i})"));
                    break;
                }
                Err(e) => {
                    log(old_pid, &format!("copy attempt {i} failed: {e}"));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        if !copied {
            log(
                old_pid,
                "copy failed after all retries; giving up (existing exe left untouched)",
            );
            return;
        }

        // 起動が成功しても、直後に(多重起動防止の判定がまだ旧プロセスを
        // 「起動中」とみなしている等の理由で)新しいプロセスが即座に終了して
        // しまうことがありうるため、起動後に本当に生き残っているかを確認し、
        // 生き残っていなければ間隔を空けて再試行する。
        let mut launched = false;
        for j in 0..3 {
            match Command::new(target_path)
                .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                .spawn()
            {
                Ok(mut child) => {
                    log(
                        old_pid,
                        &format!(
                            "launch attempt {j}: spawn succeeded (pid {}), verifying it stayed running",
                            child.id()
                        ),
                    );
                    std::thread::sleep(Duration::from_millis(1500));
                    match child.try_wait() {
                        Ok(None) => {
                            launched = true;
                            log(old_pid, &format!("launch attempt {j}: verified still running"));
                            break;
                        }
                        Ok(Some(status)) => {
                            log(
                                old_pid,
                                &format!(
                                    "launch attempt {j}: process exited immediately (status: {status}), retrying"
                                ),
                            );
                            std::thread::sleep(Duration::from_millis(1000));
                        }
                        Err(e) => {
                            log(old_pid, &format!("launch attempt {j}: failed to check status: {e}"));
                        }
                    }
                }
                Err(e) => {
                    log(old_pid, &format!("launch attempt {j}: spawn itself failed: {e}"));
                    std::thread::sleep(Duration::from_millis(1500));
                }
            }
        }
        if !launched {
            log(old_pid, "all launch attempts failed verification; giving up");
        }

        // 自分自身(Temp内の一時コピー)の削除はベストエフォート(失敗しても
        // 実害は無く、単にTempにファイルが残るだけ)。
        let _ = std::fs::remove_file(&self_path);
        log(old_pid, "helper finished");
    }

    /// 指定したPIDのプロセスが終了するまで待つ(最大max_wait)。
    /// 既に終了している・ハンドルの取得自体に失敗した場合は、待つべき対象が
    /// 無いとみなして即座に戻る。
    fn wait_for_process_exit(pid: u32, max_wait: Duration) {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) };
        let Ok(handle) = handle else {
            return;
        };
        let timeout_ms = u32::try_from(max_wait.as_millis()).unwrap_or(u32::MAX);
        let _ = unsafe { WaitForSingleObject(handle, timeout_ms) };
        let _ = unsafe { CloseHandle(handle) };
    }

    /// ダウンロードした新exe自身(new_exe_path)を、「現在のプロセス
    /// (current_pid)の終了を待ってからcurrent_exe_pathへ差し替えて再起動する
    /// 役」として、特別な引数付きでデタッチした状態で起動する。呼び出し元は、
    /// この関数が成功を返した直後に自分自身を終了させること(このプロセスが
    /// 終了してファイルの占有が外れないと差し替えができない)。
    pub fn spawn_apply_update_helper(
        current_pid: u32,
        new_exe_path: &Path,
        current_exe_path: &Path,
    ) -> std::io::Result<()> {
        Command::new(new_exe_path)
            .arg(APPLY_UPDATE_FLAG)
            .arg(current_pid.to_string())
            .arg(current_exe_path)
            // CREATE_BREAKAWAY_FROM_JOBの理由は本ファイル冒頭の定数コメント参照。
            // このヘルパーだけは本体のJob Objectに巻き込まれず生き残る必要がある。
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()?;
        Ok(())
    }
}

/// main.rs(バイナリのエントリポイント)から、通常のTauriアプリ起動を
/// する前に一度だけ呼び出す。差し替え役としての起動でなければ何もしない。
pub fn handle_apply_update_cli_if_present() {
    #[cfg(target_os = "windows")]
    apply::handle_apply_update_cli_if_present();
}

/// ダウンロード→(可能なら)検証→差し替え役起動→自分は終了、を行う。
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

        // check_for_updateと同じ理由で、ダウンロード用クライアントにも明示的な
        // タイムアウトを設定する(exe本体は数MB〜十数MBあるため、接続確立自体は
        // 短めのタイムアウトにしつつ、転送全体には余裕を持たせてある)。
        let client = reqwest::Client::builder()
            .user_agent("ReaTap-Updater")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
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
        apply::spawn_apply_update_helper(pid, &new_exe_path, &current_exe)
            .map_err(|e| format!("更新用ヘルパーの起動に失敗しました: {e}"))?;

        // 差し替え役(新exe自身)が自分の終了を待っているので、ここで実際に終了する。
        app.exit(0);
        Ok(())
    }
}
