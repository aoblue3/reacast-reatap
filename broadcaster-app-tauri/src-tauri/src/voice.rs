//! 話した字幕(配信者が話した言葉を字幕にして、自動で翻訳する)。
//!
//! ReaCastの画面(WebView2)ではブラウザの音声認識が使えないため、Chrome(無ければEdge)を
//! 専用の小さな窓(--app)で開いて音声認識の画面(frontend/voice_input.html)を
//! 動かす。聞き取った文字はobs_bridgeのWebSocketで届き、OBSの「話した字幕」の
//! 画面(frontend/voice.html)へ送る。話し終わった文はGoogle翻訳(非公式の
//! 方法)で翻訳して、下に出す。
//!
//! Edgeは専用のプロフィール(--user-data-dir)で開くので、普段使いのEdgeとは
//! 別のプロセスになり、ONOFFや終了の時にこちらから閉じられる。マイクの許可は
//! そのプロフィールの中だけで自動で許可する(--use-fake-ui-for-media-stream)。

use crate::comments::{CommentEngine, CommentSettings};
use crate::obs_bridge::{ObsBridge, OBS_HTTP_PORT};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

/// 起動したEdgeと、その時に開いたURL(言語が変わったら開き直すため)
#[derive(Default)]
pub struct VoiceManager {
    edge: Mutex<Option<(EdgeProcess, String)>>,
    status: Mutex<serde_json::Value>,
    /// マイクを止めている(ホットキー・ボタン)。止めている間はEdgeの聞き取りも止める
    muted: std::sync::atomic::AtomicBool,
    /// 「Edge優先」で、Edgeの音声認識が使えなくなった(上限に達した等)。ReaCastを
    /// 起動し直すか「開き直す」まではChromeを使う
    edge_failed: std::sync::atomic::AtomicBool,
    /// 今開いているブラウザの名前("Edge"/"Chrome"。状態の表示用)
    browser_name: Mutex<String>,
    /// 音声認識の画面が見つけたマイクの名前の一覧(設定画面の「マイク」に出す)
    devices: Mutex<Vec<String>>,
}

/// Edgeの音声認識の使用時間の上限(秒)。Edgeはプロフィールの「Local State」の
/// speech_recognition_limiter.amount_usedに使った秒数を記録していて、約10時間(36000秒)で
/// 止める(2026-10-18に本人のPCで36020秒で止まった)。記録は一定の期間で戻る
/// (2026-10-27には8471秒に戻っていた。戻るまでの期間は分かっていない)。
/// ここは記録を読むだけで、書き換えはしない(上限をすり抜けることになるため)
const EDGE_LIMIT_SECS: f64 = 36000.0;
/// 「Edge優先」で、残りがこれより少なければ最初からChromeで開く
const EDGE_MARGIN_SECS: f64 = 120.0;

/// Edgeの音声認識の使用時間(秒)と、最後に記録された時刻(UNIXミリ秒)。読めなければNone
fn edge_usage(app: &AppHandle) -> Option<(f64, Option<u64>)> {
    let path = app.path().app_local_data_dir().ok()?.join("voice-edge").join("Local State");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let lim = v.get("speech_recognition_limiter")?;
    let used = lim.get("amount_used")?.as_f64()?;
    // Chromium系の時刻(1601年からのマイクロ秒。文字列で入っている)をUNIXミリ秒に直す
    let checkpoint = lim
        .get("last_checkpoint")
        .and_then(|c| c.as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| c.as_u64()))
        .and_then(|us| (us / 1000).checked_sub(11_644_473_600_000));
    Some((used, checkpoint))
}

/// 設定の「音声認識に使うブラウザ」から、実際に開くブラウザを決める。
/// "edge-first"(Edge優先)は、Edgeが使えなくなっている時・上限ぎりぎりの時だけChrome
fn effective_browser(app: &AppHandle, choice: &str) -> String {
    if choice != "edge-first" {
        return choice.to_string();
    }
    let failed = app.state::<VoiceManager>().edge_failed.load(Ordering::SeqCst);
    let near_limit = edge_usage(app).is_some_and(|(used, _)| used >= EDGE_LIMIT_SECS - EDGE_MARGIN_SECS);
    if failed || near_limit {
        "chrome".into()
    } else {
        "edge".into()
    }
}

