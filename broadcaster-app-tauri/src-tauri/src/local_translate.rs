//! PCの中で翻訳する(Chrome/Edgeに入っている翻訳の機能。Translator API)。
//!
//! Google翻訳(非公式の方法)は、使いすぎると断られる(HTTP 429)ことがあり、長時間の
//! 配信では何度も止まった(2026-10-28)。Chrome/Edgeには、ネットに頼まずPCの中だけで
//! 訳す機能(Translator API)が入っていて、回数の上限が無い。ReaCastの画面(WebView2)
//! では使えるか分からないので、話した字幕と同じように、Chrome(無ければEdge)の小さな窓
//! (frontend/translate_worker.html)を最小化して開き、OBS連携のWebSocketで頼む。
//!
//! ・翻訳データのダウンロード(言語の組み合わせごとに最初の1回だけ)はブラウザの決まりで
//!   ボタンを押す必要があるので、窓に「翻訳の準備をする」ボタンを出して、窓を前に出す
//! ・準備中・その言語に未対応・返事が無い時は、None を返して呼び出し側(voice.rsの
//!   translate_detect)がGoogle翻訳を使う
//! ・訳の質はChromeの方が良かった(Edgeは「強すぎない？」を逆の意味に訳した)ので、Chrome優先

use crate::comments::CommentEngine;
use crate::obs_bridge::{ObsBridge, OBS_HTTP_PORT};
use crate::voice::EdgeProcess;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 1件の翻訳の返事を待つ時間(過ぎたらGoogle翻訳を使う。普段は0.1秒もかからない)
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// 窓が閉じられていた時に開き直す間隔の最短
const RELAUNCH_GAP: Duration = Duration::from_secs(60);

type Reply = Result<(String, String), String>;

#[derive(Default)]
struct Local {
    /// 開いている翻訳の窓と、その時に開いたURL(設定が変わったら開き直すため)
    window: Mutex<Option<(EdgeProcess, String)>>,
    /// 設定が「PCの中で翻訳」で、翻訳を使う設定になっている
    enabled: AtomicBool,
    /// 翻訳の窓が繋がっていて、頼める状態
    connected: AtomicBool,
    /// 「準備」のボタンを押してもらうために窓を前に出した(何度も出さない)
    shown_for_prep: AtomicBool,
    last_launch: Mutex<Option<Instant>>,
    pending: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Reply>>>,
    status: Mutex<serde_json::Value>,
}

fn local() -> &'static Local {
    static LOCAL: OnceLock<Local> = OnceLock::new();
    LOCAL.get_or_init(Local::default)
}

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Google翻訳の言語コード → Chromeの翻訳の言語コード
fn chrome_code(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh-Hans" => "zh",
        "zh-TW" => "zh-Hant",
        l => l,
    }
}

/// Chromeの言語コード → Google翻訳の言語コード(呼び出し側は翻訳先と比べる)
fn google_code(lang: &str) -> String {
    match lang {
        "zh" | "zh-Hans" => "zh-CN".into(),
        "zh-Hant" => "zh-TW".into(),
        l => l.to_string(),
    }
}

/// ひらがな・カタカナが入っている(=日本語。漢字だけだと中国語の時もあるので判定に任せる)
fn has_kana(text: &str) -> bool {
    text.chars().any(|c| matches!(c as u32, 0x3040..=0x30FF | 0xFF66..=0xFF9F))
}

