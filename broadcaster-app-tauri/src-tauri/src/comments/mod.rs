//! コメント読み上げ・字幕機能(SpeechCastMe + unacastの置き換え)。
//!
//! 掲示板(jpnkn等の2ch互換・したらば)の新着レスを取得し、1本の順番待ち列に
//! 並べて、1件ずつ「字幕を表示 → 読み上げコマンド(vrx.exe等)に渡す →
//! 読み上げ間隔だけ待つ」を繰り返す。字幕(下)・レス一覧(右)・デスクトップ
//! 字幕は、どれもobs_bridgeのWebSocketから同じメッセージを受け取って描画する
//! ので、表示のタイミングがずれない。
//!
//! 取得・順番待ち・コマンド実行はすべてRust側(このモジュール)で行う。
//! コントロールパネル等の画面を隠していてもWebViewのタイマー抑制の影響を
//! 受けずに動き続けるようにするため。

pub mod source;
pub mod text;

use crate::config_store::ConfigStore;
use crate::obs_bridge::ObsBridge;
use serde::{Deserialize, Serialize};
use source::{Board, ThreadReader, ThreadRef};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

pub const SETTINGS_KEY: &str = "commentSettings";

/// 起動直後(最初の取得)に、既に書き込まれていたレスのうち右のレス一覧に
/// 載せておく件数(読み上げはしない)。
const INITIAL_HISTORY: usize = 5;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentSettings {
    // ---- 取得 ----
    pub thread_url: String,
    pub poll_interval_ms: u64,
    pub auto_next_thread: bool,
    pub next_thread_keyword: String,
    /// このレス番号に達したら一度だけ「次スレを立ててください」と知らせる(0で知らせない)
    pub thread_warn_count: u32,
    pub start_on_launch: bool,
    // ---- 読み上げ ----
    pub speech_enabled: bool,
    pub command_path: String,
    pub command_args: String,
    pub read_interval_ms: u64,
    pub turbo_interval_ms: u64,
    /// 順番待ちがこの件数以上溜まったらターボ(短い間隔)にする
    pub turbo_threshold: usize,
    pub read_res_number: bool,
    pub max_chars: usize,
    pub reading_rules: String,
    pub ng_words: String,
    // ---- 字幕 ----
    pub display_ms: u64,
    pub aa_display_ms: u64,
    // ---- AAモード ----
    pub aa_threshold_chars: usize,
    pub aa_patterns: String,
    pub aa_read_quoted_only: bool,
    pub aa_read_res_number: bool,
    pub aa_fixed_reading: String,
    // ---- デスクトップ字幕 ----
    pub desktop_enabled: bool,
    pub desktop_monitor_id: Option<String>,
    pub desktop_x: f64,
    pub desktop_y: f64,
    pub desktop_width: f64,
    pub desktop_height: f64,
    /// 見た目の設定(フォント・色・縁など)。Rust側では中身を解釈せず、そのまま
    /// 字幕・レス一覧の画面に渡す(画面側が既定値を補う)。
    pub style: serde_json::Value,
}

impl Default for CommentSettings {
    fn default() -> Self {
        Self {
            thread_url: String::new(),
            poll_interval_ms: 7000,
            auto_next_thread: true,
            next_thread_keyword: String::new(),
            thread_warn_count: 975,
            start_on_launch: false,
            speech_enabled: true,
            command_path: r"C:\tamiyasuex\vrx.exe".into(),
            command_args: "#Res#".into(),
            read_interval_ms: 3000,
            turbo_interval_ms: 400,
            turbo_threshold: 3,
            read_res_number: true,
            max_chars: 270,
            reading_rules: [
                r"(https|ttps)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/リンク",
                r"(http|ttp)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/リンク",
                "ww+/ワラワラ",
                "ｗｗ+/ワラワラ",
            ]
            .join("\n"),
            ng_words: String::new(),
            display_ms: 3000,
            aa_display_ms: 6000,
            aa_threshold_chars: 300,
            aa_patterns: ["Д", "(●)", "从", "∀", "( 人 )"].join("\n"),
            aa_read_quoted_only: true,
            aa_read_res_number: true,
            aa_fixed_reading: "アスキーアート".into(),
            desktop_enabled: true,
            desktop_monitor_id: None,
            desktop_x: 0.0,
            desktop_y: 0.78,
            desktop_width: 1.0,
            desktop_height: 0.22,
            style: serde_json::json!({}),
        }
    }
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommentStatus {
    pub running: bool,
    pub thread_title: Option<String>,
    pub thread_url: Option<String>,
    pub last_no: u32,
    pub queue_len: usize,
    pub error: Option<String>,
    pub warning: Option<String>,
}

#[derive(Clone)]
struct Post {
    no: u32,
    name: String,
    date: String,
    body: String,
    /// ReaCast自身からのお知らせ(次スレ移動・スレ建て警告・テスト表示)
    system: bool,
}

impl Post {
    fn from_raw(p: &source::RawPost) -> Self {
        Self {
            no: p.no,
            name: text::html_to_text(&p.name),
            date: p.date.clone(),
            body: text::html_to_text(&p.body_html),
            system: false,
        }
    }