/// 起動したEdge。Windowsでは、起動するたびに専用のJobを作ってその中で動かし、
/// Job単位で見張る・閉じる。Edgeは起動したプロセスがすぐ別のプロセスを立ち上げて
/// 終わることがある(2026-10のEdgeの更新以降、本人のPCで発生)ため、最初の
/// プロセスだけを見ていると「閉じられた」と勘違いして、設定を保存するたびに
/// 窓を開き直してしまう。Jobの中のプロセスは、後から立ち上がった分も含めて
/// まとめて数えられ、まとめて終わらせられる。JobはKILL_ON_JOB_CLOSEなので、
/// ReaCastが(落ちた時も含めて)終わればEdgeも終わる。
/// CreateProcessWを直接使うのは、Jobに入れてから動かすため(止めた状態で起動する)。
#[cfg(target_os = "windows")]
pub(crate) struct EdgeProcess {
    job: isize,
}

#[cfg(target_os = "windows")]
impl EdgeProcess {
    pub(crate) fn spawn(exe: &std::path::Path, args: &[String]) -> Result<Self, String> {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows::Win32::System::Threading::{
            CreateProcessW, ResumeThread, CREATE_SUSPENDED, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
        };
        // 引数は全部""で囲む(パスに空白が入っていても大丈夫なように)
        let line = std::iter::once(exe.display().to_string())
            .chain(args.iter().cloned())
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let mut wide: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
        let job = unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|e| format!("ブラウザを起動できませんでした: {e}"))?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job
        };
        let me = Self { job: job.0 as isize };
        const SW_SHOWMINNOACTIVE: u16 = 7;
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: SW_SHOWMINNOACTIVE,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(wide.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
            .map_err(|e| format!("ブラウザを起動できませんでした: {e}"))?;
            // Edgeが子プロセスを作る前にJobに入れておく
            let _ = AssignProcessToJobObject(job, pi.hProcess);
            ResumeThread(pi.hThread);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }
        // Edgeは起動時の「最小化」の指定を無視して前に出てくるので、窓が出たら最小化する
        let raw = me.job;
        minimize_window_when_shown(move |pid| process_in_job(pid, raw));
        Ok(me)
    }

    /// 最小化している窓を元に戻して前に出す(翻訳の窓の「準備」ボタンを押してもらう時)
    pub(crate) fn show(&self) {
        let raw = self.job;
        show_window_when_shown(move |pid| process_in_job(pid, raw), true);
    }

    fn handle(&self) -> windows::Win32::Foundation::HANDLE {
        windows::Win32::Foundation::HANDLE(self.job as *mut core::ffi::c_void)
    }

    /// Jobの中にまだ動いているプロセスがあるか(窓が閉じられたら0になる)
    pub(crate) fn is_running(&self) -> bool {
        use windows::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let ok = unsafe {
            QueryInformationJobObject(
                Some(self.handle()),
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
        };
        ok.is_ok() && info.ActiveProcesses > 0
    }

    pub(crate) fn kill(&self) {
        unsafe {
            let _ = windows::Win32::System::JobObjects::TerminateJobObject(self.handle(), 0);
        }
    }
}

/// そのプロセスがJobに入っているか
#[cfg(target_os = "windows")]
fn process_in_job(pid: u32, job: isize) -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::IsProcessInJob;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut result = windows::core::BOOL(0);
        let ok = IsProcessInJob(h, Some(HANDLE(job as *mut core::ffi::c_void)), &mut result).is_ok();
        let _ = CloseHandle(h);
        ok && result.as_bool()
    }
}

/// 起動したソフト(そのプロセスID)の窓が出たら最小化する(読み上げソフトの自動起動用)。
#[cfg(target_os = "windows")]
pub(crate) fn minimize_when_shown(pid: u32) {
    minimize_window_when_shown(move |p| p == pid);
}

/// 条件に合うプロセスの窓が出たら、すぐ最小化する(最小化しても聞き取りは
/// 続くことを確認済み)。最大10秒待つ。
#[cfg(target_os = "windows")]
fn minimize_window_when_shown(matches: impl Fn(u32) -> bool + Send + 'static) {
    show_window_when_shown(matches, false);
}

