//! 管理者の権限で動く「キーの見張り係」(ホットキー用)。
//!
//! Windowsの決まり(UIPI)で、管理者として動いているアプリ(管理者で起動したOBS・
//! ゲーム・専ブラ等)が前面にある間は、普通の権限のアプリにはキーボードの入力が
//! 一切届かない(低レベルフックも、RegisterHotKeyのホットキーも。2026-10-19に
//! 本人のPCで確認)。そこで、同じexeを「見張り係」として管理者の権限で起動し、
//! 割り当てたキーが押されたら、PCの中だけの通信(127.0.0.1)でアプリ本体に知らせる。
//!
//! - 管理者の権限での起動は、タスクスケジューラーのタスク(「最上位の特権で実行」)
//!   で行う。タスクの登録の時だけWindowsの確認(UAC)が出て、以降はアプリ本体
//!   (普通の権限)から`schtasks /run`で確認なしに起動できる。
//! - 本体は127.0.0.1の空いているポートで待ち、ポートと合言葉をデータフォルダの
//!   key-watcher.jsonに書く。見張り係はそれを読んで繋ぎ、本体から割り当て(キーの
//!   一覧)を受け取って、一致したキーの番号だけを送り返す(押したキーを全部送る
//!   わけではない)。本体が終わって接続が切れたら、見張り係も終わる。
//! - 見張り係と繋がっている間は、本体自身のキーボードの見張りでは反応しない
//!   (二重に動かないように)。

use crate::hotkeys as km;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// アプリごとの違い(ReaTapでは "ReaTap" / "com.aoh.reatap")
const APP_NAME: &str = "ReaTap";
const IDENTIFIER: &str = "com.aoh.reatap";
const TASK_NAME: &str = "ReaTap\\KeyWatcher";
/// 見張り係として起動する時の引数
pub const CLI_FLAG: &str = "--key-watcher";
/// 割り当ての最大数(見張り係に何でも見張らせられないように)
const MAX_BINDINGS: usize = 16;

fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join(IDENTIFIER))
}

fn info_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("key-watcher.json"))
}

// ---------------- 本体側 ----------------

static CONNECTED: AtomicBool = AtomicBool::new(false);

struct Server {
    /// 今繋がっている見張り係(へ書き込む側)
    writer: Option<TcpStream>,
    /// 今の割り当て(キーの書き方)と、キーの割り当て中で止めているか
    bindings: Vec<String>,
    suspended: bool,
}

fn server() -> &'static Mutex<Server> {
    static S: OnceLock<Mutex<Server>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Server { writer: None, bindings: Vec::new(), suspended: false }))
}

/// 見張り係と繋がっているか(繋がっている間は本体のキーボードの見張りでは反応しない)
pub fn connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}

fn bindings_line(s: &Server) -> String {
    serde_json::json!({ "bindings": s.bindings, "suspended": s.suspended }).to_string() + "\n"
}

/// 割り当てを見張り係に伝える(繋がっていなければ、繋がった時に送る)。
pub fn set_bindings(bindings: Vec<String>, suspended: bool) {
    let mut s = server().lock().unwrap();
    s.bindings = bindings.into_iter().take(MAX_BINDINGS).collect();
    s.suspended = suspended;
    let line = bindings_line(&s);
    if let Some(w) = s.writer.as_mut() {
        let _ = w.write_all(line.as_bytes());
    }
}

/// 設定画面でキーを割り当てている間は、見張り係でも反応しないようにする。
pub fn set_suspended() {
    let (bindings, _) = {
        let s = server().lock().unwrap();
        (s.bindings.clone(), s.suspended)
    };
    set_bindings(bindings, true);
}

/// 見張り係を待ち受ける。`on_fire`は割り当ての番号(set_bindingsの順番)で呼ばれる。
pub fn start_server(on_fire: fn(usize)) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => return log::warn!("キーの見張り係: 待ち受けを始められませんでした: {e}"),
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        // 合言葉(同じPCの他のプログラムが本体のふりをしないように。ReaTapにはrandが無いのでsha2で)
        let token: String = {
            use sha2::{Digest, Sha256};
            let seed = format!("{:?}{}{:p}{port}", std::time::SystemTime::now(), std::process::id(), &listener);
            format!("{:x}", Sha256::digest(seed.as_bytes()))[..32].to_string()
        };
        if let Some(p) = info_path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            if let Err(e) = std::fs::write(&p, serde_json::json!({ "port": port, "token": token }).to_string()) {
                log::warn!("キーの見張り係: 接続先を書き込めませんでした: {e}");
            }
        }
        for stream in listener.incoming().flatten() {
            let token = token.clone();
            std::thread::spawn(move || serve(stream, &token, on_fire));
        }
    });
}