    fn system(body: impl Into<String>) -> Self {
        Self { no: 0, name: "ReaCast".into(), date: String::new(), body: body.into(), system: true }
    }

    fn to_json(&self, aa: bool) -> serde_json::Value {
        serde_json::json!({
            "no": self.no,
            "name": self.name,
            "date": self.date,
            "body": self.body,
            "aa": aa,
            "system": self.system,
        })
    }
}

/// 設定から作る、処理用にコンパイル済みの値(レスごとに作り直さないため)。
struct Compiled {
    ng_words: Vec<String>,
    aa_patterns: Vec<String>,
    reading_rules: Vec<text::ReadingRule>,
}

impl Compiled {
    fn from(s: &CommentSettings) -> Self {
        Self {
            ng_words: text::non_empty_lines(&s.ng_words),
            aa_patterns: text::non_empty_lines(&s.aa_patterns),
            reading_rules: text::parse_reading_rules(&s.reading_rules),
        }
    }
}

struct State {
    settings: CommentSettings,
    compiled: Arc<Compiled>,
    /// start/stopのたびに増やす。古い取得タスクはこれが変わったら終了する。
    run_id: u64,
    queue: VecDeque<Post>,
    status: CommentStatus,
}

pub struct CommentEngine {
    app: AppHandle,
    state: Mutex<State>,
    wake: Notify,
}

impl CommentEngine {
    pub fn new(app: AppHandle, settings: CommentSettings) -> Arc<Self> {
        let engine = Arc::new(Self {
            app,
            state: Mutex::new(State {
                compiled: Arc::new(Compiled::from(&settings)),
                settings,
                run_id: 0,
                queue: VecDeque::new(),
                status: CommentStatus::default(),
            }),
            wake: Notify::new(),
        });
        let worker = engine.clone();
        tauri::async_runtime::spawn(async move { worker.run_queue().await });
        engine
    }

    pub fn settings(&self) -> CommentSettings {
        self.state.lock().unwrap().settings.clone()
    }

    pub fn status(&self) -> CommentStatus {
        self.state.lock().unwrap().status.clone()
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().status.running
    }

    pub fn apply_settings(&self, settings: CommentSettings) {
        let compiled = Arc::new(Compiled::from(&settings));
        let style = settings.style.clone();
        {
            let mut st = self.state.lock().unwrap();
            st.settings = settings;
            st.compiled = compiled;
        }
        self.bridge().set_comment_style(style);
    }

