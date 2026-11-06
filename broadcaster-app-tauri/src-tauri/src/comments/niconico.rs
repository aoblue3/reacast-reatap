//! ニコニコ生放送のコメント取得(ログイン不要)。
//!
//! 2024年8月の作り直し(「帰ってきたニコニコ」)から、コメントは新しいメッセージサーバー
//! (NDGR)から、protobufの形で届くようになった。ブラウザの視聴ページと同じ手順で読む:
//! 1. 視聴ページ(live.nicovideo.jp/watch/lv…)の`embedded-data`から、番組の状態と
//!    視聴用のWebSocketのURLを取り出す
//! 2. そのWebSocketに`startWatching`を送ると、`messageServer`でメッセージサーバーの
//!    入口(viewUri)が届く(WebSocketはそこで閉じてよい)
//! 3. viewUri(`?at=now`)を読むと、コメントが入った区切り(segment。16秒ずつ)のURLと、
//!    次に読む時刻(next)が流れてくる。segmentのURLを読むとコメント(ChunkedMessage)が届く
//!
//! protobufは、使う所だけを手で読む(参考: n-air-app/nicolive-comment-protobuf、
//! tsukumijima/NDGRClient)。ニコニコ側の仕様変更で動かなくなる可能性がある。

use regex::Regex;
use std::sync::OnceLock;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// 入力(番組・ユーザー・チャンネルのURLか番号)から、視聴ページのURLを作る。
/// ユーザー・チャンネルの視聴ページは、その時に放送中の番組を開く
pub fn parse_target(input: &str) -> Result<String, String> {
    static LV: OnceLock<Regex> = OnceLock::new();
    static USER: OnceLock<Regex> = OnceLock::new();
    static CH: OnceLock<Regex> = OnceLock::new();
    let s = input.trim();
    let err = || "ニコ生の番組のURL(lv…)、ユーザーページのURL、またはユーザーIDを入力してください".to_string();
    if s.is_empty() {
        return Err(err());
    }
    let lv = LV.get_or_init(|| Regex::new(r"(?:^|[/=])(lv\d+)").unwrap());
    if let Some(c) = lv.captures(s) {
        return Ok(format!("https://live.nicovideo.jp/watch/{}", &c[1]));
    }
    let user = USER.get_or_init(|| Regex::new(r"user/(\d{1,12})(?:$|[/?#])|^(\d{1,12})$").unwrap());
    if let Some(c) = user.captures(s) {
        let id = c.get(1).or(c.get(2)).map_or("", |m| m.as_str());
        return Ok(format!("https://live.nicovideo.jp/watch/user/{id}"));
    }
    let ch = CH.get_or_init(|| Regex::new(r"(?:^|/)((?:ch|co)\d+)/?(?:$|[?#])").unwrap());
    if let Some(c) = ch.captures(s) {
        return Ok(format!("https://live.nicovideo.jp/watch/{}", &c[1]));
    }
    Err(err())
}

/// 視聴ページから分かる番組の情報
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// 番組ID(lv…)
    pub id: String,
    pub title: String,
    /// "ON_AIR" / "ENDED" / "BEFORE_RELEASE" など
    pub status: String,
    pub web_socket_url: String,
}

