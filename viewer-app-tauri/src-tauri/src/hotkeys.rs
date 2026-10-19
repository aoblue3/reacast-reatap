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
pub(crate) struct Combo {
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) meta: bool,
    pub(crate) code: String,
}

pub(crate) fn parse(key: &str) -> Option<Combo> {
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
    let mut keys = Vec::new();
    let mut errors = Vec::new();
    for b in list {
        match parse(&b.shortcut) {
            Some(c) => {
                keys.push(b.shortcut.trim().to_string());
                parsed.push((c, Action { label: b.label, emoji_id: b.emoji_id }));
            }
            None => errors.push(format!("キー「{}」の書き方が正しくありません", b.shortcut)),
        }
    }
    {
        let mut b = bindings().lock().unwrap();
        b.list = parsed;
        b.suspended = false;
    }
    // 管理者の見張り係にも同じ順番で伝える(知らせはこの順番の番号で届く。fire_index)
    crate::key_watcher::set_bindings(keys, false);
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
    crate::key_watcher::set_suspended();
}

/// スキャンコードでは区別できない・表に無いキーを、仮想キーコードから
/// KeyboardEvent.codeと同じ名前にする(Pauseは NumLockと同じスキャンコード0x45で届く、など)。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn vk_to_code(vk: u32) -> Option<&'static str> {
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
pub(crate) fn scan_to_code(scan: u32, extended: bool) -> Option<String> {
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

/// キーボードの見張り(本体の普通の権限で動く側)。
///
/// 低レベルフック(WH_KEYBOARD_LL)で「見るだけ」にして、ホットキーのキーでも文字が
/// 打てるようにしている。管理者として動いているアプリ(管理者で起動したOBS等)が
/// 前面にある間はWindowsの決まりでキーが届かないので、その場合のために管理者の権限で
/// 動く見張り係(key_watcher.rs)を使える。見張り係と繋がっている間は、こちらでは
/// 反応しない(見張り係が全部のキーを見ているので、二重に動かないように)。
#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// 押しっぱなしの間に何度も反応しないよう、押されているキーと最後に届いた時刻を
    /// 覚えておく。離したのを見逃しても1秒以上届かなければ新しく押したとみなす
    /// (押しっぱなしの繰り返しは約0.03秒おきに届く)。
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
    pub(super) fn fire(action: Action, via: &str) {
        log::info!("ホットキー: {}({via})", label(&action));
        if let Some(app) = APP.get() {
            let app = app.clone();
            std::thread::spawn(move || run_action(&app, action));
        }
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && !crate::key_watcher::connected() {
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
                        let action = {
                            let b = bindings().lock().unwrap();
                            if b.suspended {
                                None
                            } else {
                                b.list.iter().find(|(c, _)| *c == combo).map(|(_, a)| a.clone())
                            }
                        };
                        if let Some(action) = action {
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

    /// キーボードの見張りを始める(2回目以降は何もしない)。フックはメッセージを
    /// 受け取り続けるスレッドが必要なので、専用のスレッドで動かす。
    pub fn start(app: AppHandle) {
        if APP.set(app).is_err() {
            return;
        }
        // 管理者の見張り係からの知らせを受け付け、登録済みなら見張り係も起動する
        crate::key_watcher::start_server(super::fire_index);
        crate::key_watcher::launch_if_installed();
        std::thread::spawn(|| unsafe {
            let module = GetModuleHandleW(None).ok();
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module.map(|m| m.into()), 0);
            if let Err(e) = hook {
                log::warn!("ホットキーのためのキーボードの見張りを始められませんでした: {e}");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        });
    }
}

/// 管理者の見張り係から「割り当ての何番目が押された」と届いた時。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn fire_index(i: usize) {
    let action = {
        let b = bindings().lock().unwrap();
        if b.suspended {
            return;
        }
        b.list.get(i).map(|(_, a)| a.clone())
    };
    #[cfg(target_os = "windows")]
    if let Some(action) = action {
        win::fire(action, "管理者の見張り係");
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