    pub fn start(self: &Arc<Self>, url: &str) -> Result<(), String> {
        let (board, key) = source::parse_url(url)?;
        let run_id = {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.settings.thread_url = url.trim().to_string();
            st.status = CommentStatus { running: true, ..Default::default() };
            st.run_id
        };
        self.emit_status();
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.run_fetch(run_id, board, key).await });
        Ok(())
    }

    pub fn stop(&self) {
        {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.status = CommentStatus::default();
        }
        self.bridge().clear_subtitle();
        self.emit_status();
    }

    /// 設定画面の「テスト表示」用。順番待ちの先頭に割り込ませる。
    pub fn test(&self, body: &str) {
        let mut post = Post::system(body);
        post.no = 1;
        post.name = "テスト".into();
        self.state.lock().unwrap().queue.push_front(post);
        self.wake.notify_one();
    }

    fn bridge(&self) -> ObsBridge {
        self.app.state::<ObsBridge>().inner().clone()
    }

    fn is_current(&self, run_id: u64) -> bool {
        self.state.lock().unwrap().run_id == run_id
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app.emit("comments:status", status);
    }

    fn update_status(&self, run_id: u64, f: impl FnOnce(&mut CommentStatus)) {
        {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            f(&mut st.status);
            st.status.queue_len = st.queue.len();
        }
        self.emit_status();
    }

    fn enqueue(&self, run_id: u64, posts: Vec<Post>) {
        if posts.is_empty() {
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            st.queue.extend(posts);
            st.status.queue_len = st.queue.len();
        }
        self.wake.notify_one();
    }

    async fn run_fetch(self: Arc<Self>, run_id: u64, board: Board, key: Option<String>) {
        let client = source::http_client();
        let poll_interval = |me: &Self| Duration::from_millis(me.settings().poll_interval_ms.max(1000));

        // 板のURLが入力された場合は、一番新しいスレッドを選ぶ
        let key = match key {
            Some(k) => k,
            None => loop {
                if !self.is_current(run_id) {
                    return;
                }
                match source::list_threads(&client, &board).await {
                    Ok(list) => match source::pick_newest(&list) {
                        Some(t) => break t.key.clone(),
                        None => self.update_status(run_id, |s| s.error = Some("板にスレッドが見つかりません".into())),
                    },
                    Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
                }
                tokio::time::sleep(poll_interval(&self)).await;
            },
        };

        let mut reader = ThreadReader::new(ThreadRef { board: board.clone(), key });
        let mut first = true;
        let mut warned = false;
        loop {
            if !self.is_current(run_id) {
                return;
            }
            match reader.poll(&client).await {
                Ok(raw) => {
                    let posts: Vec<Post> = raw.iter().map(Post::from_raw).collect();
                    if first {
                        // 起動時点で既にあったレスは読み上げない(右のレス一覧にだけ載せる)
                        first = false;
                        let compiled = self.state.lock().unwrap().compiled.clone();
                        let settings = self.settings();
                        let skip = posts.len().saturating_sub(INITIAL_HISTORY);
                        for p in posts.iter().skip(skip) {
                            if !text::contains_ng(&p.name, &p.body, &compiled.ng_words) {
                                let aa = text::is_aa(&p.body, settings.aa_threshold_chars, &compiled.aa_patterns);
                                self.bridge().push_comment(p.to_json(aa));
                            }
                        }
                    } else {
                        self.enqueue(run_id, posts);
                    }
                    let (title, url, last_no) = (reader.title.clone(), reader.thread.read_url(), reader.last_no);
                    self.update_status(run_id, |s| {
                        s.error = None;
                        s.thread_title = title;
                        s.thread_url = Some(url);
                        s.last_no = last_no;
                    });

                    let settings = self.settings();
                    if !warned && settings.thread_warn_count > 0 && reader.last_no >= settings.thread_warn_count && reader.last_no < 1000 {
                        warned = true;
                        let msg = format!("レスが{}を超えました。次スレを立ててください", settings.thread_warn_count);
                        self.update_status(run_id, |s| s.warning = Some(msg.clone()));
                        self.enqueue(run_id, vec![Post::system(msg)]);
                    }

                    if settings.auto_next_thread && reader.last_no >= 1000 {
                        match source::list_threads(&client, &board).await {
                            Ok(list) => {
                                if let Some(next) = source::pick_next(&list, &reader.thread.key, &settings.next_thread_keyword) {
                                    // 次スレのレスは1番から全部新着として扱う
                                    reader = ThreadReader::new(ThreadRef { board: board.clone(), key: next.key.clone() });
                                    warned = false;
                                    self.update_status(run_id, |s| s.warning = None);
                                    self.enqueue(run_id, vec![Post::system(format!("次スレ「{}」に移動しました", next.title))]);
                                    continue;
                                } else {
                                    self.update_status(run_id, |s| s.warning = Some("スレッドが1000を超えました。次スレを探しています".into()));
                                }
                            }
                            Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
                        }
                    }
                }
                Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
            }
            tokio::time::sleep(poll_interval(&self)).await;
        }
    }

    async fn run_queue(self: Arc<Self>) {
        loop {
            let next = {
                let mut st = self.state.lock().unwrap();
                st.queue.pop_front().map(|p| (p, st.settings.clone(), st.compiled.clone(), st.queue.len()))
            };
            let Some((post, settings, compiled, backlog)) = next else {
                self.wake.notified().await;
                continue;
            };
            {
                let mut st = self.state.lock().unwrap();
                st.status.queue_len = st.queue.len();
            }
            self.emit_status();

            if !post.system && text::contains_ng(&post.name, &post.body, &compiled.ng_words) {
                continue;
            }
            let aa = !post.system && text::is_aa(&post.body, settings.aa_threshold_chars, &compiled.aa_patterns);
            let bridge = self.bridge();
            bridge.show_subtitle(
                post.to_json(aa),
                if aa { settings.aa_display_ms } else { settings.display_ms },
            );
            bridge.push_comment(post.to_json(aa));

            if settings.speech_enabled && !settings.command_path.trim().is_empty() {
                let spoken = speech_text(&post, aa, &settings, &compiled);
                if !spoken.trim().is_empty() {
                    run_speech_command(&settings, &post, &spoken);
                }
            }

            let interval = if backlog >= settings.turbo_threshold.max(1) {
                settings.turbo_interval_ms
            } else {
                settings.read_interval_ms
            };
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    }
}