/// 条件に合うプロセスの窓が出たら、最小化する(`restore`なら元に戻して前に出す)。最大10秒待つ。
#[cfg(target_os = "windows")]
fn show_window_when_shown(matches: impl Fn(u32) -> bool + Send + 'static, restore: bool) {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_MINIMIZE,
        SW_RESTORE,
    };
    struct Search<'a> {
        matches: &'a dyn Fn(u32) -> bool,
        found: isize,
    }
    std::thread::spawn(move || {
        unsafe extern "system" fn find(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let search = &mut *(lparam.0 as *mut Search);
            if !IsWindowVisible(hwnd).as_bool() {
                return true.into();
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if (search.matches)(pid) {
                search.found = hwnd.0 as isize;
                return false.into();
            }
            true.into()
        }
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let mut search = Search { matches: &matches, found: 0 };
            unsafe {
                let _ = EnumWindows(Some(find), LPARAM(&mut search as *mut Search as isize));
            }
            if search.found != 0 {
                let hwnd = HWND(search.found as *mut core::ffi::c_void);
                unsafe {
                    if restore {
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                        let _ = SetForegroundWindow(hwnd);
                    } else {
                        let _ = ShowWindow(hwnd, SW_MINIMIZE);
                    }
                }
                return;
            }
        }
    });
}

#[cfg(target_os = "windows")]
impl Drop for EdgeProcess {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSEなので、閉じればJobの中のEdgeも終わる
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle());
        }
    }
}

// Windows以外(開発用のビルドだけ)は普通に起動する
#[cfg(not(target_os = "windows"))]
pub(crate) struct EdgeProcess(std::sync::Mutex<std::process::Child>);

#[cfg(not(target_os = "windows"))]
impl EdgeProcess {
    pub(crate) fn spawn(exe: &std::path::Path, args: &[String]) -> Result<Self, String> {
        std::process::Command::new(exe)
            .args(args)
            .spawn()
            .map(|c| Self(std::sync::Mutex::new(c)))
            .map_err(|e| format!("ブラウザを起動できませんでした: {e}"))
    }
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.0.lock().unwrap().try_wait(), Ok(None))
    }
    pub(crate) fn show(&self) {}
    pub(crate) fn kill(&self) {
        let _ = self.0.lock().unwrap().kill();
    }
}

impl VoiceManager {
    /// 音声認識の画面から最後に届いた状態(状態, 説明)
    pub fn status_message(&self) -> (String, String) {
        let v = self.status.lock().unwrap();
        let state = v["state"].as_str().unwrap_or("wait").to_string();
        let message = v["message"].as_str().unwrap_or("音声認識の窓を開いています…").to_string();
        (state, message)
    }
}

/// 音声認識に使うブラウザ: (exeの場所, 名前, プロフィールのフォルダ名の後ろ半分("chrome"/"edge"))。
/// Chromeがあれば優先する。Edgeの音声認識は、プロフィールごとに使った時間を記録して
/// (Local Stateのspeech_recognition_limiter)、約10時間で使えなくなる(エラー「network」
/// になる。本人のPCで2026-10-18に発生)ため。Chromeが無ければEdge。
/// `choice`は設定のvoice_browser("chrome"/"edge"なら、それが無い時だけもう一方)。
fn browser_path(choice: &str) -> Option<(PathBuf, &'static str, &'static str)> {
    let chrome = (r"Google\Chrome\Application\chrome.exe", "Chrome", "chrome");
    let edge = (r"Microsoft\Edge\Application\msedge.exe", "Edge", "edge");
    let browsers = if choice == "edge" { [edge, chrome] } else { [chrome, edge] };
    for (rel, name, profile) in browsers {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                let p = PathBuf::from(base).join(rel);
                if p.exists() {
                    return Some((p, name, profile));
                }
            }
        }
    }
    None
}

fn input_url(s: &CommentSettings) -> String {
    let lang: String = s.voice_lang.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    // マイク・声の増幅・自動音量調整(音声認識の画面がChromeの時だけ使う)
    let gain = if s.voice_gain.is_finite() { s.voice_gain.clamp(1.0, 4.0) } else { 1.0 };
    format!(
        "http://127.0.0.1:{OBS_HTTP_PORT}/voice_input.html?lang={lang}&mic={}&gain={gain}&agc={}",
        encode_query(s.voice_mic.trim()),
        u8::from(s.voice_agc),
    )
}

/// URLの「?」の後ろに入れられるようにする(マイクの名前には空白・括弧・日本語が入る)
fn encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn set_status(app: &AppHandle, state: &str, message: impl Into<String>) {
    let v = serde_json::json!({ "state": state, "message": message.into() });
    {
        let manager = app.state::<VoiceManager>();
        let mut cur = manager.status.lock().unwrap();
        if *cur != v {
            log::info!("話した字幕: {} {}", state, v["message"].as_str().unwrap_or_default());
        }
        *cur = v.clone();
    }
    let _ = app.emit("voice:status", v);
}

