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
pub mod twitch;
pub mod youtube;
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
    /// 開始時に右のレス一覧へ載せるレスの開始番号(0なら載せない。読み上げはしない)
    pub start_res_no: u32,
    /// 開始時に右のレス一覧へ出すお知らせ(空なら出さない)
    pub initial_text: String,
    /// 右のレス一覧に出すアイコン画像のフォルダ(掲示板のレス用。中の画像からランダムに選ぶ)
    pub icon_folder_bbs: String,
    pub icon_folder_youtube: String,
    pub icon_folder_twitch: String,
    // ---- 配信サイトのコメント(空欄なら取得しない) ----
    /// YouTubeの配信URL・動画ID・チャンネルURL・@ハンドル
    pub youtube_target: String,
    /// Twitchのチャンネル名(またはURL)
    pub twitch_channel: String,
    // ---- 次スレの自動作成 ----
    pub auto_create_thread: bool,
    /// このレス番号に達したら次スレを建てる
    pub create_thread_count: u32,
    /// {next}=今のタイトルの最後の数字を1増やしたもの、{n}=その数字だけ
    pub create_title_template: String,
    pub create_name: String,
    pub create_mail: String,
    /// {prevUrl}=今のスレッドのURL、{prevTitle}=今のタイトル、{title}=新しいタイトル
    pub create_body_template: String,
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
    /// 長いレスほど長く表示する: 1文字あたりに足す時間(0で長さに関係なく一定)
    pub display_per_char_ms: u64,
    /// 文字数で延ばした場合の表示時間の上限
    pub display_max_ms: u64,
    /// 字幕を出し終わるまで次のレスを待つ(ターボ中は待たない)
    pub wait_for_display: bool,
    pub aa_display_ms: u64,
    // ---- AAモード ----
    pub aa_enabled: bool,
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
    /// マウスで動かして決めた位置・大きさ(物理ピクセル)。あればモニター・割合の
    /// 指定より優先する。
    pub desktop_rect: Option<DesktopRect>,
    /// 見た目の設定(フォント・色・縁など)。Rust側では中身を解釈せず、そのまま
    /// 字幕・レス一覧の画面に渡す(画面側が既定値を補う)。
    pub style: serde_json::Value,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
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
            start_res_no: 0,
            initial_text: "スレッド読み込みを開始しました".into(),
            icon_folder_bbs: String::new(),
            icon_folder_youtube: String::new(),
            icon_folder_twitch: String::new(),
            youtube_target: String::new(),
            twitch_channel: String::new(),
            auto_create_thread: false,
            create_thread_count: 975,
            create_title_template: "{next}".into(),
            create_name: String::new(),
            create_mail: String::new(),
            create_body_template: "前スレ\n{prevUrl}".into(),
            speech_enabled: true,
            command_path: r"C:\tamiyasuex\vrx.exe".into(),
            command_args: "#Res#".into(),
            read_interval_ms: 3000,
            turbo_interval_ms: 400,
            turbo_threshold: 3,
            // 既定は本文だけを読み上げる(レス番号・名前は読まない)
            read_res_number: false,
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
            display_per_char_ms: 80,
            display_max_ms: 12000,
            wait_for_display: true,
            aa_display_ms: 6000,
            aa_enabled: true,
            aa_threshold_chars: 300,
            aa_patterns: ["д", "（●）", "从", "）)ﾉヽ", "∀", "<●>", "（__人__）"].join("\n"),
            aa_read_quoted_only: true,
            aa_read_res_number: false,
            aa_fixed_reading: "アスキーアート".into(),
            desktop_enabled: true,
            desktop_monitor_id: None,
            // 画面の一番下だとタスクバーに重なるので、少し上(70%〜92%)を既定にする
            desktop_x: 0.0,
            desktop_y: 0.70,
            desktop_width: 1.0,
            desktop_height: 0.22,
            desktop_rect: None,
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
    /// 配信サイト(YouTube・Twitch)ごとの状態
    pub sources: Vec<SourceStatus>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    /// "youtube" | "twitch"
    pub id: String,
    pub label: String,
    /// "ok"(取得中) | "wait"(配信待ち・再接続中) | "error"
    pub state: String,
    pub message: String,
}

