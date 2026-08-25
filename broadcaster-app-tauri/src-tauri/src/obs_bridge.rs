//! OBSなどの外部ツールから、配信者アプリのオーバーレイと同じ内容を直接
//! 取り込めるようにするための、ローカル専用の橋渡し役。
//!
//! 経緯: OBSの「ウィンドウキャプチャ」でTauriのオーバーレイウィンドウ(透明・
//! タスクバー非表示)を直接キャプチャしようとすると、実機のテストで
//! 透明部分が正しく抜けずデスクトップが映り込んでしまう相性問題が見つかった
//! (「ゲームキャプチャ」でも解決しなかった)。これはWindowsのウィンドウ
//! キャプチャ関連APIと、タスクバー非表示・透明ウィンドウの組み合わせでよく
//! 起きる既知の問題。
//!
//! 回避策として、世の中の配信オーバーレイツール(StreamElements等の
//! アラート機能)と同じ「OBSの『ブラウザ』ソースにWebページとして直接
//! 読み込ませる」方式を用意する。OBSのブラウザソースはCEF(Chromium)で
//! 直接描画するため、透明背景がウィンドウキャプチャのような相性問題なく
//! 正しく扱える。
//!
//! 具体的には、127.0.0.1限定(外部には一切公開しない)で
//!   - HTTPサーバー(OBS_HTTP_PORT): overlay.html / overlay.js / emoji-set.js を
//!     そのまま配信する(Tauriのオーバーレイウィンドウが使っているのと
//!     完全に同じファイル。二重管理を避けるためinclude_str!で埋め込んで
//!     使い回している)
//!   - WebSocketサーバー(OBS_WS_PORT): リアクションが発生するたびに、
//!     繋がっている全クライアント(OBSのブラウザソース)へ配信する
//! の2つをこのアプリ自身の中で立てる。overlay.js側は、Tauriの中で開かれて
//! いる(window.__TAURI__が使える)時は従来通りTauriのイベントを使い、
//! そうでない(=OBSのブラウザソースとして開かれた)時だけこのWebSocketに
//! 直接繋ぎに行くよう分岐している。
//!
//! OBS側の設定: 「ブラウザ」ソースを追加し、URLに
//! `http://127.0.0.1:18772/` を指定するだけでよい(ローカルファイルではなく
//! URLとして指定する)。

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

pub const OBS_HTTP_PORT: u16 = 18772;
pub const OBS_WS_PORT: u16 = 18771;

const OVERLAY_HTML: &str = include_str!("../../frontend/overlay.html");
const OVERLAY_JS: &str = include_str!("../../frontend/overlay.js");
const EMOJI_SET_JS: &str = include_str!("../../frontend/shared/emoji-set.js");

/// 設定パネルの「表示関連」設定のうち、Taur本体のオーバーレイとOBS側の両方に
/// 反映する必要があるもの一式(絵文字の大きさ・透明度・連打で大きくなる仕様の
/// ON/OFF)。まとめて1つの"settings"メッセージとして配信する。
// noComboGrowthIds(Vec<String>)を持つため、以前の#[derive(Copy)]は
// 外してある(呼び出し側は.clone()する。下記参照)。
#[derive(Clone)]
struct BridgeSettings {
    glyph_scale: f64,
    glyph_opacity: f64,
    combo_growth_enabled: bool,
    // 「大きくしない」に個別指定されているリアクションIDの一覧。
    no_combo_growth_ids: Vec<String>,
}

impl BridgeSettings {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "settings",
            "glyphScale": self.glyph_scale,
            "glyphOpacity": self.glyph_opacity,
            "comboGrowthEnabled": self.combo_growth_enabled,
            "noComboGrowthIds": self.no_combo_growth_ids,
        })
    }
}

#[derive(Clone)]
pub struct ObsBridge {
    tx: broadcast::Sender<String>,
    // 現在値を保持しておく。broadcast channelは後から繋いできたクライアントに
    // 過去のメッセージを配ってくれないため、OBS側でブラウザソースを開き直した
    // 時などに、繋いだ直後の1回だけこの値を直接読んで送るために使う
    // (handle_ws_connection参照)。
    settings: Arc<Mutex<BridgeSettings>>,
}

impl ObsBridge {
    /// リアクション発生時に呼ぶ。OBS側のブラウザソースが繋がっていれば
    /// そちらにも同じリアクションを配信する(誰も繋いでいなくてもエラーには
    /// ならず、単に何も起きない)。
    pub fn broadcast_reaction(&self, emoji: &str, viewer_id: &str) {
        let payload =
            serde_json::json!({ "type": "reaction", "emoji": emoji, "viewerId": viewer_id })
                .to_string();
        let _ = self.tx.send(payload);
    }

    /// 設定パネルで「絵文字の大きさ」が変更された時に呼ぶ。
    pub fn set_glyph_scale(&self, scale: f64) {
        self.update_and_broadcast(|s| s.glyph_scale = scale);
    }

    /// 設定パネルで「スタンプの透明度」が変更された時に呼ぶ。
    pub fn set_glyph_opacity(&self, opacity: f64) {
        self.update_and_broadcast(|s| s.glyph_opacity = opacity);
    }