fn launch(app: &AppHandle, url: &str, choice: &str) -> Result<EdgeProcess, String> {
    let (child, name) = launch_app_window(
        app,
        url,
        choice,
        "voice",
        // このプロフィールの中だけ、マイクの許可を自動で出す(開くのはReaCastの画面だけ)。
        // 声を大きくする処理(Web Audio)を、ボタンを押さなくても動かせるようにする
        &["--use-fake-ui-for-media-stream", "--autoplay-policy=no-user-gesture-required"],
    )?;
    log::info!("話した字幕: {name}で開きました");
    *app.state::<VoiceManager>().browser_name.lock().unwrap() = name.to_string();
    Ok(child)
}

/// ReaCastの画面(OBS用のHTTPで配っているページ)を、Chrome/Edgeの小さな窓(--app)で
/// 最小化して開く。プロフィールは用途ごとに分ける(`{purpose}-chrome`など)。
/// 話した字幕の音声認識と、PCの中での翻訳(local_translate.rs)で使う。(窓, ブラウザの名前)を返す
pub(crate) fn launch_app_window(
    app: &AppHandle,
    url: &str,
    choice: &str,
    purpose: &str,
    extra_args: &[&str],
) -> Result<(EdgeProcess, &'static str), String> {
    let (exe, name, kind) = browser_path(choice).ok_or("Google ChromeもMicrosoft Edgeも見つかりません")?;
    let profile = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join(format!("{purpose}-{kind}"));
    let mut args: Vec<String> = vec![
        format!("--app={url}"),
        format!("--user-data-dir={}", profile.display()),
        "--window-size=460,260".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        // 最小化しても動き続けるように、裏に回った時の抑制を止める
        "--disable-background-timer-throttling".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-backgrounding-occluded-windows".into(),
    ];
    args.extend(extra_args.iter().map(|a| a.to_string()));
    Ok((EdgeProcess::spawn(&exe, &args)?, name))
}

/// 設定に合わせて、Edgeの音声認識の窓を開く・閉じる・開き直す。
pub fn sync(app: &AppHandle) {
    let settings = app.state::<Arc<CommentEngine>>().settings();
    let manager = app.state::<VoiceManager>();
    let mut edge = manager.edge.lock().unwrap();
    // 窓が閉じられていたら、終わったものとして扱う
    if edge.as_ref().is_some_and(|(p, _)| !p.is_running()) {
        *edge = None;
    }
    // ブラウザの選択もURLに入れておく(変えたら開き直す。画面側は使わない)。
    // 「Edge優先」は、実際に開くブラウザ(Edge/Chrome)を入れる(切り替わったら開き直す)
    let browser = effective_browser(app, &settings.voice_browser);
    let want = settings.voice_enabled.then(|| format!("{}&browser={}", input_url(&settings), browser));
    // 同じ設定で開いているなら何もしない
    if matches!((&*edge, &want), (Some((_, url)), Some(w)) if url == w) {
        return;
    }
    if let Some((p, _)) = edge.take() {
        p.kill();
    }
    drop(edge);
    match want {
        // マイクを止めている時は、画面を最初から止めた状態で開く(開いた瞬間に一瞬だけ
        // 聞き取ってしまわないように)。開いている窓と比べるURLには入れない(マイクを
        // 止める・再開するたびに窓を開き直さないように)
        Some(url) => match launch(
            app,
            &if manager.muted.load(Ordering::SeqCst) { format!("{url}&muted=1") } else { url.clone() },
            &browser,
        ) {
            Ok(child) => {
                log::info!("話した字幕: 音声認識の窓を開きました");
                set_status(app, "wait", "音声認識の窓を開いています…");
                *manager.edge.lock().unwrap() = Some((child, url));
            }
            Err(e) => {
                log::warn!("話した字幕: {e}");
                set_status(app, "error", e);
            }
        },
        None => set_status(app, "off", "使っていません"),
    }
}

/// マイクを止めているか(設定ファイルに覚えておく)
const MUTED_KEY: &str = "voiceMuted";

