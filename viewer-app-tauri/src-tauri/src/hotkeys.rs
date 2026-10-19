//! リアクションのホットキー(他のアプリを使っている時でも効く)。キーの書き方は
//! 設定画面(main.js)で記録した形("Ctrl+Alt+KeyS"のように、修飾キー+キーのコード)。
//!
//! 以前はWindowsのグローバルショートカット(RegisterHotKey。
//! tauri-plugin-global-shortcut)を使っていたが、その方式だと登録したキーを
//! Windowsが横取りしてしまい、そのキーで文字が打てなくなる。そこで、キーボードの
//! 入力を「見るだけ」の低レベルフック(WH_KEYBOARD_LL)に切り替えた。押された
//! キーは必ず次へ渡すので、ホットキーに割り当てたキーでも文字として入力できる。
//! キーの照合の部分はReaCast(broadcaster-app-tauri/src-tauri/src/comments/hotkeys.rs)と同じ。

#[cfg(target_os = "windows")]
use std::sync::OnceLock;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// 押された時に、どの配信者のバー(label)のどのリアクションを送るか
#[derive(Clone, PartialEq)]
struct Action {
    label: String,
    emoji_id: String,
}

/// 修飾キー(Ctrl/Alt/Shift/Win)の組み合わせとキーのコード
#[derive(Clone, PartialEq, Debug)]
struct Combo {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    code: String,
}

fn parse(key: &str) -> Option<Combo> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut combo = Combo { ctrl: false, alt: false, shift: false, meta: false, code: String::new() };
    for part in key.split('+') {
        match part {
            "Ctrl" | "Control" => combo.ctrl = true,
            "Alt" => combo.alt = true,
            "Shift" => combo.shift = true,
            "Super" | "Meta" | "Win" => combo.meta = true,
            code if !code.is_empty() && combo.code.is_empty() => combo.code = code.to_string(),
            _ => return None,
        }
    }
    (!combo.code.is_empty()).then_some(combo)
}

/// 今のホットキーの割り当て。フックの中から読むだけなので、軽く取れる形で持つ
struct Bindings {
    list: Vec<(Combo, Action)>,
    /// 設定画面でキーを割り当てている間は反応しない
    suspended: bool,
}

fn bindings() -> &'static Mutex<Bindings> {
    static B: std::sync::OnceLock<Mutex<Bindings>> = std::sync::OnceLock::new();
    B.get_or_init(|| Mutex::new(Bindings { list: Vec::new(), suspended: false }))
}

fn run_action(app: &AppHandle, action: Action) {
    // 実際の送信(連打防止など)はそのバー自身が行う(bar.jsの"hotkey-reaction")
    let _ = app.emit_to(action.label.as_str(), "hotkey-reaction", action.emoji_id);
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBinding {
    shortcut: String,
    label: String,
    emoji_id: String,
}

/// 接続中の配信者のホットキーをまとめて設定し直す(main.jsのapplyAllHotkeys)。
/// 初回はキーボードの見張りも始める。書き方が正しくないキーがあれば、その説明を返す。
#[tauri::command]
pub fn set_hotkeys(app: AppHandle, list: Vec<HotkeyBinding>) -> Result<(), String> {
    let mut parsed = Vec::new();
    let mut errors = Vec::new();
    for b in list {
        match parse(&b.shortcut) {
            Some(c) => parsed.push((c, Action { label: b.label, emoji_id: b.emoji_id })),
            None => errors.push(format!("キー「{}」の書き方が正しくありません", b.shortcut)),
        }
    }
    {
        let mut b = bindings().lock().unwrap();
        b.list = parsed;
        b.suspended = false;
    }
    #[cfg(target_os = "windows")]
    win::start(app);
    #[cfg(not(target_os = "windows"))]
    let _ = app;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// 設定画面でキーを割り当てている間は、押したキーでリアクションが飛ばないようにする
/// (割り当てが終わったらset_hotkeysで戻る)。
#[tauri::command]
pub fn suspend_hotkeys() {
    bindings().lock().unwrap().suspended = true;
}

/// スキャンコードでは区別できない・表に無いキーを、仮想キーコードから
/// KeyboardEvent.codeと同じ名前にする(Pauseは NumLockと同じスキャンコード0x45で届く、など)。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn vk_to_code(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x13 => "Pause",
        0x03 => "Pause", // Ctrl+Pause はVK_CANCELで届く
        0x90 => "NumLock",
        0x91 => "ScrollLock",
        0x2C => "PrintScreen",
        0x14 => "CapsLock",
        0x5D => "ContextMenu",
        0xAD => "AudioVolumeMute",
        0xAE => "AudioVolumeDown",
        0xAF => "AudioVolumeUp",
        0xB0 => "MediaTrackNext",
        0xB1 => "MediaTrackPrevious",
        0xB2 => "MediaStop",
        0xB3 => "MediaPlayPause",
        _ => return None,
    })
}