/// PCの中で翻訳する。(訳文, 元の言語)。使えない時はNone(Google翻訳を使ってもらう)
pub(crate) async fn translate(text: &str, source: Option<&str>, target: &str) -> Option<(String, String)> {
    let l = local();
    if !l.enabled.load(Ordering::SeqCst) {
        return None;
    }
    let app = APP.get()?;
    if !l.connected.load(Ordering::SeqCst) {
        relaunch_if_closed(app);
        return None;
    }
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = tokio::sync::oneshot::channel();
    l.pending.lock().unwrap().insert(id, tx);
    let source = source.map(|s| chrome_code(s).to_string()).or_else(|| has_kana(text).then(|| "ja".into()));
    app.state::<ObsBridge>().push_voice(serde_json::json!({
        "type": "translateRequest",
        "id": id,
        "text": text,
        "source": source,
        "target": chrome_code(target),
    }));
    match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok((t, s)))) => Some((t, google_code(&s))),
        // 準備がまだ・その言語に未対応など(窓が状態を知らせてくる)
        Ok(Ok(Err(_))) => None,
        _ => {
            l.pending.lock().unwrap().remove(&id);
            if l.connected.swap(false, Ordering::SeqCst) {
                log::warn!("PCの中での翻訳: 翻訳の窓から返事が無いので、しばらくGoogle翻訳を使います");
                set_status(app, "wait", "翻訳の窓から返事がありません(その間はGoogle翻訳(ネット)を使います)");
            }
            None
        }
    }
}

/// 翻訳の窓からのメッセージ(voice.rsの受け口から渡される)
pub fn on_message(app: &AppHandle, msg: &serde_json::Value) {
    let l = local();
    match msg["type"].as_str() {
        Some("translateResult") => {
            let Some(id) = msg["id"].as_u64() else { return };
            let Some(tx) = l.pending.lock().unwrap().remove(&id) else { return };
            let reply = match msg["text"].as_str() {
                Some(t) if !t.trim().is_empty() => Ok((t.to_string(), msg["source"].as_str().unwrap_or_default().to_string())),
                _ => Err(msg["error"].as_str().unwrap_or("翻訳できませんでした").to_string()),
            };
            let _ = tx.send(reply);
        }
        Some("translatorStatus") => {
            let state = msg["state"].as_str().unwrap_or("wait");
            l.connected.store(matches!(state, "ready" | "needs-click" | "preparing"), Ordering::SeqCst);
            // 最初の1回だけの準備が要る時は、ボタンを押してもらえるように窓を前に出す(窓を
            // 開いた時の確認でだけ。配信中に別の言語のレスが来ても、いきなり前に出さない)
            if state == "needs-click" && msg["popup"] == true && !l.shown_for_prep.swap(true, Ordering::SeqCst) {
                if let Some((p, _)) = l.window.lock().unwrap().as_ref() {
                    p.show();
                }
            }
            set_status(app, state, msg["message"].as_str().unwrap_or_default());
        }
        _ => {}
    }
}

fn set_status(app: &AppHandle, state: &str, message: &str) {
    let v = serde_json::json!({ "state": state, "message": message });
    {
        let mut cur = local().status.lock().unwrap();
        if *cur == v {
            return;
        }
        // ダウンロードの進み具合(%)は、状態が変わった時だけ残す
        if cur["state"] != state || state != "preparing" {
            log::info!("PCの中での翻訳: {state} {message}");
        }
        *cur = v.clone();
    }
    let _ = app.emit("translate:status", v);
}

/// 翻訳の窓に、最初に準備しておく言語の組み合わせ(元>先)
fn prep_pairs(s: &crate::comments::CommentSettings) -> Vec<String> {
    let mut pairs: Vec<String> = Vec::new();
    let mut add = |src: &str, dst: &str| {
        let p = format!("{}>{}", chrome_code(src), chrome_code(dst));
        if src != dst && !pairs.contains(&p) {
            pairs.push(p);
        }
    };
    if s.voice_enabled && s.voice_translate {
        add(s.voice_lang.split('-').next().unwrap_or("ja"), s.voice_target_lang.trim());
    }
    if s.subtitle_translate {
        let target = match s.subtitle_translate_target.trim() {
            "" | "ja" => "en",
            t => t,
        };
        add("ja", target);
        add("en", "ja");
    }
    pairs
}