fn serve(stream: TcpStream, token: &str, on_fire: fn(usize)) {
    // 最初の1行は合言葉
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let hello: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
    if hello["hello"].as_str() != Some(token) {
        return;
    }
    // 合言葉を待つ間だけの時間制限を外す(読む側の複製にも。外し忘れると、5秒キーが
    // 押されないだけで切れてしまう=v1.5.18の不具合)
    let _ = stream.set_read_timeout(None);
    let _ = reader.get_ref().set_read_timeout(None);
    let id = {
        let mut s = server().lock().unwrap();
        let mut w = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let _ = w.write_all(bindings_line(&s).as_bytes());
        // 前の見張り係が残っていたら、新しい方に切り替える
        if let Some(old) = s.writer.replace(w) {
            let _ = old.shutdown(std::net::Shutdown::Both);
        }
        stream.local_addr().ok().zip(stream.peer_addr().ok())
    };
    CONNECTED.store(true, Ordering::SeqCst);
    log::info!("キーの見張り係(管理者): 繋がりました。管理者のアプリが前面でもホットキーが効きます");
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            // 時間切れ(念のため)は切断ではないので待ち続ける
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => break,
            Ok(_) => {
                let msg: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
                if let Some(i) = msg["fire"].as_u64() {
                    on_fire(i as usize);
                }
            }
        }
    }
    // この接続が今の見張り係なら、切れたことにする
    let mut s = server().lock().unwrap();
    let current = s.writer.as_ref().and_then(|w| w.local_addr().ok().zip(w.peer_addr().ok()));
    if current.is_none() || current == id {
        s.writer = None;
        CONNECTED.store(false, Ordering::SeqCst);
        log::info!("キーの見張り係(管理者): 接続が切れました");
    }
}

// ---------------- タスクスケジューラー ----------------

fn hidden_command(exe: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 登録済みのタスクの状態: "none"(未登録) / "installed" / "stale"(別の場所のexeを指している)
pub fn task_state() -> &'static str {
    let Ok(out) = hidden_command("schtasks.exe").args(["/query", "/tn", TASK_NAME, "/xml"]).output() else {
        return "none";
    };
    if !out.status.success() {
        return "none";
    }
    // schtasksの出力はコンソールの文字コード。exeの場所(ASCII以外を含むことがある)は
    // UTF-16でもShift_JISでも読めるよう、両方で比べる
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.decode(&out.stdout);
    let exe = std::env::current_exe().map(|p| xml_escape(&p.display().to_string()).to_lowercase()).unwrap_or_default();
    if !exe.is_empty() && (text.contains(&exe) || sjis.to_lowercase().contains(&exe)) {
        "installed"
    } else {
        "stale"
    }
}

/// 管理者の権限でschtasksを動かす(Windowsの確認が出る)。終わるまで待って成否を返す。
#[cfg(windows)]
fn run_schtasks_elevated(args: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("schtasks.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| {
            if e.code().0 as u32 == 0x800704C7 {
                "Windowsの確認で「いいえ」が選ばれました".to_string()
            } else {
                format!("管理者の権限で実行できませんでした: {e}")
            }
        })?;
        if info.hProcess.is_invalid() {
            return Ok(());
        }
        let waited = WaitForSingleObject(info.hProcess, 30_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err("タスクの登録が終わりませんでした".into());
        }
        if code != 0 {
            return Err(format!("タスクの登録に失敗しました(終了コード {code})"));
        }
    }
    Ok(())
}

/// タスクの設定(XML)。run_levelは"HighestAvailable"(管理者の権限)。
/// 電源・実行時間の既定(バッテリーでは起動しない・3日で止める・優先度低め)は外す。
fn task_xml(exe: &std::path::Path, run_level: &str) -> String {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{app}: 管理者として動いているアプリ(OBSなど)が前面でも、ホットキーを効かせるためのキーの見張り</Description>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>{run_level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>StopExisting</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{flag}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        app = APP_NAME,
        user = xml_escape(&user),
        exe = xml_escape(&exe.display().to_string()),
        flag = CLI_FLAG,
    )
}