/// 起動した時に、前回マイクを止めたまま終わっていたら、止めた状態から始める
/// (音声認識の窓を開く前に呼ぶ)
pub fn restore_muted(app: &AppHandle) {
    let muted = app
        .state::<crate::config_store::ConfigStore>()
        .get(MUTED_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if muted {
        log::info!("話した字幕: 前回マイクを止めたまま終わったので、止めた状態から始めます");
        app.state::<VoiceManager>().muted.store(true, Ordering::SeqCst);
        set_status(app, "muted", "マイクを止めています(聞き取っていません)");
    }
}

/// 音声認識の画面に、聞き取りを止める/続けるを伝える
fn send_control(app: &AppHandle, muted: bool) {
    app.state::<ObsBridge>()
        .push_voice(serde_json::json!({ "type": "voiceControl", "listening": !muted }));
}

/// マイクを止める/再開する(設定画面のボタン)。
pub fn toggle_mute(app: &AppHandle) -> bool {
    let muted = app.state::<VoiceManager>().muted.load(Ordering::SeqCst);
    set_muted(app, !muted)
}

/// マイクを止める(true)/再開する(false)。電話・離席の時用(ホットキーは止める・
/// 再開が別々のキー)。止めている間はEdgeの聞き取り自体を止めるので、声が
/// サーバーにも送られない。すでにその状態なら何もしない。
pub fn set_muted(app: &AppHandle, muted: bool) -> bool {
    let manager = app.state::<VoiceManager>();
    if manager.muted.swap(muted, Ordering::SeqCst) == muted {
        return muted;
    }
    log::info!("話した字幕: マイクを{}", if muted { "止めました" } else { "再開しました" });
    // 次にReaCastを起動した時も同じ状態にする(止めたまま終わったら、止めたまま始まる)
    let _ = app.state::<crate::config_store::ConfigStore>().set(MUTED_KEY.into(), serde_json::json!(muted));
    send_control(app, muted);
    if muted {
        // 表示中の字幕も消す
        app.state::<ObsBridge>().push_voice(serde_json::json!({ "type": "voice", "clear": true }));
        set_status(app, "muted", "マイクを止めています(聞き取っていません)");
    } else {
        set_status(app, "wait", "マイクを再開しています…");
    }
    let _ = app.emit("voice:muted", muted);
    muted
}

/// 音声認識の窓を閉じてから開き直す(設定画面の「開き直す」)。
pub fn reopen(app: &AppHandle) {
    if let Some((p, _)) = app.state::<VoiceManager>().edge.lock().unwrap().take() {
        p.kill();
    }
    sync(app);
}

/// ReaCastの終了時に、開いたEdgeも閉じる。
pub fn stop(app: &AppHandle) {
    if let Some(manager) = app.try_state::<VoiceManager>() {
        if let Some((p, _)) = manager.edge.lock().unwrap().take() {
            p.kill();
        }
    }
}

/// Edgeの窓が今も開いているか(配信前チェック用)
pub fn is_running(app: &AppHandle) -> bool {
    let manager = app.state::<VoiceManager>();
    let edge = manager.edge.lock().unwrap();
    edge.as_ref().is_some_and(|(p, _)| p.is_running())
}

/// Google翻訳(非公式の方法)の応答から訳文を取り出す。
/// 形: [[["訳文1","原文1",...],["訳文2","原文2",...]],...]
fn parse_translation(v: &serde_json::Value) -> Option<String> {
    let parts = v.get(0)?.as_array()?;
    let text: String = parts.iter().filter_map(|p| p.get(0).and_then(|t| t.as_str())).collect();
    (!text.trim().is_empty()).then_some(text)
}

async fn translate(client: &reqwest::Client, text: &str, source: Option<&str>, target: &str) -> Result<String, String> {
    translate_detect(client, text, source, target).await.map(|(t, _)| t)
}

/// 翻訳を休んでいる間に返すエラー(呼び出し側は、これはログに残さない)
pub(crate) const TRANSLATE_PAUSED: &str = "翻訳を休んでいます";
/// 訳さなかった(短すぎる文・「草」「w」だけ等)時に返すエラー(これもログに残さない)
pub(crate) const TRANSLATE_SKIPPED: &str = "翻訳しませんでした";

/// ログに残さなくていい翻訳のエラーか
pub(crate) fn is_quiet_translate_error(e: &str) -> bool {
    e == TRANSLATE_PAUSED || e == TRANSLATE_SKIPPED
}

/// 訳さない文: 記号・空白・「w」「草」「笑」を除いて2文字以下(「草」「おつ」を
/// 訳すと「Grass」のような変な訳になる。あいさつ程度の短い文は訳さなくても伝わる)
fn skip_translation(text: &str) -> bool {
    let meaningful = text
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !matches!(c, 'w' | 'W' | 'ｗ' | 'Ｗ' | '草' | '笑'))
        .filter(|c| !matches!(*c as u32, 0x3000..=0x303F | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0x2000..=0x206F))
        .count();
    meaningful <= 2
}