/// キーボードのスキャンコード(物理的なキーの位置)を、ブラウザのKeyboardEvent.codeと
/// 同じ名前にする(設定画面で割り当てたキーの書き方と合わせるため)。
/// extendedは矢印キー・Home等や、テンキーのEnter・/ を区別するためのフラグ。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn scan_to_code(scan: u32, extended: bool) -> Option<String> {
    let s = match (scan, extended) {
        (0x01, _) => "Escape",
        (0x02..=0x0A, _) => return Some(format!("Digit{}", scan - 1)),
        (0x0B, _) => "Digit0",
        (0x0C, _) => "Minus",
        (0x0D, _) => "Equal",
        (0x0E, _) => "Backspace",
        (0x0F, _) => "Tab",
        (0x1A, _) => "BracketLeft",
        (0x1B, _) => "BracketRight",
        (0x1C, false) => "Enter",
        (0x1C, true) => "NumpadEnter",
        (0x27, _) => "Semicolon",
        (0x28, _) => "Quote",
        (0x29, _) => "Backquote",
        (0x2B, _) => "Backslash",
        (0x33, _) => "Comma",
        (0x34, _) => "Period",
        (0x35, false) => "Slash",
        (0x35, true) => "NumpadDivide",
        (0x37, _) => "NumpadMultiply",
        (0x39, _) => "Space",
        (0x3B..=0x44, _) => return Some(format!("F{}", scan - 0x3A)),
        (0x57, _) => "F11",
        (0x58, _) => "F12",
        (0x64..=0x6E, _) => return Some(format!("F{}", scan - 0x64 + 13)),
        (0x76, _) => "F24",
        (0x47, false) => "Numpad7",
        (0x47, true) => "Home",
        (0x48, false) => "Numpad8",
        (0x48, true) => "ArrowUp",
        (0x49, false) => "Numpad9",
        (0x49, true) => "PageUp",
        (0x4A, _) => "NumpadSubtract",
        (0x4B, false) => "Numpad4",
        (0x4B, true) => "ArrowLeft",
        (0x4C, _) => "Numpad5",
        (0x4D, false) => "Numpad6",
        (0x4D, true) => "ArrowRight",
        (0x4E, _) => "NumpadAdd",
        (0x4F, false) => "Numpad1",
        (0x4F, true) => "End",
        (0x50, false) => "Numpad2",
        (0x50, true) => "ArrowDown",
        (0x51, false) => "Numpad3",
        (0x51, true) => "PageDown",
        (0x52, false) => "Numpad0",
        (0x52, true) => "Insert",
        (0x53, false) => "NumpadDecimal",
        (0x53, true) => "Delete",
        // 日本語キーボードのキー
        (0x70, _) => "KanaMode",
        (0x73, _) => "IntlRo",
        (0x79, _) => "Convert",
        (0x7B, _) => "NonConvert",
        (0x7D, _) => "IntlYen",
        // 英字(物理的な位置。Q W E R T Y … の並び)
        _ => {
            const ROWS: [(u32, &str); 3] = [(0x10, "QWERTYUIOP"), (0x1E, "ASDFGHJKL"), (0x2C, "ZXCVBNM")];
            for (start, letters) in ROWS {
                if scan >= start && scan < start + letters.len() as u32 {
                    let c = letters.as_bytes()[(scan - start) as usize] as char;
                    return Some(format!("Key{c}"));
                }
            }
            return None;
        }
    };
    Some(s.to_string())
}

/// 設定のキーの名前(KeyboardEvent.code)を、RegisterHotKeyで使う仮想キーコードにする。
/// 管理者として動いているアプリが前面にある時だけ使う(win::sync_registered参照)。
/// 記号キーなどキーボードの配列で変わるものは対応しない(None)。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn code_to_vk(code: &str) -> Option<u32> {
    if let Some(c) = code.strip_prefix("Key") {
        let b = c.as_bytes();
        return (b.len() == 1 && b[0].is_ascii_uppercase()).then(|| b[0] as u32);
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return d.parse::<u32>().ok().filter(|n| *n <= 9).map(|n| 0x30 + n);
    }
    if let Some(d) = code.strip_prefix("Numpad").and_then(|d| d.parse::<u32>().ok()) {
        return (d <= 9).then(|| 0x60 + d);
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| 0x70 + n - 1);
    }
    Some(match code {
        "NumpadMultiply" => 0x6A,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E,
        "NumpadDivide" => 0x6F,
        "Enter" | "NumpadEnter" => 0x0D,
        "Pause" => 0x13,
        "NumLock" => 0x90,
        "ScrollLock" => 0x91,
        "PrintScreen" => 0x2C,
        "CapsLock" => 0x14,
        "ContextMenu" => 0x5D,
        "Escape" => 0x1B,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "AudioVolumeMute" => 0xAD,
        "AudioVolumeDown" => 0xAE,
        "AudioVolumeUp" => 0xAF,
        "MediaTrackNext" => 0xB0,
        "MediaTrackPrevious" => 0xB1,
        "MediaStop" => 0xB2,
        "MediaPlayPause" => 0xB3,
        _ => return None,
    })
}