/// タスクの設定をUTF-16(BOM付き。schtasksが読む形)で一時ファイルに書く。
fn write_task_xml(exe: &std::path::Path, run_level: &str) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!("{}-key-watcher-task.xml", APP_NAME.to_lowercase()));
    let mut bytes = vec![0xFF, 0xFE];
    for u in task_xml(exe, run_level).encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| format!("タスクの設定を書き込めませんでした: {e}"))?;
    Ok(path)
}

/// タスクを登録する(Windowsの確認が1回出る)。
#[cfg(windows)]
pub fn install_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = write_task_xml(&exe, "HighestAvailable")?;
    let result = run_schtasks_elevated(&format!("/create /tn \"{TASK_NAME}\" /xml \"{}\" /f", path.display()));
    let _ = std::fs::remove_file(&path);
    result
}

/// タスクを削除する(Windowsの確認が出る)。見張り係も終わらせる。
#[cfg(windows)]
pub fn uninstall_task() -> Result<(), String> {
    run_schtasks_elevated(&format!("/delete /tn \"{TASK_NAME}\" /f"))?;
    if let Some(w) = server().lock().unwrap().writer.take() {
        let _ = w.shutdown(std::net::Shutdown::Both);
    }
    Ok(())
}

/// 登録済みのタスクで見張り係を起動する(確認は出ない)。
pub fn run_task() -> Result<(), String> {
    let out = hidden_command("schtasks.exe")
        .args(["/run", "/tn", TASK_NAME])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("見張り係を起動できませんでした: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// 起動時: タスクが登録済みなら見張り係を起動する。
pub fn launch_if_installed() {
    std::thread::spawn(|| match task_state() {
        "installed" => {
            if let Err(e) = run_task() {
                log::warn!("キーの見張り係: {e}");
            }
        }
        "stale" => log::warn!(
            "キーの見張り係: 登録済みのタスクが別の場所の{APP_NAME}を指しています(exeを移動した時など)。設定画面から登録し直してください"
        ),
        _ => {}
    });
}

/// 設定画面に出す状態
pub fn status() -> serde_json::Value {
    serde_json::json!({ "task": task_state(), "connected": connected() })
}

// ---------------- 設定画面から使うコマンド ----------------

#[tauri::command(async)]
pub fn hotkey_watcher_status() -> serde_json::Value {
    status()
}

/// タスクを登録して見張り係を起動し、繋がるまで少し待つ。
#[tauri::command(async)]
pub fn hotkey_watcher_install() -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    install_task()?;
    run_task()?;
    for _ in 0..40 {
        if connected() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(status())
}

#[tauri::command(async)]
pub fn hotkey_watcher_uninstall() -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    uninstall_task()?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(status())
}

// ---------------- 見張り係(管理者の権限で動く側) ----------------

/// 見張り係として起動された時は、見張りを行ってプロセスごと終わる(戻ってこない)。
/// 通常の起動ならすぐ戻る。
pub fn run_if_requested() {
    if !std::env::args().skip(1).any(|a| a == CLI_FLAG) {
        return;
    }
    #[cfg(windows)]
    helper::run();
    std::process::exit(0);
}

#[cfg(windows)]
mod helper {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    struct Watch {
        list: Vec<Option<km::Combo>>,
        suspended: bool,
    }

    fn watch() -> &'static Mutex<Watch> {
        static W: OnceLock<Mutex<Watch>> = OnceLock::new();
        W.get_or_init(|| Mutex::new(Watch { list: Vec::new(), suspended: false }))
    }

    static FIRE: OnceLock<Mutex<mpsc::Sender<usize>>> = OnceLock::new();

    fn held() -> &'static Mutex<HashMap<String, Instant>> {
        static H: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        H.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn down(vk: VIRTUAL_KEY) -> bool {
        unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let msg = wparam.0 as u32;
            let extended = (info.flags.0 & LLKHF_EXTENDED.0) != 0;
            let key = km::vk_to_code(info.vkCode)
                .map(str::to_string)
                .or_else(|| km::scan_to_code(info.scanCode, extended));
            if let Some(key) = key {
                if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                    let now = Instant::now();
                    let first = {
                        let mut h = held().lock().unwrap();
                        let repeat = h.get(&key).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(1));
                        h.insert(key.clone(), now);
                        !repeat
                    };
                    if first {
                        let combo = km::Combo {
                            ctrl: down(VK_CONTROL),
                            alt: down(VK_MENU),
                            shift: down(VK_SHIFT),
                            meta: down(VK_LWIN) || down(VK_RWIN),
                            code: key,
                        };
                        let hit = {
                            let w = watch().lock().unwrap();
                            if w.suspended {
                                None
                            } else {
                                w.list.iter().position(|c| c.as_ref() == Some(&combo))
                            }
                        };
                        if let (Some(i), Some(tx)) = (hit, FIRE.get()) {
                            let _ = tx.lock().unwrap().send(i);
                        }
                    }
                } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                    held().lock().unwrap().remove(&key);
                }
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    pub fn run() {
        let Some(info) = info_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        let info: serde_json::Value = serde_json::from_str(&info).unwrap_or_default();
        let (Some(port), Some(token)) = (info["port"].as_u64(), info["token"].as_str()) else { return };
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port as u16)) else { return };
        let hello = serde_json::json!({ "hello": token }).to_string() + "\n";
        if stream.write_all(hello.as_bytes()).is_err() {
            return;
        }
        // 本体からの割り当てを受け取る。本体が終わって切れたら、見張り係も終わる
        let reader = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let msg: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
                if let Some(list) = msg["bindings"].as_array() {
                    let mut w = watch().lock().unwrap();
                    w.list = list.iter().take(MAX_BINDINGS).map(|k| k.as_str().and_then(km::parse)).collect();
                    w.suspended = msg["suspended"].as_bool().unwrap_or(false);
                }
            }
            std::process::exit(0);
        });
        // 一致したキーの番号を本体に送る(フックの中ではすぐ戻るため、送るのは別のスレッド)
        let (tx, rx) = mpsc::channel::<usize>();
        let _ = FIRE.set(Mutex::new(tx));
        std::thread::spawn(move || {
            for i in rx {
                let line = serde_json::json!({ "fire": i }).to_string() + "\n";
                if stream.write_all(line.as_bytes()).is_err() {
                    std::process::exit(0);
                }
            }
        });
        unsafe {
            let module = GetModuleHandleW(None).ok();
            if SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module.map(|m| m.into()), 0).is_err() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static FIRED: AtomicUsize = AtomicUsize::new(usize::MAX);

    fn on_fire(i: usize) {
        FIRED.store(i, Ordering::SeqCst);
    }

    /// 管理者の確認以外の流れを実際に試す: 普通の権限のタスク(別名)を登録 →
    /// schtasks /runで見張り係(開発用のexe)を起動 → 繋がる → Pauseキーを送る → 知らせが届く。
    /// 本体が起動中だと、その接続先(key-watcher.json)を上書きしてしまうので、本体を閉じてから(本体は次の起動で書き直す)。
    /// 先に `cargo build` しておくこと(target/debug/ReaTap.exeを見張り係として使う)。
    #[test]
    #[ignore]
    fn helper_flow_without_elevation() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        };
        let exe = std::env::current_dir().unwrap().join("target").join("debug").join("ReaTap.exe");
        assert!(exe.is_file(), "先に cargo build してください");
        start_server(on_fire);
        set_bindings(vec!["Ctrl+KeyQ".into(), "Pause".into()], false);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let name = "ReaTap\\KeyWatcherTest";
        let xml = write_task_xml(&exe, "LeastPrivilege").unwrap();
        let out = hidden_command("schtasks.exe")
            .args(["/create", "/tn", name, "/xml", &xml.display().to_string(), "/f"])
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&xml);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let run = hidden_command("schtasks.exe").args(["/run", "/tn", name]).output().unwrap();
        assert!(run.status.success());
        for _ in 0..50 {
            if connected() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let ok_connected = connected();
        // 繋がってから6秒は何も押さない(繋いだ直後の時間制限で切れないこと。v1.5.18の不具合)
        std::thread::sleep(std::time::Duration::from_secs(6));
        let still_connected = connected();
        let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0x13), wScan: 0x45, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        };
        unsafe { SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(std::time::Duration::from_millis(500));
        let fired = FIRED.load(Ordering::SeqCst);
        // 片付け: 接続を切れば見張り係は終わる。タスクも消す
        if let Some(w) = server().lock().unwrap().writer.take() {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
        let _ = hidden_command("schtasks.exe").args(["/delete", "/tn", name, "/f"]).output();
        assert!(ok_connected, "見張り係が繋がらなかった");
        assert!(still_connected, "見張り係が何も押さないうちに切れた");
        assert_eq!(fired, 1, "Pause(2番目の割り当て)の知らせが届かなかった");
    }
}