#[derive(Clone)]
struct Post {
    no: u32,
    name: String,
    date: String,
    body: String,
    /// ReaCast自身からのお知らせ(次スレ移動・スレ建て警告・テスト表示)
    system: bool,
    /// 右のレス一覧に出すアイコン画像のURL(obs_bridgeが配信する)
    icon: Option<String>,
    /// 取得元: "bbs" | "youtube" | "twitch" | "system"
    source: &'static str,
}

impl Post {
    fn from_raw(p: &source::RawPost, icon: Option<String>) -> Self {
        Self {
            no: p.no,
            name: text::html_to_text(&p.name),
            date: p.date.clone(),
            body: text::html_to_text(&p.body_html),
            system: false,
            icon,
            source: "bbs",
        }
    }

    fn system(body: impl Into<String>) -> Self {
        Self {
            no: 0,
            name: "ReaCast".into(),
            date: String::new(),
            body: body.into(),
            system: true,
            icon: None,
            source: "system",
        }
    }

    /// 配信サイトのチャット(レス番号は無い)
    fn chat(source: &'static str, name: String, body: String, icon: Option<String>) -> Self {
        Self { no: 0, name, date: String::new(), body, system: false, icon, source }
    }

    fn to_json(&self, aa: bool) -> serde_json::Value {
        serde_json::json!({
            "no": self.no,
            "name": self.name,
            "date": self.date,
            "body": self.body,
            "aa": aa,
            "system": self.system,
            "icon": self.icon,
            "source": self.source,
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

fn detect_aa(s: &CommentSettings, c: &Compiled, post: &Post) -> bool {
    s.aa_enabled && !post.system && text::is_aa(&post.body, s.aa_threshold_chars, &c.aa_patterns)
}

struct State {
    settings: CommentSettings,
    compiled: Arc<Compiled>,
    /// start/stopのたびに増やす。古い取得タスクはこれが変わったら終了する。
    run_id: u64,
    queue: VecDeque<Post>,
    /// 今読んでいるスレッドの全レス(設定ウィンドウで過去のレスを遡る用)
    thread_posts: Vec<Post>,
    status: CommentStatus,
    /// デスクトップ字幕をマウスで動かしている最中か
    adjusting: bool,
    /// テスト表示のために、取得していなくてもデスクトップ字幕を出しておく期限
    desktop_hold_until: Option<std::time::Instant>,
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
                thread_posts: Vec::new(),
                status: CommentStatus::default(),
                adjusting: false,
                desktop_hold_until: None,
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

    pub fn apply_settings(&self, settings: CommentSettings) {
        let compiled = Arc::new(Compiled::from(&settings));
        let style = settings.style.clone();
        let icon_folders = [
            ("bbs", settings.icon_folder_bbs.clone()),
            ("youtube", settings.icon_folder_youtube.clone()),
            ("twitch", settings.icon_folder_twitch.clone()),
        ];
        {
            let mut st = self.state.lock().unwrap();
            st.settings = settings;
            st.compiled = compiled;
        }
        let bridge = self.bridge();
        bridge.set_comment_style(style);
        for (source, folder) in &icon_folders {
            bridge.set_icon_folder(source, folder);
        }
    }

    /// デスクトップ字幕のウィンドウを今出しておくべきか(ONで、かつ取得中・
    /// 位置調整中・テスト表示の直後のいずれか)。
    pub fn desktop_wanted(&self) -> bool {
        let st = self.state.lock().unwrap();
        let held = st.desktop_hold_until.map(|t| t > std::time::Instant::now()).unwrap_or(false);
        st.settings.desktop_enabled && (st.status.running || st.adjusting || held)
    }

    pub fn adjusting(&self) -> bool {
        self.state.lock().unwrap().adjusting
    }

    pub fn set_adjusting(&self, on: bool) {
        self.state.lock().unwrap().adjusting = on;
    }

    pub fn hold_desktop(&self, dur: Duration) {
        self.state.lock().unwrap().desktop_hold_until = Some(std::time::Instant::now() + dur);
    }

    fn pick_icon(&self, source: &str) -> Option<String> {
        use rand::Rng;
        let n = self.bridge().icon_count(source);
        (n > 0).then(|| format!("/icons/{source}/{}", rand::thread_rng().gen_range(0..n)))
    }

    /// 次スレのタイトルと>>1の本文を、設定のテンプレートから作る。
    pub fn next_thread_draft(&self) -> Result<(String, String), String> {
        let (s, status) = {
            let st = self.state.lock().unwrap();
            (st.settings.clone(), st.status.clone())
        };
        let cur_title = status.thread_title.ok_or("先に取得を開始してください(今のスレッドのタイトルが必要です)")?;
        let tmpl = if s.create_title_template.trim().is_empty() { "{next}" } else { s.create_title_template.as_str() };
        let next = text::increment_last_number(&cur_title);
        if (tmpl.contains("{next}") || tmpl.contains("{n}")) && next.is_none() {
            return Err(format!(
                "今のタイトル「{cur_title}」に数字が無いため、次のタイトルを自動で決められません(タイトルの設定を変えてください)"
            ));
        }
        let next = next.unwrap_or_default();
        let title = tmpl.replace("{next}", &next).replace("{n}", &text::last_number(&next).unwrap_or_default());
        let body = s
            .create_body_template
            .replace("{prevUrl}", status.thread_url.as_deref().unwrap_or(""))
            .replace("{prevTitle}", &cur_title)
            .replace("{title}", &title);
        if title.trim().is_empty() {
            return Err("次スレのタイトルが空です".into());
        }
        if body.trim().is_empty() {
            return Err(">>1の本文が空です".into());
        }
        Ok((title, body))
    }

    /// 取得を開始する。掲示板(urlが空なら取得しない)と、設定にあるYouTube・Twitchを
    /// それぞれ別のタスクで同時に取得し、すべて同じ順番待ちの列に並べる。
    pub fn start(self: &Arc<Self>, url: &str) -> Result<(), String> {
        let settings = self.settings();
        let bbs = if url.trim().is_empty() { None } else { Some(source::parse_url(url)?) };
        let youtube = if settings.youtube_target.trim().is_empty() {
            None
        } else {
            Some(youtube::parse_target(&settings.youtube_target)?)
        };
        let twitch = if settings.twitch_channel.trim().is_empty() {
            None
        } else {
            Some(twitch::parse_channel(&settings.twitch_channel)?)
        };
        if bbs.is_none() && youtube.is_none() && twitch.is_none() {
            return Err("掲示板のURL、YouTube、Twitchのどれかを指定してください".into());
        }
        let mut sources = Vec::new();
        if youtube.is_some() {
            sources.push(SourceStatus { id: "youtube".into(), label: "YouTube".into(), state: "wait".into(), message: "接続中…".into() });
        }
        if twitch.is_some() {
            sources.push(SourceStatus { id: "twitch".into(), label: "Twitch".into(), state: "wait".into(), message: "接続中…".into() });
        }
        let run_id = {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.thread_posts.clear();
            st.settings.thread_url = url.trim().to_string();
            st.status = CommentStatus { running: true, sources, ..Default::default() };
            st.run_id
        };
        self.emit_status();
        self.emit_thread_reset();
        // 開始(再開始)のたびに右のレス一覧を空にしてから始める
        self.bridge().clear_comments();
        let initial = self.settings().initial_text;
        if !initial.trim().is_empty() {
            self.bridge().push_comment(Post::system(initial).to_json(false));
        }
        if let Some((board, key)) = bbs {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.run_fetch(run_id, board, key).await });
        }
        if let Some(target) = youtube {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.run_youtube(run_id, target).await });
        }
        if let Some(channel) = twitch {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.run_twitch(run_id, channel).await });
        }
        Ok(())
    }

    fn update_source(&self, run_id: u64, id: &str, state: &str, message: impl Into<String>) {
        let message = message.into();
        self.update_status(run_id, |s| {
            if let Some(src) = s.sources.iter_mut().find(|x| x.id == id) {
                src.state = state.into();
                src.message = message;
            }
        });
    }

    /// 停止されるまで待つ(1秒ごとに止められていないか確認する)。止められたらfalse。
    async fn sleep_while_current(&self, run_id: u64, dur: Duration) -> bool {
        let end = tokio::time::Instant::now() + dur;
        while tokio::time::Instant::now() < end {
            if !self.is_current(run_id) {
                return false;
            }
            let left = end - tokio::time::Instant::now();
            tokio::time::sleep(left.min(Duration::from_secs(1))).await;
        }
        self.is_current(run_id)
    }

    async fn run_youtube(self: Arc<Self>, run_id: u64, target: youtube::Target) {
        let client = youtube::client();
        'outer: while self.is_current(run_id) {
            // 1. 動画IDを決める(チャンネルなら、配信が始まるまで待つ)
            let video_id = match &target {
                youtube::Target::Video(id) => id.clone(),
                youtube::Target::Channel(url) => match youtube::get_text(&client, &format!("{url}/live")).await {
                    Ok(html) => match youtube::extract_live_video_id(&html) {
                        Some(id) => id,
                        None => {
                            self.update_source(run_id, "youtube", "wait", "配信が始まるのを待っています");
                            if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                                return;
                            }
                            continue;
                        }
                    },
                    Err(e) => {
                        self.update_source(run_id, "youtube", "error", e);
                        if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                            return;
                        }
                        continue;
                    }
                },
            };
            // 2. チャット欄のページから、続きを取得するためのトークンを取り出す
            let params = match youtube::get_text(&client, &format!("https://www.youtube.com/live_chat?is_popout=1&v={video_id}")).await {
                Ok(html) => youtube::extract_chat_params(&html),
                Err(e) => {
                    self.update_source(run_id, "youtube", "error", e);
                    None
                }
            };
            let Some(params) = params else {
                self.update_source(run_id, "youtube", "wait", "チャットを読み込めません(配信前・終了後、またはチャットが無効)");
                if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                    return;
                }
                continue;
            };
            self.update_source(run_id, "youtube", "ok", format!("取得中(動画 {video_id})"));
            // 3. 新着コメントを取り続ける(最初のページにある過去のコメントは読まない)
            let mut continuation = params.continuation.clone();
            let mut failures = 0;
            loop {
                if !self.is_current(run_id) {
                    return;
                }
                match youtube::fetch_chat(&client, &params, &continuation).await {
                    Ok(page) => {
                        failures = 0;
                        let posts: Vec<Post> = page
                            .messages
                            .into_iter()
                            .map(|m| {
                                let body = match m.amount {
                                    Some(a) if m.text.is_empty() => a,
                                    Some(a) => format!("{a} {}", m.text),
                                    None => m.text,
                                };
                                Post::chat("youtube", m.author, body, self.pick_icon("youtube"))
                            })
                            .collect();
                        self.enqueue(run_id, posts);
                        match page.next {
                            Some((c, timeout)) => {
                                continuation = c;
                                let wait = Duration::from_millis(timeout.clamp(1500, 5000));
                                if !self.sleep_while_current(run_id, wait).await {
                                    return;
                                }
                            }
                            None => {
                                self.update_source(run_id, "youtube", "wait", "配信が終了しました。次の配信を待っています");
                                if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                                    return;
                                }
                                continue 'outer;
                            }
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        self.update_source(run_id, "youtube", "error", e);
                        if !self.sleep_while_current(run_id, Duration::from_secs(5)).await {
                            return;
                        }
                        if failures >= 3 {
                            continue 'outer; // 最初からやり直す
                        }
                    }
                }
            }
        }
    }

    async fn run_twitch(self: Arc<Self>, run_id: u64, channel: String) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        while self.is_current(run_id) {
            let conn = tokio_tungstenite::connect_async("wss://irc-ws.chat.twitch.tv:443").await;
            let (mut write, mut read) = match conn {
                Ok((ws, _)) => ws.split(),
                Err(e) => {
                    self.update_source(run_id, "twitch", "error", format!("Twitchへの接続に失敗しました: {e}"));
                    if !self.sleep_while_current(run_id, Duration::from_secs(10)).await {
                        return;
                    }
                    continue;
                }
            };
            let nick = format!("justinfan{}", rand::random::<u32>() % 90000 + 10000);
            for line in [
                "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
                "PASS SCHMOOPIIE".to_string(),
                format!("NICK {nick}"),
                format!("JOIN #{channel}"),
            ] {
                let _ = write.send(Message::Text(line)).await;
            }
            self.update_source(run_id, "twitch", "ok", format!("取得中(#{channel})"));
            loop {
                if !self.is_current(run_id) {
                    let _ = write.close().await;
                    return;
                }
                // 1秒ごとに止められていないか確認しつつ読む
                let msg = match tokio::time::timeout(Duration::from_secs(1), read.next()).await {
                    Err(_) => continue,
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(m))) => m,
                };
                let Message::Text(text) = msg else { continue };
                let mut posts = Vec::new();
                let mut reconnect = false;
                for line in text.split("\r\n").filter(|l| !l.is_empty()) {
                    if let Some(rest) = line.strip_prefix("PING") {
                        let _ = write.send(Message::Text(format!("PONG{rest}"))).await;
                    } else if line.contains(" RECONNECT") {
                        // Twitch側からの「繋ぎ直してください」の合図
                        reconnect = true;
                    } else if let Some(m) = twitch::parse_privmsg(line) {
                        posts.push(Post::chat("twitch", m.author, m.text, self.pick_icon("twitch")));
                    }
                }
                self.enqueue(run_id, posts);
                if reconnect {
                    break;
                }
            }
            self.update_source(run_id, "twitch", "wait", "接続が切れました。再接続しています");
            if !self.sleep_while_current(run_id, Duration::from_secs(3)).await {
                return;
            }
        }
    }

    pub fn stop(&self) {
        {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.thread_posts.clear();
            st.status = CommentStatus::default();
        }
        self.bridge().clear_subtitle();
        self.emit_status();
        self.emit_thread_reset();
    }

    fn emit_thread_reset(&self) {
        let _ = self.app.emit("comments:posts", serde_json::json!({ "reset": true, "posts": [] }));
    }

    /// 今のスレッドの全レス(設定ウィンドウの「スレッドのレス」タブ用)
    pub fn thread_posts_json(&self) -> serde_json::Value {
        let st = self.state.lock().unwrap();
        let posts: Vec<serde_json::Value> = st
            .thread_posts
            .iter()
            .map(|p| p.to_json(detect_aa(&st.settings, &st.compiled, p)))
            .collect();
        serde_json::json!({ "posts": posts })
    }

    /// 指定したレス番号のレスを、順番待ちの先頭に入れてもう一度表示・読み上げる。
    pub fn replay(&self, no: u32) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let post = st.thread_posts.iter().find(|p| p.no == no).cloned().ok_or("そのレスが見つかりません")?;
        st.queue.push_front(post);
        drop(st);
        self.wake.notify_one();
        Ok(())
    }

    /// 取得したレスをスレッドの全レス一覧に追加し、設定ウィンドウにも知らせる。
    fn record_posts(&self, run_id: u64, posts: &[Post], reset: bool) {
        let json = {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            if reset {
                st.thread_posts.clear();
            }
            st.thread_posts.extend(posts.iter().cloned());
            let (settings, compiled) = (st.settings.clone(), st.compiled.clone());
            posts
                .iter()
                .map(|p| p.to_json(detect_aa(&settings, &compiled, p)))
                .collect::<Vec<_>>()
        };
        if reset || !json.is_empty() {
            let _ = self.app.emit("comments:posts", serde_json::json!({ "reset": reset, "posts": json }));
        }
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
        let mut created = false;
        loop {
            if !self.is_current(run_id) {
                return;
            }
            match reader.poll(&client).await {
                Ok(raw) => {
                    let posts: Vec<Post> = raw.iter().map(|p| Post::from_raw(p, self.pick_icon("bbs"))).collect();
                    self.record_posts(run_id, &posts, false);
                    if first {
                        // 起動時点で既にあったレスは読み上げない(右のレス一覧にだけ載せる)
                        first = false;
                        let compiled = self.state.lock().unwrap().compiled.clone();
                        let settings = self.settings();
                        // 開始レス番号が指定されている時だけ、それ以降を右のレス一覧に載せる
                        // (既定では載せず、「スレッド読み込みを開始しました」だけを出す)
                        let shown: Vec<&Post> = if settings.start_res_no > 0 {
                            posts.iter().filter(|p| p.no >= settings.start_res_no).collect()
                        } else {
                            Vec::new()
                        };
                        for p in shown {
                            if !text::contains_ng(&p.name, &p.body, &compiled.ng_words) {
                                self.bridge().push_comment(p.to_json(detect_aa(&settings, &compiled, p)));
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
                    if settings.auto_create_thread
                        && !created
                        && settings.create_thread_count > 0
                        && reader.last_no >= settings.create_thread_count
                        && reader.last_no < 1000
                    {
                        // 次スレを1回だけ建てる(既に誰かが建てていれば建てない)
                        created = true;
                        warned = true;
                        let msg = match source::list_threads(&client, &board).await {
                            Ok(list) => match source::pick_next(&list, &reader.thread.key, &settings.next_thread_keyword) {
                                Some(t) => format!("次スレ「{}」は既に建っています", t.title),
                                None => match self.next_thread_draft() {
                                    Ok((title, body)) => match source::create_thread(
                                        &client,
                                        &board,
                                        &title,
                                        &settings.create_name,
                                        &settings.create_mail,
                                        &body,
                                    )
                                    .await
                                    {
                                        Ok(()) => format!("次スレ「{title}」を建てました"),
                                        Err(e) => e,
                                    },
                                    Err(e) => e,
                                },
                            },
                            Err(e) => e,
                        };
                        self.update_status(run_id, |s| s.warning = Some(msg.clone()));
                        self.enqueue(run_id, vec![Post::system(msg)]);
                    }

                    if !settings.auto_create_thread
                        && !warned
                        && settings.thread_warn_count > 0
                        && reader.last_no >= settings.thread_warn_count
                        && reader.last_no < 1000
                    {
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
                                    self.record_posts(run_id, &[], true);
                                    warned = false;
                                    created = false;
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
            let aa = detect_aa(&settings, &compiled, &post);
            let bridge = self.bridge();
            let shown_ms = display_duration(&post, aa, &settings);
            bridge.show_subtitle(post.to_json(aa), shown_ms);
            bridge.push_comment(post.to_json(aa));

            if settings.speech_enabled && !settings.command_path.trim().is_empty() {
                let spoken = speech_text(&post, aa, &settings, &compiled);
                if !spoken.trim().is_empty() {
                    run_speech_command(&settings, &post, &spoken);
                }
            }

            let interval = next_interval(backlog, shown_ms, &settings);
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    }
}

/// 字幕を表示しておく時間。通常のレスは文字数に応じて延ばす(上限あり)。
/// AAは「AAモードで字幕が消える時間」で固定。
fn display_duration(post: &Post, aa: bool, s: &CommentSettings) -> u64 {
    if aa {
        return s.aa_display_ms;
    }
    let chars = post.body.chars().filter(|c| !c.is_whitespace()).count() as u64;
    let extended = s.display_ms.saturating_add(chars.saturating_mul(s.display_per_char_ms));
    if s.display_per_char_ms == 0 {
        s.display_ms
    } else {
        extended.min(s.display_max_ms.max(s.display_ms))
    }
}

/// 次のレスを出すまでの間隔。溜まっている時(ターボ)は短い間隔で追いつくことを
/// 優先し、そうでなければ読み上げ間隔と(設定がONなら)字幕の表示時間の長い方。
fn next_interval(backlog: usize, shown_ms: u64, s: &CommentSettings) -> u64 {
    if backlog >= s.turbo_threshold.max(1) {
        s.turbo_interval_ms
    } else if s.wait_for_display {
        s.read_interval_ms.max(shown_ms)
    } else {
        s.read_interval_ms
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
pub fn comments_get_thread(engine: Engine) -> serde_json::Value {
    engine.thread_posts_json()
}

#[tauri::command]
pub fn comments_replay(engine: Engine, no: u32) -> Result<(), String> {
    engine.replay(no)
}

/// テスト表示。取得していなくてもデスクトップ字幕で確認できるよう、表示が
/// 終わるまでの間だけ字幕ウィンドウを作っておく。
#[tauri::command(async)]
pub fn comments_test(app: AppHandle, engine: Engine, text: String) {
    let hold = Duration::from_millis(engine.settings().display_ms.max(1000) + 4000);
    let created = app.get_webview_window("subtitle").is_none();
    engine.hold_desktop(hold);
    crate::sync_subtitle_window(&app);
    if created && app.get_webview_window("subtitle").is_some() {
        // 作った直後は画面の読み込みが終わっていないので、少し待ってから表示する
        // (読み込みが遅れても、繋いできた時点で表示中の字幕は送り直される)
        std::thread::sleep(Duration::from_millis(600));
    }
    engine.test(&text);
    std::thread::spawn(move || {
        std::thread::sleep(hold + Duration::from_millis(200));
        crate::sync_subtitle_window(&app);
    });
}

/// 右のレス一覧・ニコ生風の表示だけに、見た目の確認用のレスを出す
/// (字幕・読み上げはしない)。
#[tauri::command]
pub fn comments_test_list(engine: Engine, text: String, name: String) {
    let mut post = Post::system(text);
    post.system = false;
    post.no = 1;
    post.name = name;
    post.date = "2026/10/08(木) 21:00:00.00 ID:test".into();
    post.icon = engine.pick_icon("bbs");
    engine.bridge().push_comment(post.to_json(false));
}

/// YouTubeのURLで、配信とチャットに接続できるかを確かめる(取得は開始しない)。
#[tauri::command]
pub async fn comments_check_youtube(target: String) -> youtube::CheckResult {
    youtube::check(&target).await
}

/// Twitchのチャンネルに接続できるかを確かめる(数秒だけ繋いでコメントを受け取る)。
#[tauri::command]
pub async fn comments_check_twitch(channel: String) -> serde_json::Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let channel = match twitch::parse_channel(&channel) {
        Ok(c) => c,
        Err(e) => return serde_json::json!({ "ok": false, "message": e, "samples": [] }),
    };
    let (mut write, mut read) = match tokio_tungstenite::connect_async("wss://irc-ws.chat.twitch.tv:443").await {
        Ok((ws, _)) => ws.split(),
        Err(e) => return serde_json::json!({ "ok": false, "message": format!("Twitchへの接続に失敗しました: {e}"), "samples": [] }),
    };
    for line in [
        "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
        "PASS SCHMOOPIIE".to_string(),
        format!("NICK justinfan{}", rand::random::<u32>() % 90000 + 10000),
        format!("JOIN #{channel}"),
    ] {
        let _ = write.send(Message::Text(line)).await;
    }
    let mut joined = false;
    let mut samples: Vec<String> = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < end {
        let left = end - tokio::time::Instant::now();
        let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(left, read.next()).await else {
            break;
        };
        for line in text.split("\r\n") {
            if line.contains(&format!(" JOIN #{channel}")) {
                joined = true;
            } else if let Some(m) = twitch::parse_privmsg(line) {
                if samples.len() < 5 {
                    samples.push(format!("{}: {}", m.author, m.text));
                }
            }
        }
    }
    let _ = write.close().await;
    let message = match (joined, samples.len()) {
        (false, _) => "チャンネルに参加できませんでした".to_string(),
        (true, 0) => "接続できました(8秒の間にコメントはありませんでした。配信していないか、コメントが少ない可能性があります)".to_string(),
        (true, n) => format!("接続できました(8秒の間に{n}件以上のコメントを受信)"),
    };
    serde_json::json!({ "ok": joined, "message": message, "channel": channel, "samples": samples })
}

/// 次スレのタイトルと>>1の本文を確認する(実際には建てない)。
#[tauri::command]
pub fn comments_preview_next_thread(engine: Engine) -> Result<serde_json::Value, String> {
    let (title, body) = engine.next_thread_draft()?;
    Ok(serde_json::json!({ "title": title, "body": body }))
}

#[tauri::command]
pub fn comments_desktop_adjusting(engine: Engine) -> bool {
    engine.adjusting()
}

/// デスクトップ字幕をマウスで動かすモードの開始/終了。終了した時点の
/// ウィンドウの位置・大きさを保存する。
#[tauri::command(async)]
pub fn comments_desktop_adjust(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    on: bool,
) -> Result<(), String> {
    if !on {
        if let Some(win) = app.get_webview_window("subtitle") {
            if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
                let mut s = engine.settings();
                s.desktop_rect = Some(DesktopRect { x: pos.x, y: pos.y, width: size.width, height: size.height });
                save_settings(&store, &s)?;
                engine.apply_settings(s.clone());
                let _ = app.emit("comments:settings", &s);
            }
        }
    }
    engine.set_adjusting(on);
    crate::sync_subtitle_window(&app);
    let _ = app.emit("subtitle:adjust", on);
    Ok(())
}

/// デスクトップ字幕の位置を、モニター・割合の指定(既定の位置)に戻す。
#[tauri::command(async)]
pub fn comments_desktop_reset(app: AppHandle, engine: Engine, store: tauri::State<ConfigStore>) -> Result<(), String> {
    let mut s = engine.settings();
    s.desktop_rect = None;
    save_settings(&store, &s)?;
    engine.apply_settings(s.clone());
    let _ = app.emit("comments:settings", &s);
    crate::sync_subtitle_window(&app);
    Ok(())
}

/// 読み上げソフト(exe)を選ぶダイアログ。
#[tauri::command(async)]
pub fn comments_pick_exe(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("読み上げソフトを選択")
        .add_filter("実行ファイル", &["exe"])
        .blocking_pick_file()
        .map(|p| p.to_string())
}

/// アイコン画像のフォルダを選ぶダイアログ。
#[tauri::command(async)]
pub fn comments_pick_folder(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("アイコン画像のフォルダを選択")
        .blocking_pick_folder()
        .map(|p| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(no: u32, body: &str) -> Post {
        Post { no, name: "名無し".into(), date: String::new(), body: body.into(), system: false, icon: None, source: "bbs" }
    }

    #[test]
    fn speech_text_for_normal_and_aa_posts() {
        let mut s = CommentSettings::default();
        let c = Compiled::from(&s);
        // 既定は本文だけ(レス番号は読まない)
        assert_eq!(speech_text(&post(69, "おつかれ www"), false, &s, &c), "おつかれ ワラワラ");
        assert_eq!(speech_text(&post(5, "(´Д｀)「やあ」"), true, &s, &c), "やあ");
        assert_eq!(speech_text(&post(5, "(´Д｀)"), true, &s, &c), "アスキーアート");
        s.read_res_number = true;
        assert_eq!(speech_text(&post(69, "おつかれ"), false, &s, &c), "69、おつかれ");
        s.read_res_number = false;
        s.max_chars = 4;
        assert_eq!(speech_text(&post(1, "あいうえおか"), false, &s, &c), "あいうえ");
        // お知らせにはレス番号を付けない
        assert_eq!(speech_text(&Post::system("次スレです"), false, &s, &c), "次スレで");
    }

    #[test]
    fn long_posts_stay_longer_and_hold_the_queue() {
        let s = CommentSettings::default(); // 3000ms + 80ms/文字、上限12000ms
        assert_eq!(display_duration(&post(1, "おつ"), false, &s), 3160);
        assert_eq!(display_duration(&post(1, &"あ".repeat(50)), false, &s), 7000);
        assert_eq!(display_duration(&post(1, &"あ".repeat(500)), false, &s), 12000);
        assert_eq!(display_duration(&post(1, &"あ".repeat(500)), true, &s), 6000);
        // 普段は表示し終わるまで待つ(短いレスは読み上げ間隔3000ms)
        assert_eq!(next_interval(0, 3160, &s), 3160);
        assert_eq!(next_interval(0, 1000, &s), 3000);
        // 溜まっている時はターボ間隔
        assert_eq!(next_interval(3, 12000, &s), 400);
        let mut s2 = s.clone();
        s2.wait_for_display = false;
        s2.display_per_char_ms = 0;
        assert_eq!(display_duration(&post(1, &"あ".repeat(50)), false, &s2), 3000);
        assert_eq!(next_interval(0, 7000, &s2), 3000);
    }

    #[test]
    fn settings_fill_defaults_for_missing_fields() {
        let s: CommentSettings = serde_json::from_value(serde_json::json!({ "threadUrl": "x" })).unwrap();
        assert_eq!(s.thread_url, "x");
        assert_eq!(s.read_interval_ms, 3000);
        assert_eq!(s.command_args, "#Res#");
    }
}