/// Google翻訳(非公式の方法)に断られない(HTTP 429)ための決まり(以下はGoogleに頼む時だけ):
/// ・頼むのは1つずつ、前に頼んでから最低MIN_GAPあける(話した字幕と下の字幕の翻訳の合計)
/// ・断られたら、しばらく(1分→2分→4分…最長15分)頼みに行かない。断られてもすぐ頼み
///   直していたので、止められたまま戻らなくなっていた(2026-10-28、本人のPCで1日中429)
/// ・同じ文の翻訳は覚えておいて、もう一度頼まない(「草」「おつ」など)
struct TranslateGate {
    last: Option<std::time::Instant>,
    paused_until: Option<std::time::Instant>,
    strikes: u32,
    cache: std::collections::HashMap<(String, String), (String, String)>,
}
const TRANSLATE_MIN_GAP: std::time::Duration = std::time::Duration::from_millis(1000);
const TRANSLATE_CACHE_MAX: usize = 300;

fn translate_gate() -> &'static tokio::sync::Mutex<TranslateGate> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<TranslateGate>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| {
        tokio::sync::Mutex::new(TranslateGate { last: None, paused_until: None, strikes: 0, cache: Default::default() })
    })
}

/// 断られた回数から、休む時間を決める(1分→2分→4分→8分→15分)
fn translate_pause(strikes: u32) -> std::time::Duration {
    std::time::Duration::from_secs((60u64 << strikes.saturating_sub(1).min(4)).min(15 * 60))
}

/// 翻訳して、(訳文, 判定した元の言語)を返す。下の字幕のレスの翻訳(comments)でも使う。
/// `source`は分かっていれば元の言語(話した字幕の聞き取る言葉など。Noneなら自動で判定)。
/// 設定が「PCの中で翻訳」なら、まずChrome/Edgeの中で訳す(local_translate.rs。回数の
/// 上限が無い)。使えない時(準備中・その言語に未対応など)だけGoogle翻訳(非公式の方法)。
pub(crate) async fn translate_detect(
    client: &reqwest::Client,
    text: &str,
    source: Option<&str>,
    target: &str,
) -> Result<(String, String), String> {
    if skip_translation(text) {
        return Err(TRANSLATE_SKIPPED.into());
    }
    if let Some(v) = crate::local_translate::translate(text, source, target).await {
        return Ok(v);
    }
    let key = (target.to_string(), text.to_string());
    // 1つずつ頼む(待っている間はこのロックを持ったまま)
    let mut gate = translate_gate().lock().await;
    if let Some(hit) = gate.cache.get(&key) {
        return Ok(hit.clone());
    }
    let now = std::time::Instant::now();
    if gate.paused_until.is_some_and(|t| t > now) {
        return Err(TRANSLATE_PAUSED.into());
    }
    if let Some(last) = gate.last {
        let next = last + TRANSLATE_MIN_GAP;
        if next > now {
            tokio::time::sleep(next - now).await;
        }
    }
    gate.last = Some(std::time::Instant::now());
    let result = request_translation(client, text, target).await;
    match &result {
        Ok(v) => {
            gate.strikes = 0;
            if gate.cache.len() >= TRANSLATE_CACHE_MAX {
                gate.cache.clear();
            }
            gate.cache.insert(key, v.clone());
        }
        Err(e) if e.contains("429") => {
            gate.strikes += 1;
            let pause = translate_pause(gate.strikes);
            gate.paused_until = Some(std::time::Instant::now() + pause);
            log::warn!("Google翻訳に断られたので(HTTP 429)、翻訳を{}分休みます", pause.as_secs() / 60);
        }
        Err(_) => {}
    }
    result
}