/// 読み上げ用の文字列を作る(AAなら固定文字列か「」の中だけ、読み方ルール、
/// レス番号、文字数制限)。
fn speech_text(post: &Post, aa: bool, s: &CommentSettings, c: &Compiled) -> String {
    let body = if aa {
        let quoted = if s.aa_read_quoted_only { text::quoted_only(&post.body) } else { String::new() };
        if quoted.is_empty() { s.aa_fixed_reading.clone() } else { quoted }
    } else {
        post.body.clone()
    };
    let body = text::apply_reading_rules(&body.replace('\n', " "), &c.reading_rules);
    let with_no = if !post.system && post.no > 0 && (if aa { s.aa_read_res_number } else { s.read_res_number }) {
        format!("{}、{}", post.no, body)
    } else {
        body
    };
    text::truncate_chars(&with_no, s.max_chars)
}

/// 読み上げコマンドを起動する。シェルを経由せず引数の配列で直接起動するので、
/// レス本文の内容がコマンドとして解釈されることはない。終了は待たない
/// (vrx.exeは民安★TALKに文字列を渡したらすぐ終わる。順番の管理は読み上げ
/// 間隔で行う=SpeechCastMeと同じ方式)。
fn run_speech_command(s: &CommentSettings, post: &Post, spoken: &str) {
    let values = text::TokenValues { res: spoken, no: post.no, name: &post.name, time: &post.date };
    let args: Vec<String> = text::split_args(&s.command_args).iter().map(|a| text::fill_tokens(a, &values)).collect();
    let mut cmd = std::process::Command::new(s.command_path.trim());
    cmd.args(&args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        // 終了コードの回収だけ別スレッドで行う(プロセスハンドルを残さないため)
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("読み上げコマンドの起動に失敗しました: {e}"),
    }
}

pub fn load_settings(store: &ConfigStore) -> CommentSettings {
    store
        .get(SETTINGS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn save_settings(store: &ConfigStore, settings: &CommentSettings) -> Result<(), String> {
    let v = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    store.set(SETTINGS_KEY.to_string(), v).map_err(|e| e.to_string())
}

// ---------------- Tauriコマンド ----------------

type Engine<'a> = tauri::State<'a, Arc<CommentEngine>>;

#[tauri::command]
pub fn comments_get_settings(engine: Engine) -> CommentSettings {
    engine.settings()
}

#[tauri::command]
pub fn comments_status(engine: Engine) -> CommentStatus {
    engine.status()
}

/// ウィンドウの作成・破棄を伴うことがあるので、メインスレッドを塞がないよう
/// async指定にしている(lib.rsのopen_region_pickerのコメント参照)。
#[tauri::command(async)]
pub fn comments_save_settings(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    settings: CommentSettings,
) -> Result<(), String> {
    save_settings(&store, &settings)?;
    engine.apply_settings(settings);
    crate::sync_subtitle_window(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn comments_start(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    url: String,
) -> Result<(), String> {
    engine.start(&url)?;
    save_settings(&store, &engine.settings())?;
    crate::sync_subtitle_window(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn comments_stop(app: AppHandle, engine: Engine) {
    engine.stop();
    crate::sync_subtitle_window(&app);
}

#[tauri::command]
pub fn comments_test(engine: Engine, text: String) {
    engine.test(&text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(no: u32, body: &str) -> Post {
        Post { no, name: "名無し".into(), date: String::new(), body: body.into(), system: false }
    }

    #[test]
    fn speech_text_for_normal_and_aa_posts() {
        let mut s = CommentSettings::default();
        let c = Compiled::from(&s);
        assert_eq!(speech_text(&post(69, "おつかれ www"), false, &s, &c), "69、おつかれ ワラワラ");
        assert_eq!(speech_text(&post(5, "(´Д｀)「やあ」"), true, &s, &c), "5、やあ");
        assert_eq!(speech_text(&post(5, "(´Д｀)"), true, &s, &c), "5、アスキーアート");
        s.read_res_number = false;
        s.max_chars = 4;
        assert_eq!(speech_text(&post(1, "あいうえおか"), false, &s, &c), "あいうえ");
        // お知らせにはレス番号を付けない
        assert_eq!(speech_text(&Post::system("次スレです"), false, &s, &c), "次スレで");
    }

    #[test]
    fn settings_fill_defaults_for_missing_fields() {
        let s: CommentSettings = serde_json::from_value(serde_json::json!({ "threadUrl": "x" })).unwrap();
        assert_eq!(s.thread_url, "x");
        assert_eq!(s.read_interval_ms, 3000);
        assert_eq!(s.command_args, "#Res#");
    }
}