    /// 設定パネルで「連打で大きくなる」のON/OFFが変更された時に呼ぶ。
    pub fn set_combo_growth_enabled(&self, enabled: bool) {
        self.update_and_broadcast(|s| s.combo_growth_enabled = enabled);
    }

    /// 設定パネルで「大きくしない」個別リアクションのチェック状態が
    /// 変更された時に呼ぶ(IDの配列を丸ごと置き換える)。
    pub fn set_no_combo_growth_ids(&self, ids: Vec<String>) {
        self.update_and_broadcast(|s| s.no_combo_growth_ids = ids);
    }

    fn update_and_broadcast(&self, apply: impl FnOnce(&mut BridgeSettings)) {
        let payload = {
            let mut s = self.settings.lock().unwrap();
            apply(&mut s);
            s.to_json().to_string()
        };
        let _ = self.tx.send(payload);
    }
}

/// HTTP・WebSocketの両サーバーをバックグラウンドで起動する。ポートが
/// 既に使われている等の理由で起動に失敗しても、ログを出すだけでアプリ本体は
/// 問題なく動き続ける(OBS連携が使えなくなるだけで、通常のTauriオーバーレイ
/// 表示には一切影響しない)。
/// initial_*: 起動時点でConfigStoreに保存されている値(未設定なら既定値)。
/// OBS側が最初に繋いだ時点からこれらの値を反映できるように、呼び出し側
/// (lib.rsのsetup())から渡してもらう。
pub fn start(
    initial_glyph_scale: f64,
    initial_glyph_opacity: f64,
    initial_combo_growth_enabled: bool,
    initial_no_combo_growth_ids: Vec<String>,
) -> ObsBridge {
    let (tx, _rx) = broadcast::channel::<String>(64);
    let settings = Arc::new(Mutex::new(BridgeSettings {
        glyph_scale: initial_glyph_scale,
        glyph_opacity: initial_glyph_opacity,
        combo_growth_enabled: initial_combo_growth_enabled,
        no_combo_growth_ids: initial_no_combo_growth_ids,
    }));
    let bridge = ObsBridge {
        tx: tx.clone(),
        settings: settings.clone(),
    };

    tauri::async_runtime::spawn(run_http_server());
    tauri::async_runtime::spawn(run_ws_server(tx, settings));

    bridge
}

async fn run_http_server() {
    let listener = match TcpListener::bind(("127.0.0.1", OBS_HTTP_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            log::warn!(
                "OBS連携用HTTPサーバーの起動に失敗しました({OBS_HTTP_PORT}番ポート): {e}"
            );
            return;
        }
    };
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        tauri::async_runtime::spawn(handle_http_connection(socket));
    }
}

async fn handle_http_connection(mut socket: TcpStream) {
    let mut buf = [0u8; 2048];
    // GETリクエストの1行目(パス)だけ分かれば十分なので、最初に読めた分だけ見る
    // (このサーバーにボディ付きのリクエストが来ることは想定していない)。
    let n = match socket.read(&mut buf).await {
        Ok(n) => n,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (content_type, body): (&str, &str) = match path {
        "/" | "/overlay.html" => ("text/html; charset=utf-8", OVERLAY_HTML),
        "/overlay.js" => ("text/javascript; charset=utf-8", OVERLAY_JS),
        "/shared/emoji-set.js" => ("text/javascript; charset=utf-8", EMOJI_SET_JS),
        _ => ("text/plain; charset=utf-8", "not found"),
    };
    let status = if body == "not found" {
        "404 Not Found"
    } else {
        "200 OK"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn run_ws_server(tx: broadcast::Sender<String>, settings: Arc<Mutex<BridgeSettings>>) {
    let listener = match TcpListener::bind(("127.0.0.1", OBS_WS_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            log::warn!(
                "OBS連携用WebSocketサーバーの起動に失敗しました({OBS_WS_PORT}番ポート): {e}"
            );
            return;
        }
    };
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        let rx = tx.subscribe();
        let initial_settings = settings.lock().map(|s| s.clone()).unwrap_or(BridgeSettings {
            glyph_scale: 1.0,
            glyph_opacity: 1.0,
            combo_growth_enabled: true,
            no_combo_growth_ids: Vec::new(),
        });
        tauri::async_runtime::spawn(handle_ws_connection(socket, rx, initial_settings));
    }
}

async fn handle_ws_connection(
    socket: TcpStream,
    mut rx: broadcast::Receiver<String>,
    initial_settings: BridgeSettings,
) {
    let ws_stream = match tokio_tungstenite::accept_async(socket).await {
        Ok(s) => s,
        Err(_) => return,
    };
    use futures_util::SinkExt;
    let (mut write, _read) = futures_util::StreamExt::split(ws_stream);
    // 繋いだ直後に、今の設定を1回送っておく。broadcast channelは過去の
    // メッセージを新規クライアントに配ってくれないため、これが無いとOBS側で
    // ブラウザソースを開き直すたびに既定値に戻って見えてしまう。
    let initial_payload = initial_settings.to_json().to_string();
    if write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            initial_payload,
        ))
        .await
        .is_err()
    {
        return;
    }
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if write
                    .send(tokio_tungstenite::tungstenite::Message::Text(msg))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}