async fn request_translation(client: &reqwest::Client, text: &str, target: &str) -> Result<(String, String), String> {
    let resp = client
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[("client", "gtx"), ("sl", "auto"), ("tl", target), ("dt", "t"), ("q", text)])
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| format!("翻訳できませんでした: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("翻訳できませんでした(HTTP {})", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("翻訳できませんでした: {e}"))?;
    let text = parse_translation(&v).ok_or("翻訳の結果が空でした")?;
    let source = v.get(2).and_then(|s| s.as_str()).unwrap_or_default().to_string();
    Ok((text, source))
}

/// 音声認識の画面から届くメッセージを受け取り続ける(起動時に1回だけ呼ぶ)。
pub fn start_listener(app: AppHandle) {
    let Some(mut rx) = app.state::<ObsBridge>().take_voice_inbound() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        // 話し始めるたびに番号を進める(途中の文字と、確定した文・翻訳を結び付ける)
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let mut current_id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let client = reqwest::Client::new();
        while let Some(msg) = rx.recv().await {
            // PCの中で翻訳する窓(translate_worker.html)からの結果・状態
            if matches!(msg["type"].as_str(), Some("translateResult") | Some("translatorStatus")) {
                crate::local_translate::on_message(&app, &msg);
                continue;
            }
            let muted = app.state::<VoiceManager>().muted.load(Ordering::SeqCst);
            // 音声認識の画面からの、マイクの一覧と、今の声の大きさ(設定画面のメーター)
            if msg["type"] == "voiceDevices" {
                let list: Vec<String> = msg["devices"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                *app.state::<VoiceManager>().devices.lock().unwrap() = list.clone();
                let _ = app.emit("voice:devices", list);
                continue;
            }
            if msg["type"] == "voiceLevel" {
                if !muted {
                    let _ = app.emit("voice:level", msg["level"].as_f64().unwrap_or(0.0));
                }
                continue;
            }
            if msg["type"] == "voiceStatus" {
                let state = msg["state"].as_str().unwrap_or("wait");
                // 止めている間に画面が繋ぎ直してきたら(Edgeの再起動など)、改めて止めてもらう
                if muted && state == "listening" {
                    send_control(&app, true);
                    continue;
                }
                let manager = app.state::<VoiceManager>();
                let browser = manager.browser_name.lock().unwrap().clone();
                // 「Edge優先」で、Edgeの音声認識が使えなくなった(上限に達すると、聞き取りを
                // 始めるたびに「network」になる)→Chromeに切り替える
                if state == "error"
                    && browser == "Edge"
                    && app.state::<Arc<CommentEngine>>().settings().voice_browser == "edge-first"
                    && !manager.edge_failed.swap(true, Ordering::SeqCst)
                {
                    log::info!("話した字幕: Edgeの音声認識が使えないので、Chromeに切り替えます({})", msg["message"].as_str().unwrap_or_default());
                    set_status(&app, "wait", "Edgeの音声認識が使えなくなったので、Chromeに切り替えています…");
                    sync(&app);
                    continue;
                }
                let message = msg["message"].as_str().unwrap_or_default();
                // 聞き取り中は、どちらのブラウザで聞き取っているかも出す
                if state == "listening" && !browser.is_empty() {
                    set_status(&app, state, format!("{message}・{browser}"));
                } else {
                    set_status(&app, state, message);
                }
                continue;
            }
            if muted {
                continue;
            }
            let settings = app.state::<Arc<CommentEngine>>().settings();
            if !settings.voice_enabled {
                continue;
            }
            let text = msg["text"].as_str().unwrap_or_default().trim().to_string();
            if text.is_empty() {
                continue;
            }
            let is_final = msg["final"].as_bool().unwrap_or(false);
            let bridge = app.state::<ObsBridge>().inner().clone();
            bridge.push_voice(serde_json::json!({ "type": "voice", "id": current_id, "original": text, "final": is_final }));
            if !is_final {
                continue;
            }
            let id = current_id;
            current_id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            // 話した言葉がルールに当てはまったら、コメントと同じようにリアクションを飛ばす
            if settings.voice_reaction_link {
                for rid in app.state::<Arc<CommentEngine>>().matched_reactions(&text) {
                    if let Err(e) = crate::show_reaction(&app, &rid, "voice") {
                        log::warn!("話した言葉からのリアクションを表示できませんでした: {e}");
                    }
                }
            }
            if settings.voice_translate && !settings.voice_target_lang.trim().is_empty() {
                let client = client.clone();
                let target = settings.voice_target_lang.clone();
                // 聞き取る言葉("ja-JP")の前半が元の言語
                let source = settings.voice_lang.split('-').next().unwrap_or_default().to_string();
                tauri::async_runtime::spawn(async move {
                    match translate(&client, &text, Some(&source).filter(|s| !s.is_empty()).map(|s| s.as_str()), &target).await {
                        Ok(t) => bridge.push_voice(serde_json::json!({ "type": "voice", "id": id, "translation": t })),
                        Err(e) if is_quiet_translate_error(&e) => {}
                        Err(e) => log::warn!("話した字幕: {e}"),
                    }
                });
            }
        }
    });
}