/// キーボードの見張り。
///
/// 普段は低レベルフック(WH_KEYBOARD_LL)で「見るだけ」にして、ホットキーのキーでも
/// 文字が打てるようにしている。ただしWindowsの決まりで、**管理者として動いている
/// アプリ(管理者で起動したOBS・ゲーム・専ブラ等)が前面にある間は、普通の権限の
/// ReaTapのフックにはキーが届かない**(2026-10-19、ReaCastで判明。配信者のOBS・
/// Jane2chが管理者で動いていて、ホットキーが効かなかった)。そこで、前面のアプリが管理者で動いている
/// 間だけ、Windowsのホットキー(RegisterHotKey。前面のアプリの権限に関係なく届く)
/// にも登録する。その間はそのキーが前面のアプリに届かなくなるので、前面が普通の
/// アプリに戻ったらすぐ登録を外す。
#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
        MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, SetTimer,
        SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN,
        WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
    };

    static APP: OnceLock<AppHandle> = OnceLock::new();
    /// ホットキーが動いた回数(テスト用)
    pub(super) static FIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 押しっぱなしの間に何度も反応しないよう、押されているキーと最後に届いた時刻を
    /// 覚えておく。離したのを見逃しても(離す瞬間に管理者のアプリが前面だった等)
    /// 1秒以上届かなければ新しく押したとみなす(押しっぱなしの繰り返しは約0.03秒おきに届く)。
    fn held() -> &'static Mutex<HashMap<String, Instant>> {
        static H: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        H.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 }
    }

    fn label(action: &Action) -> String {
        format!("{}のリアクション{}", action.label, action.emoji_id)
    }

    /// ホットキーの動作を実行する(フックの中ではすぐ戻らないといけないので別のスレッドで)。
    fn fire(action: Action, via: &str) {
        FIRED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        log::info!("ホットキー: {}({via})", label(&action));
        if let Some(app) = APP.get() {
            let app = app.clone();
            std::thread::spawn(move || run_action(&app, action));
        }
    }

    fn find_action(combo: &Combo) -> Option<Action> {
        let b = bindings().lock().unwrap();
        if b.suspended {
            return None;
        }
        b.list.iter().find(|(c, _)| c == combo).map(|(_, a)| a.clone())
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let msg = wparam.0 as u32;
            let extended = (info.flags.0 & LLKHF_EXTENDED.0) != 0;
            let key = vk_to_code(info.vkCode).map(str::to_string).or_else(|| scan_to_code(info.scanCode, extended));
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
                        let combo = Combo {
                            ctrl: down(VK_CONTROL),
                            alt: down(VK_MENU),
                            shift: down(VK_SHIFT),
                            meta: down(VK_LWIN) || down(VK_RWIN),
                            code: key,
                        };
                        if let Some(action) = find_action(&combo) {
                            fire(action, "キーボードの見張り");
                        }
                    }
                } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                    held().lock().unwrap().remove(&key);
                }
            }
        }
        // 必ず次へ渡す(=キーを横取りしない。ホットキーのキーでも文字が打てる)
        CallNextHookEx(None, code, wparam, lparam)
    }

    /// そのプロセスが管理者として動いているか(調べられない時はfalse)
    fn process_elevated(process: HANDLE) -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }

    /// 前面のウィンドウのアプリが管理者として動いているか
    fn foreground_elevated() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == std::process::id() {
                return false;
            }
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let elevated = process_elevated(h);
            let _ = CloseHandle(h);
            elevated
        }
    }

    /// RegisterHotKeyで登録しているホットキー(番号 → 動作)
    struct Registered {
        ids: Vec<(i32, Action)>,
        /// 登録した時の割り当て(変わったら登録し直す)
        list: Vec<(Combo, Action)>,
    }

    /// 前面のアプリに合わせて、Windowsのホットキーを登録する/外す(このスレッドで呼ぶ)。
    fn sync_registered(reg: &mut Option<Registered>, self_elevated: bool) {
        let (list, suspended) = {
            let b = bindings().lock().unwrap();
            (b.list.clone(), b.suspended)
        };
        let want = !self_elevated && !suspended && !list.is_empty() && foreground_elevated();
        if reg.as_ref().is_some_and(|r| !want || r.list != list) {
            for (id, _) in reg.take().unwrap().ids {
                unsafe {
                    let _ = UnregisterHotKey(None, id);
                }
            }
            if !want {
                log::info!("ホットキー: 前面のアプリが普通の権限に戻ったので、キーボードの見張りだけに戻しました");
            }
        }
        if want && reg.is_none() {
            let mut ids = Vec::new();
            for (i, (combo, action)) in list.iter().enumerate() {
                let Some(vk) = code_to_vk(&combo.code) else {
                    log::warn!("ホットキー: {}のキー「{}」は、管理者のアプリが前面の時は使えません", label(action), combo.code);
                    continue;
                };
                let mut mods = MOD_NOREPEAT;
                if combo.ctrl {
                    mods |= MOD_CONTROL;
                }
                if combo.alt {
                    mods |= MOD_ALT;
                }
                if combo.shift {
                    mods |= MOD_SHIFT;
                }
                if combo.meta {
                    mods |= MOD_WIN;
                }
                let id = 0x5243 + i as i32;
                match unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods.0), vk) } {
                    Ok(()) => ids.push((id, action.clone())),
                    Err(e) => log::warn!("ホットキー: {}のキー「{}」を登録できませんでした(他のアプリが使用中など): {e}", label(action), combo.code),
                }
            }
            log::info!("ホットキー: 管理者として動いているアプリが前面にあるので、Windowsのホットキーにも登録しました");
            *reg = Some(Registered { ids, list });
        }
    }

    /// フックを入れて、メッセージを受け取り続ける(専用のスレッドで動かす)。
    pub(super) fn install() {
        std::thread::spawn(|| unsafe {
            let module = GetModuleHandleW(None).ok();
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module.map(|m| m.into()), 0);
            if let Err(e) = hook {
                log::warn!("ホットキーのためのキーボードの見張りを始められませんでした: {e}");
                return;
            }
            let self_elevated = process_elevated(GetCurrentProcess());
            // 前面のアプリを0.3秒おきに確かめる
            SetTimer(None, 0, 300, None);
            let mut reg: Option<Registered> = None;
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_TIMER => sync_registered(&mut reg, self_elevated),
                    WM_HOTKEY => {
                        let id = msg.wParam.0 as i32;
                        if let Some((_, action)) = reg.as_ref().and_then(|r| r.ids.iter().find(|(i, _)| *i == id)) {
                            fire(action.clone(), "Windowsのホットキー");
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// キーボードの見張りを始める(2回目以降は何もしない)。
    pub fn start(app: AppHandle) {
        if APP.set(app).is_err() {
            return;
        }
        install();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts_like_the_settings_window() {
        let c = parse("Ctrl+Alt+KeyS").unwrap();
        assert!(c.ctrl && c.alt && !c.shift && !c.meta);
        assert_eq!(c.code, "KeyS");
        assert_eq!(parse("F9").unwrap().code, "F9");
        assert!(parse("").is_none());
        assert!(parse("Ctrl+").is_none());
    }

    #[test]
    fn maps_scan_codes_to_key_codes() {
        assert_eq!(scan_to_code(0x1E, false).as_deref(), Some("KeyA"));
        assert_eq!(scan_to_code(0x1F, false).as_deref(), Some("KeyS"));
        assert_eq!(scan_to_code(0x32, false).as_deref(), Some("KeyM"));
        assert_eq!(scan_to_code(0x10, false).as_deref(), Some("KeyQ"));
        assert_eq!(scan_to_code(0x02, false).as_deref(), Some("Digit1"));
        assert_eq!(scan_to_code(0x0B, false).as_deref(), Some("Digit0"));
        assert_eq!(scan_to_code(0x43, false).as_deref(), Some("F9"));
        assert_eq!(scan_to_code(0x58, false).as_deref(), Some("F12"));
        assert_eq!(scan_to_code(0x48, true).as_deref(), Some("ArrowUp"));
        assert_eq!(scan_to_code(0x48, false).as_deref(), Some("Numpad8"));
        assert_eq!(scan_to_code(0x1D, false), None); // Ctrl自体はキーにしない
        // スキャンコードではNumLockと同じになるキー・表に無いキーは仮想キーコードで
        assert_eq!(vk_to_code(0x13), Some("Pause"));
        assert_eq!(vk_to_code(0x90), Some("NumLock"));
        assert_eq!(vk_to_code(0xB3), Some("MediaPlayPause"));
        assert_eq!(vk_to_code(0x41), None); // 普通のキーはスキャンコードで
    }
}