/// 設定に合わせて、翻訳の窓を開く・閉じる・開き直す(起動時と、設定を保存した時)
pub fn sync(app: &AppHandle) {
    let _ = APP.set(app.clone());
    let s = app.state::<Arc<CommentEngine>>().settings();
    let pairs = prep_pairs(&s);
    let want_local = s.translate_engine != "google" && !pairs.is_empty();
    let l = local();
    l.enabled.store(want_local, Ordering::SeqCst);
    let mut window = l.window.lock().unwrap();
    if window.as_ref().is_some_and(|(p, _)| !p.is_running()) {
        *window = None;
        l.connected.store(false, Ordering::SeqCst);
    }
    let want = want_local
        .then(|| format!("http://127.0.0.1:{OBS_HTTP_PORT}/translate_worker.html?prep={}", pairs.join(",")));
    if matches!((&*window, &want), (Some((_, url)), Some(w)) if url == w) {
        return;
    }
    if let Some((p, _)) = window.take() {
        p.kill();
    }
    l.connected.store(false, Ordering::SeqCst);
    let Some(url) = want else {
        drop(window);
        let msg = if s.translate_engine == "google" { "Google翻訳(ネット)を使っています" } else { "使っていません" };
        set_status(app, "off", msg);
        return;
    };
    *l.last_launch.lock().unwrap() = Some(Instant::now());
    // 開き直した窓でも、準備が要ればもう一度前に出す
    l.shown_for_prep.store(false, Ordering::SeqCst);
    match crate::voice::launch_app_window(app, &url, "auto", "translate", &[]) {
        Ok((child, name)) => {
            log::info!("PCの中での翻訳: {name}で翻訳の窓を開きました");
            *window = Some((child, url));
            drop(window);
            set_status(app, "wait", "翻訳の窓を開いています…");
        }
        Err(e) => {
            drop(window);
            log::warn!("PCの中での翻訳: {e}");
            set_status(app, "error", &format!("{e}(Google翻訳(ネット)を使います)"));
        }
    }
}

/// 翻訳の窓が閉じられていたら開き直す(頻繁には開き直さない)
fn relaunch_if_closed(app: &AppHandle) {
    let l = local();
    let closed = l.window.lock().unwrap().as_ref().is_none_or(|(p, _)| !p.is_running());
    let recent = l.last_launch.lock().unwrap().is_some_and(|t| t.elapsed() < RELAUNCH_GAP);
    if closed && !recent {
        // 閉じた窓は捨てて開き直す(同じURLでも開く)
        if let Some((p, _)) = l.window.lock().unwrap().take() {
            p.kill();
        }
        sync(app);
    }
}

/// ReaCastの終了時に、翻訳の窓も閉じる
pub fn stop() {
    if let Some((p, _)) = local().window.lock().unwrap().take() {
        p.kill();
    }
}

/// 今の状態(設定画面に出す)。{state, message}
#[tauri::command]
pub fn translate_status() -> serde_json::Value {
    let v = local().status.lock().unwrap().clone();
    if v.is_null() {
        serde_json::json!({ "state": "off", "message": "使っていません" })
    } else {
        v
    }
}

/// 翻訳の窓を前に出す(設定画面の「翻訳の窓を出す」。準備のボタンを押すため)。
/// 閉じられていたら開き直す(準備が要れば、開いた窓が前に出る)
#[tauri::command(async)]
pub fn translate_show_window(app: AppHandle) {
    let l = local();
    {
        let window = l.window.lock().unwrap();
        if let Some((p, _)) = window.as_ref().filter(|(p, _)| p.is_running()) {
            p.show();
            return;
        }
    }
    if let Some((p, _)) = l.window.lock().unwrap().take() {
        p.kill();
    }
    sync(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_language_codes() {
        assert_eq!(chrome_code("zh-CN"), "zh");
        assert_eq!(chrome_code("zh-TW"), "zh-Hant");
        assert_eq!(chrome_code("en"), "en");
        assert_eq!(google_code("zh-Hans"), "zh-CN");
        assert_eq!(google_code("zh-Hant"), "zh-TW");
        assert_eq!(google_code("pt"), "pt");
    }

    #[test]
    fn detects_kana_only() {
        assert!(has_kana("草生える"));
        assert!(has_kana("ｱｲｳ"));
        assert!(!has_kana("今日最高"));
        assert!(!has_kana("hello"));
    }
}