#[tauri::command]
pub fn voice_status(manager: tauri::State<VoiceManager>) -> serde_json::Value {
    let mut v = manager.status.lock().unwrap().clone();
    v["muted"] = serde_json::json!(manager.muted.load(Ordering::SeqCst));
    v["devices"] = serde_json::json!(*manager.devices.lock().unwrap());
    v
}

#[tauri::command]
pub fn voice_toggle_mute(app: AppHandle) -> bool {
    toggle_mute(&app)
}

#[tauri::command(async)]
pub fn voice_reopen(app: AppHandle) {
    // 開き直す時は、「Edge優先」ならもう一度Edgeを試す
    app.state::<VoiceManager>().edge_failed.store(false, Ordering::SeqCst);
    reopen(&app);
}

/// Edgeの音声認識の使用時間(設定画面に出す)。{usedSecs, limitSecs, checkpointMs}。読めなければnull
#[tauri::command(async)]
pub fn voice_edge_usage(app: AppHandle) -> Option<serde_json::Value> {
    let (used, checkpoint) = edge_usage(&app)?;
    Some(serde_json::json!({ "usedSecs": used, "limitSecs": EDGE_LIMIT_SECS, "checkpointMs": checkpoint }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_mic_name_for_url() {
        assert_eq!(encode_query("マイク (USB) A&B"), "%E3%83%9E%E3%82%A4%E3%82%AF%20%28USB%29%20A%26B");
        assert_eq!(encode_query(""), "");
    }

    #[test]
    fn skips_short_slang() {
        for s in ["草", "w", "wwwww", "おつ", "草草", "ｗｗｗ", "笑笑", "!?", "ok"] {
            assert!(skip_translation(s), "{s}");
        }
        for s in ["おはよう", "Hello", "このボス強すぎない？", "草生える"] {
            assert!(!skip_translation(s), "{s}");
        }
    }

    #[test]
    fn translate_pause_grows_and_caps() {
        let m = |n| translate_pause(n).as_secs() / 60;
        assert_eq!((m(1), m(2), m(3), m(4), m(5), m(9)), (1, 2, 4, 8, 15, 15));
    }

    #[test]
    fn parses_google_translation_response() {
        let v = serde_json::json!([[["Hello. ", "こんにちは。", null, null, 10], ["Thank you", "ありがとう", null, null, 10]], null, "ja"]);
        assert_eq!(parse_translation(&v).as_deref(), Some("Hello. Thank you"));
        assert_eq!(parse_translation(&serde_json::json!([[], null, "ja"])), None);
    }

    #[test]
    #[ignore] // 実際にGoogle翻訳に繋ぐ(cargo test -- --ignored)
    fn translates_with_google() {
        // Google翻訳だけを試す(translate()はPCの中での翻訳=Tauriの窓まで繋がり、テストの
        // exeに入らない部品(comctl32 v6)まで入ってテストが起動しなくなるため)
        let (t, _) = tauri::async_runtime::block_on(request_translation(&reqwest::Client::new(), "こんにちは、今日もよろしくお願いします", "en"))
            .unwrap();
        eprintln!("{t}");
        assert!(!t.is_empty());
    }
}

#[cfg(all(test, target_os = "windows"))]
mod job_tests {
    use super::*;

    /// 起動したプロセスが別のプロセスを立ち上げてすぐ終わっても(Edgeの起動と同じ)、
    /// 動いていると分かり、まとめて終わらせられる
    #[test]
    fn tracks_relaunched_children_via_job() {
        let p = EdgeProcess::spawn(
            std::path::Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            &[
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Process ping -ArgumentList '-n','30','127.0.0.1' -WindowStyle Hidden".into(),
            ],
        )
        .unwrap();
        // PowerShellはすぐ終わるが、pingが残っている
        std::thread::sleep(std::time::Duration::from_millis(4000));
        assert!(p.is_running());
        p.kill();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!p.is_running());
    }
}