/// 視聴ページのHTMLから番組の情報を取り出す
pub fn parse_watch_page(html: &str) -> Option<Program> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"id="embedded-data"[^>]*data-props="([^"]*)""#).unwrap());
    let raw = re.captures(html)?.get(1)?.as_str();
    let json = html_unescape(raw);
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let p = &v["program"];
    Some(Program {
        id: p["nicoliveProgramId"].as_str()?.to_string(),
        title: p["title"].as_str().unwrap_or_default().to_string(),
        status: p["status"].as_str().unwrap_or_default().to_string(),
        web_socket_url: v["site"]["relive"]["webSocketUrl"].as_str().unwrap_or_default().to_string(),
    })
}

fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn fetch_program(client: &reqwest::Client, watch_url: &str) -> Result<Program, String> {
    let resp = client
        .get(watch_url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("ニコ生に繋がりません: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("番組・ユーザーが見つかりません(URLを確かめてください)".into());
    }
    let html = resp.text().await.map_err(|e| format!("ニコ生に繋がりません: {e}"))?;
    parse_watch_page(&html).ok_or_else(|| "放送中の番組がありません".into())
}

/// 視聴用のWebSocketに繋いで、メッセージサーバーの入口(viewUri)を聞く
pub async fn fetch_view_uri(web_socket_url: &str) -> Result<String, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let mut req = web_socket_url.into_client_request().map_err(|e| format!("ニコ生の視聴の準備に失敗しました: {e}"))?;
    req.headers_mut().insert("User-Agent", UA.parse().unwrap());
    let (ws, _) = tokio::time::timeout(std::time::Duration::from_secs(15), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| "ニコ生の視聴の準備に時間がかかりすぎました".to_string())?
        .map_err(|e| format!("ニコ生の視聴の準備に失敗しました: {e}"))?;
    let (mut write, mut read) = ws.split();
    let start = serde_json::json!({ "type": "startWatching", "data": { "reconnect": false } });
    write.send(Message::Text(start.to_string())).await.map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let msg = tokio::time::timeout_at(deadline, read.next())
            .await
            .map_err(|_| "ニコ生からコメントの入口が届きません".to_string())?;
        let Some(Ok(Message::Text(text))) = msg else {
            return Err("ニコ生の視聴の接続が切れました".into());
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        match v["type"].as_str() {
            Some("ping") => {
                let _ = write.send(Message::Text(r#"{"type":"pong"}"#.into())).await;
            }
            Some("messageServer") => {
                let uri = v["data"]["viewUri"].as_str().unwrap_or_default().to_string();
                let _ = write.close().await;
                return if uri.is_empty() { Err("ニコ生のコメントの入口が空でした".into()) } else { Ok(uri) };
            }
            Some("error") | Some("disconnect") => {
                let code = v["data"]["code"].as_str().or(v["data"]["reason"].as_str()).unwrap_or("UNKNOWN");
                return Err(format!("ニコ生に視聴を断られました({code})"));
            }
            _ => {}
        }
    }
}

/// 配信前チェック・接続確認用: 放送中か確かめる(コメントの取得は始めない)。(放送中か, 説明)
pub async fn check(input: &str) -> (bool, String) {
    let url = match parse_target(input) {
        Ok(u) => u,
        Err(e) => return (false, e),
    };
    match fetch_program(&client(), &url).await {
        Ok(p) if p.status == "ON_AIR" => (true, format!("放送中「{}」({})", p.title, p.id)),
        Ok(p) => (false, format!("「{}」は放送中ではありません", p.title)),
        Err(e) => (false, e),
    }
}

// ---- protobuf(使う所だけ) ----

#[derive(Debug, Clone, Copy, PartialEq)]
enum Val<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// メッセージの中のフィールドを順に返す(番号, 値)。壊れていたらそこで終わる
fn fields(buf: &[u8]) -> Vec<(u64, Val<'_>)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(key) = read_varint(buf, &mut pos) else { break };
        let (no, wire) = (key >> 3, key & 7);
        match wire {
            0 => match read_varint(buf, &mut pos) {
                Some(v) => out.push((no, Val::Varint(v))),
                None => break,
            },
            2 => {
                let Some(len) = read_varint(buf, &mut pos) else { break };
                let Some(end) = pos.checked_add(len as usize).filter(|&e| e <= buf.len()) else { break };
                out.push((no, Val::Bytes(&buf[pos..end])));
                pos = end;
            }
            1 => pos += 8,
            5 => pos += 4,
            _ => break,
        }
    }
    out
}

fn get_bytes<'a>(f: &[(u64, Val<'a>)], no: u64) -> Option<&'a [u8]> {
    f.iter().find_map(|(n, v)| match v {
        Val::Bytes(b) if *n == no => Some(*b),
        _ => None,
    })
}

fn get_varint(f: &[(u64, Val<'_>)], no: u64) -> Option<u64> {
    f.iter().find_map(|(n, v)| match v {
        Val::Varint(x) if *n == no => Some(*x),
        _ => None,
    })
}

fn get_str(f: &[(u64, Val<'_>)], no: u64) -> Option<String> {
    get_bytes(f, no).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// 長さ付きで続けて届くprotobufを1つずつ取り出す(届いた分を足していく)
#[derive(Default)]
pub struct StreamReader {
    buf: Vec<u8>,
}

impl StreamReader {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        let mut pos = 0;
        let len = read_varint(&self.buf, &mut pos)? as usize;
        if self.buf.len() < pos + len {
            return None;
        }
        let msg = self.buf[pos..pos + len].to_vec();
        self.buf.drain(..pos + len);
        Some(msg)
    }
}

/// メッセージサーバーの入口(viewUri)から流れてくるもの(ChunkedEntry)
#[derive(Debug, PartialEq)]
pub enum Entry {
    /// コメントが入った区切りのURL
    Segment(String),
    /// 次に入口を読む時刻(?at=に付ける)
    Next(i64),
    Other,
}

pub fn parse_entry(buf: &[u8]) -> Entry {
    let f = fields(buf);
    if let Some(seg) = get_bytes(&f, 1) {
        if let Some(uri) = get_str(&fields(seg), 3) {
            return Entry::Segment(uri);
        }
    }
    if let Some(next) = get_bytes(&f, 4) {
        return Entry::Next(get_varint(&fields(next), 1).unwrap_or(0) as i64);
    }
    Entry::Other
}

/// 区切り(segment)から届くもの(ChunkedMessage)
#[derive(Debug, PartialEq)]
pub enum Message {
    Chat(Chat),
    Gift { id: String, at: i64, name: String, item: String, point: u64, message: String },
    /// 視聴者数・コメント数
    Statistics { viewers: Option<u64>, comments: Option<u64> },
    /// 放送が終わった
    Ended,
    Other,
}

#[derive(Debug, PartialEq, Default)]
pub struct Chat {
    /// 同じコメントを2回出さないための番号
    pub id: String,
    /// 書き込まれた時刻(UNIX秒)
    pub at: i64,
    pub content: String,
    /// コテハン(付けている時だけ)
    pub name: Option<String>,
    /// ユーザーID(184でない時だけ)
    pub user_id: Option<u64>,
    /// 184の人を見分ける文字列
    pub hashed_user_id: Option<String>,
    pub no: u64,
}

pub fn parse_message(buf: &[u8]) -> Message {
    let f = fields(buf);
    let meta = get_bytes(&f, 1).map(fields).unwrap_or_default();
    let id = get_str(&meta, 1).unwrap_or_default();
    let at = get_bytes(&meta, 2).and_then(|t| get_varint(&fields(t), 1)).unwrap_or(0) as i64;
    if let Some(msg) = get_bytes(&f, 2) {
        let m = fields(msg);
        // 1: 普通のコメント 20: あふれたコメント(画面に流れない分。読み上げ・一覧には出す)
        if let Some(chat) = get_bytes(&m, 1).or_else(|| get_bytes(&m, 20)) {
            let c = fields(chat);
            return Message::Chat(Chat {
                id,
                at,
                content: get_str(&c, 1).unwrap_or_default(),
                name: get_str(&c, 2).filter(|s| !s.is_empty()),
                user_id: get_varint(&c, 5).filter(|&x| x > 0),
                hashed_user_id: get_str(&c, 6).filter(|s| !s.is_empty()),
                no: get_varint(&c, 8).unwrap_or(0),
            });
        }
        if let Some(gift) = get_bytes(&m, 8) {
            let g = fields(gift);
            return Message::Gift {
                id,
                at,
                name: get_str(&g, 3).unwrap_or_default(),
                item: get_str(&g, 6).unwrap_or_default(),
                point: get_varint(&g, 4).unwrap_or(0),
                message: get_str(&g, 5).unwrap_or_default(),
            };
        }
        return Message::Other;
    }
    if let Some(state) = get_bytes(&f, 4) {
        let s = fields(state);
        if let Some(stat) = get_bytes(&s, 1) {
            let st = fields(stat);
            return Message::Statistics { viewers: get_varint(&st, 1), comments: get_varint(&st, 2) };
        }
        if let Some(ps) = get_bytes(&s, 9) {
            if get_varint(&fields(ps), 1) == Some(1) {
                return Message::Ended;
            }
        }
    }
    Message::Other
}

/// protobufが長さ付きで続けて届くURLを読み、1つずつ`on_message`に渡す。読み終わったらOk
pub async fn read_stream(
    client: &reqwest::Client,
    url: &str,
    mut on_message: impl FnMut(Vec<u8>) -> bool,
) -> Result<(), String> {
    let mut resp = client
        .get(url)
        .header("Origin", "https://live.nicovideo.jp")
        .header("Referer", "https://live.nicovideo.jp/")
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("ニコ生のコメントを読めません: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ニコ生のコメントを読めません(HTTP {})", resp.status()));
    }
    let mut reader = StreamReader::default();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("ニコ生のコメントを読めません: {e}"))? {
        reader.push(&chunk);
        while let Some(m) = reader.next_message() {
            if !on_message(m) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// 入口(viewUri)を読み続け、区切り(segment)が届くたびに別の係で読んで、届いたものを`tx`に送る。
/// 次の区切りは前の区切りが終わる少し前に届くので、同時に2つ読むことがある。放送が終わる
/// (入口に「次」が無くなる)か、`alive`がfalseになったら、最後にNoneを送って終わる
pub async fn stream_messages(
    client: reqwest::Client,
    view: String,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<Option<Message>>,
) {
    use std::sync::atomic::Ordering;
    let mut at = "now".to_string();
    let mut started: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut fails = 0;
    while alive.load(Ordering::SeqCst) {
        let mut next = None;
        // 区切りのURLは入口から少しずつ流れてくる(入口は30秒ほど続く)ので、届いたらすぐ読み始める
        let r = read_stream(&client, &format!("{view}?at={at}"), |m| {
            match parse_entry(&m) {
                Entry::Segment(uri) => {
                    if started.len() > 50 {
                        started.clear();
                    }
                    if started.insert(uri.clone()) {
                        let (client, alive, tx) = (client.clone(), alive.clone(), tx.clone());
                        tokio::spawn(async move {
                            let _ = read_stream(&client, &uri, |m| {
                                let _ = tx.send(Some(parse_message(&m)));
                                alive.load(Ordering::SeqCst)
                            })
                            .await;
                        });
                    }
                }
                Entry::Next(t) => next = Some(t),
                Entry::Other => {}
            }
            alive.load(Ordering::SeqCst)
        })
        .await;
        match (r, next) {
            (_, Some(t)) => {
                fails = 0;
                at = t.to_string();
            }
            // 次が無い=放送が終わった
            (Ok(()), None) => break,
            (Err(_), None) => {
                fails += 1;
                if fails >= 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
    let _ = tx.send(None);
}

/// ユーザーの名前とアイコンのURL(公開されているユーザー情報から)
pub async fn fetch_user(client: &reqwest::Client, user_id: u64) -> Option<(String, Option<String>)> {
    let v: serde_json::Value = client
        .get(format!("https://nvapi.nicovideo.jp/v1/users/{user_id}"))
        .header("X-Frontend-Id", "6")
        .header("X-Frontend-Version", "0")
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let u = &v["data"]["user"];
    let name = u["nickname"].as_str()?.to_string();
    let icon = u["icons"]["small"].as_str().map(|s| s.to_string());
    Some((name, icon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        let w = |s: &str| parse_target(s).unwrap();
        assert_eq!(w("lv123456789"), "https://live.nicovideo.jp/watch/lv123456789");
        assert_eq!(w("https://live.nicovideo.jp/watch/lv345?ref=top"), "https://live.nicovideo.jp/watch/lv345");
        assert_eq!(w("https://www.nicovideo.jp/user/12345"), "https://live.nicovideo.jp/watch/user/12345");
        assert_eq!(w("https://www.nicovideo.jp/user/12345/live_programs"), "https://live.nicovideo.jp/watch/user/12345");
        assert_eq!(w("12345"), "https://live.nicovideo.jp/watch/user/12345");
        assert_eq!(w("https://ch.nicovideo.jp/ch2646436"), "https://live.nicovideo.jp/watch/ch2646436");
        assert_eq!(w("co1234"), "https://live.nicovideo.jp/watch/co1234");
        assert!(parse_target("").is_err());
        assert!(parse_target("https://example.com/").is_err());
    }

    #[test]
    fn parses_watch_page() {
        let html = r#"<script id="embedded-data" data-props="{&quot;program&quot;:{&quot;nicoliveProgramId&quot;:&quot;lv1&quot;,&quot;title&quot;:&quot;A &amp; B&quot;,&quot;status&quot;:&quot;ON_AIR&quot;},&quot;site&quot;:{&quot;relive&quot;:{&quot;webSocketUrl&quot;:&quot;wss://a.live2.nicovideo.jp/x&quot;}}}"></script>"#;
        let p = parse_watch_page(html).unwrap();
        assert_eq!(p.id, "lv1");
        assert_eq!(p.title, "A & B");
        assert_eq!(p.status, "ON_AIR");
        assert_eq!(p.web_socket_url, "wss://a.live2.nicovideo.jp/x");
    }

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn field_bytes(no: u64, b: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        varint(no << 3 | 2, &mut out);
        varint(b.len() as u64, &mut out);
        out.extend_from_slice(b);
        out
    }
    fn field_varint(no: u64, v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        varint(no << 3, &mut out);
        varint(v, &mut out);
        out
    }

    #[test]
    fn parses_chat_message() {
        let chat = [field_bytes(1, "こんにちは".as_bytes()), field_varint(5, 1234), field_varint(8, 42)].concat();
        let message = field_bytes(1, &chat);
        let meta = [field_bytes(1, b"abc"), field_bytes(2, &field_varint(1, 1_700_000_000))].concat();
        let buf = [field_bytes(1, &meta), field_bytes(2, &message)].concat();
        assert_eq!(
            parse_message(&buf),
            Message::Chat(Chat { id: "abc".into(), at: 1_700_000_000, content: "こんにちは".into(), user_id: Some(1234), no: 42, ..Default::default() })
        );
    }

    #[test]
    fn parses_statistics_and_end() {
        let stat = [field_varint(1, 321), field_varint(2, 45)].concat();
        let buf = field_bytes(4, &field_bytes(1, &stat));
        assert_eq!(parse_message(&buf), Message::Statistics { viewers: Some(321), comments: Some(45) });
        let buf = field_bytes(4, &field_bytes(9, &field_varint(1, 1)));
        assert_eq!(parse_message(&buf), Message::Ended);
    }

    #[test]
    fn parses_entries_and_stream() {
        let seg = field_bytes(1, &field_bytes(3, b"https://mpn/seg"));
        assert_eq!(parse_entry(&seg), Entry::Segment("https://mpn/seg".into()));
        let next = field_bytes(4, &field_varint(1, 1_700_000_016));
        assert_eq!(parse_entry(&next), Entry::Next(1_700_000_016));
        // 長さ付きで続けて届き、途中で切れていても続きを待つ
        let mut all = Vec::new();
        for m in [&seg, &next] {
            varint(m.len() as u64, &mut all);
            all.extend_from_slice(m);
        }
        let mut r = StreamReader::default();
        r.push(&all[..5]);
        assert_eq!(r.next_message(), None);
        r.push(&all[5..]);
        assert_eq!(r.next_message().as_deref(), Some(&seg[..]));
        assert_eq!(r.next_message().as_deref(), Some(&next[..]));
        assert_eq!(r.next_message(), None);
    }

    /// 本物のニコ生(ニコニコ実況のNHK総合。いつも放送中)に繋ぎ、40秒(区切りを2つ以上またぐ)
    /// 読み続ける(手で動かす時だけ: cargo test reads_real_niconico -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn reads_real_niconico() {
        tauri::async_runtime::block_on(async {
            let c = client();
            let p = fetch_program(&c, &parse_target("ch2646436").unwrap()).await.unwrap();
            println!("{p:?}");
            let view = fetch_view_uri(&p.web_socket_url).await.unwrap();
            let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(stream_messages(c.clone(), view, alive.clone(), tx));
            let end = tokio::time::Instant::now() + std::time::Duration::from_secs(40);
            let (mut chats, mut ids, mut stats, mut gaps) = (0, std::collections::HashSet::new(), 0, 0);
            let mut last = tokio::time::Instant::now();
            while let Ok(Some(Some(m))) = tokio::time::timeout_at(end, rx.recv()).await {
                match m {
                    Message::Chat(ch) => {
                        chats += 1;
                        ids.insert(ch.id);
                        if last.elapsed() > std::time::Duration::from_secs(8) {
                            gaps += 1;
                        }
                        last = tokio::time::Instant::now();
                        if chats % 20 == 1 {
                            println!("{} {}", ch.no, ch.content);
                        }
                    }
                    Message::Statistics { comments, .. } => {
                        stats += 1;
                        println!("statistics comments {comments:?}");
                    }
                    _ => {}
                }
            }
            alive.store(false, std::sync::atomic::Ordering::SeqCst);
            println!("chats {chats} unique {} stats {stats} gaps {gaps}", ids.len());
            assert!(chats > 0 && stats > 0);
        });
    }
}
