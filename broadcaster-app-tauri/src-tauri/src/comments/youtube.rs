//! YouTube Liveのチャット取得(非公式)。
//!
//! 公式のYouTube Data APIは1日あたりの利用量の上限が厳しく、数時間の配信で
//! 足りなくなるため、unacast等と同じくブラウザのチャット欄と同じ仕組みを使う:
//! 1. チャンネルが指定されたら`{チャンネル}/live`を開いて、配信中の動画IDを調べる
//! 2. `live_chat?v={動画ID}`のページから、クライアントのバージョンと
//!    続きを取得するためのトークン(continuation)を取り出す
//! 3. `youtubei/v1/live_chat/get_live_chat`にcontinuationを送ると、その後の
//!    新着コメントと次のcontinuationが返ってくるので、これを繰り返す
//!
//! YouTube側の仕様変更で動かなくなる可能性がある(その場合は状態表示にエラーを
//! 出し、しばらく待ってから最初からやり直す)。

use regex::Regex;
use std::sync::OnceLock;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

#[derive(Debug, PartialEq)]
pub enum Target {
    Video(String),
    /// チャンネルのURL(`https://www.youtube.com/@xxx`、`/channel/UC...`)
    Channel(String),
}

/// 入力(動画URL・動画ID・チャンネルURL・@ハンドル・チャンネルID)を解釈する。
pub fn parse_target(input: &str) -> Result<Target, String> {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return Err("YouTubeの配信URLかチャンネルを入力してください".into());
    }
    static VIDEO_ID: OnceLock<Regex> = OnceLock::new();
    let vid = VIDEO_ID.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap());
    if vid.is_match(s) {
        return Ok(Target::Video(s.to_string()));
    }
    if s.starts_with('@') {
        return Ok(Target::Channel(format!("https://www.youtube.com/{s}")));
    }
    if s.starts_with("UC") && s.len() == 24 {
        return Ok(Target::Channel(format!("https://www.youtube.com/channel/{s}")));
    }
    let url = reqwest::Url::parse(s).map_err(|_| "YouTubeのURLの形式が正しくありません".to_string())?;
    let host = url.host_str().unwrap_or_default();
    if host == "youtu.be" {
        let id = url.path().trim_matches('/');
        if vid.is_match(id) {
            return Ok(Target::Video(id.to_string()));
        }
    }
    if !host.ends_with("youtube.com") {
        return Err("YouTubeのURLではありません".into());
    }
    if let Some((_, v)) = url.query_pairs().find(|(k, _)| k == "v") {
        if vid.is_match(&v) {
            return Ok(Target::Video(v.to_string()));
        }
    }
    let segs: Vec<&str> = url.path_segments().map(|x| x.filter(|p| !p.is_empty()).collect()).unwrap_or_default();
    match segs.as_slice() {
        ["live" | "shorts" | "embed", id, ..] if vid.is_match(id) => Ok(Target::Video(id.to_string())),
        [first, ..] if first.starts_with('@') => Ok(Target::Channel(format!("https://www.youtube.com/{first}"))),
        ["channel" | "c" | "user", name, ..] => {
            Ok(Target::Channel(format!("https://www.youtube.com/{}/{name}", segs[0])))
        }
        _ => Err("YouTubeの配信URLかチャンネルのURLを入力してください".into()),
    }
}

/// チャンネルの`/live`ページから、配信中の動画IDを取り出す(配信していなければNone)。
pub fn extract_live_video_id(html: &str) -> Option<String> {
    static CANON: OnceLock<Regex> = OnceLock::new();
    let re = CANON.get_or_init(|| {
        Regex::new(r#"<link rel="canonical" href="https://www\.youtube\.com/watch\?v=([A-Za-z0-9_-]{11})""#).unwrap()
    });
    let id = re.captures(html)?.get(1)?.as_str().to_string();
    // 配信予定(待機所)のページもcanonicalがwatch?v=になるので、配信中かどうかも見る
    if html.contains("\"isLiveNow\":true") || html.contains("\"isLive\":true") {
        Some(id)
    } else {
        None
    }
}

pub struct ChatParams {
    pub client_version: String,
    pub continuation: String,
}

/// live_chatのページから、クライアントのバージョンと最初のcontinuationを取り出す。
pub fn extract_chat_params(html: &str) -> Option<ChatParams> {
    static VER: OnceLock<Regex> = OnceLock::new();
    let ver = VER.get_or_init(|| Regex::new(r#""INNERTUBE_CONTEXT_CLIENT_VERSION":"([^"]+)""#).unwrap());
    let client_version = ver.captures(html)?.get(1)?.as_str().to_string();
    let continuation = ["invalidationContinuationData", "timedContinuationData", "reloadContinuationData"]
        .iter()
        .find_map(|k| {
            let i = html.find(k)?;
            let key = "\"continuation\":\"";
            let j = html[i..].find(key)? + i + key.len();
            let end = html[j..].find('"')? + j;
            Some(html[j..end].to_string())
        })?;
    Some(ChatParams { client_version, continuation })
}

#[derive(Debug, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub author: String,
    pub text: String,
    /// スーパーチャットの金額(例: "¥500")
    pub amount: Option<String>,
}

pub struct ChatPage {
    pub messages: Vec<ChatMessage>,
    /// 次のcontinuationと、次に取りに行くまでの推奨待ち時間(ms)
    pub next: Option<(String, u64)>,
}

fn runs_text(runs: Option<&serde_json::Value>) -> String {
    let Some(runs) = runs.and_then(|r| r.as_array()) else {
        return String::new();
    };
    runs.iter()
        .map(|r| {
            if let Some(t) = r.get("text").and_then(|t| t.as_str()) {
                return t.to_string();
            }
            let Some(e) = r.get("emoji") else {
                return String::new();
            };
            // 標準の絵文字はemojiIdがそのまま絵文字の文字。カスタム絵文字は:名前:にする
            if e.get("isCustomEmoji").and_then(|v| v.as_bool()).unwrap_or(false) {
                e.pointer("/shortcuts/0").and_then(|v| v.as_str()).unwrap_or("").to_string()
            } else {
                e.get("emojiId").and_then(|v| v.as_str()).unwrap_or("").to_string()
            }
        })
        .collect()
}

/// get_live_chatの応答(JSON)から新着コメントと次のcontinuationを取り出す。
pub fn parse_chat_response(v: &serde_json::Value) -> ChatPage {
    let lc = v.pointer("/continuationContents/liveChatContinuation");
    let mut messages = Vec::new();
    if let Some(actions) = lc.and_then(|l| l.get("actions")).and_then(|a| a.as_array()) {
        for a in actions {
            let Some(item) = a.pointer("/addChatItemAction/item") else {
                continue;
            };
            let (r, amount) = if let Some(r) = item.get("liveChatTextMessageRenderer") {
                (r, None)
            } else if let Some(r) = item.get("liveChatPaidMessageRenderer") {
                (r, r.pointer("/purchaseAmountText/simpleText").and_then(|s| s.as_str()).map(String::from))
            } else {
                continue;
            };
            let text = runs_text(r.pointer("/message/runs"));
            if text.trim().is_empty() && amount.is_none() {
                continue;
            }
            messages.push(ChatMessage {
                id: r.get("id").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
                author: r.pointer("/authorName/simpleText").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
                text,
                amount,
            });
        }
    }
    let next = lc
        .and_then(|l| l.pointer("/continuations/0"))
        .and_then(|c| c.as_object())
        .and_then(|o| o.values().next())
        .and_then(|d| {
            let c = d.get("continuation")?.as_str()?.to_string();
            let t = d.get("timeoutMs").and_then(|t| t.as_u64()).unwrap_or(5000);
            Some((c, t))
        });
    ChatPage { messages, next }
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(UA)
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(reqwest::header::ACCEPT_LANGUAGE, "ja,en;q=0.8".parse().unwrap());
            h
        })
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("YouTubeへの接続に失敗しました: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("YouTubeへの接続に失敗しました(HTTP {})", resp.status().as_u16()));
    }
    resp.text().await.map_err(|e| format!("YouTubeへの接続に失敗しました: {e}"))
}

pub async fn fetch_chat(client: &reqwest::Client, params: &ChatParams, continuation: &str) -> Result<ChatPage, String> {
    let body = serde_json::json!({
        "context": { "client": { "clientName": "WEB", "clientVersion": params.client_version, "hl": "ja", "gl": "JP" } },
        "continuation": continuation,
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?prettyPrint=false")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("YouTubeのチャット取得に失敗しました: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("YouTubeのチャット取得に失敗しました(HTTP {})", resp.status().as_u16()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("YouTubeのチャット取得に失敗しました: {e}"))?;
    Ok(parse_chat_response(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(parse_target("dQw4w9WgXcQ").unwrap(), Target::Video("dQw4w9WgXcQ".into()));
        assert_eq!(parse_target("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1").unwrap(), Target::Video("dQw4w9WgXcQ".into()));
        assert_eq!(parse_target("https://youtu.be/dQw4w9WgXcQ").unwrap(), Target::Video("dQw4w9WgXcQ".into()));
        assert_eq!(parse_target("https://www.youtube.com/live/dQw4w9WgXcQ?si=x").unwrap(), Target::Video("dQw4w9WgXcQ".into()));
        assert_eq!(parse_target("@LofiGirl").unwrap(), Target::Channel("https://www.youtube.com/@LofiGirl".into()));
        assert_eq!(parse_target("https://www.youtube.com/@LofiGirl/live").unwrap(), Target::Channel("https://www.youtube.com/@LofiGirl".into()));
        assert_eq!(
            parse_target("UCoNlwM7wU-l3r-t5lXefFoQ").unwrap(),
            Target::Channel("https://www.youtube.com/channel/UCoNlwM7wU-l3r-t5lXefFoQ".into())
        );
        assert_eq!(
            parse_target("https://www.youtube.com/channel/UCoNlwM7wU-l3r-t5lXefFoQ").unwrap(),
            Target::Channel("https://www.youtube.com/channel/UCoNlwM7wU-l3r-t5lXefFoQ".into())
        );
        assert!(parse_target("https://example.com/x").is_err());
        assert!(parse_target("").is_err());
    }

    #[test]
    fn extracts_live_video_and_params() {
        let live = r#"<link rel="canonical" href="https://www.youtube.com/watch?v=1-LpQekNa9g"> ..."isLiveNow":true"#;
        assert_eq!(extract_live_video_id(live).as_deref(), Some("1-LpQekNa9g"));
        let offline = r#"<link rel="canonical" href="https://www.youtube.com/channel/UCxxx">"#;
        assert_eq!(extract_live_video_id(offline), None);
        let html = r#"..."INNERTUBE_CONTEXT_CLIENT_VERSION":"2.20261007.01.00"...
            "invalidationContinuationData":{"invalidationId":{"objectSource":1},"timeoutMs":10000,"continuation":"0ofABC"}"#;
        let p = extract_chat_params(html).unwrap();
        assert_eq!(p.client_version, "2.20261007.01.00");
        assert_eq!(p.continuation, "0ofABC");
    }

    #[test]
    fn parses_chat_response() {
        let v = serde_json::json!({ "continuationContents": { "liveChatContinuation": {
            "continuations": [{ "invalidationContinuationData": { "continuation": "NEXT", "timeoutMs": 10000 } }],
            "actions": [
                { "addChatItemAction": { "item": { "liveChatTextMessageRenderer": {
                    "id": "a1", "authorName": { "simpleText": "@たぬき" },
                    "message": { "runs": [ { "text": "こんにちは" }, { "emoji": { "emojiId": "😀" } },
                        { "emoji": { "isCustomEmoji": true, "shortcuts": [":yt:"] } } ] } } } } },
                { "addChatItemAction": { "item": { "liveChatPaidMessageRenderer": {
                    "id": "a2", "authorName": { "simpleText": "@ねこ" },
                    "purchaseAmountText": { "simpleText": "¥500" },
                    "message": { "runs": [ { "text": "応援" } ] } } } } },
                { "addChatItemAction": { "item": { "liveChatMembershipItemRenderer": {} } } },
                { "markChatItemAsDeletedAction": {} }
            ]
        } } });
        let page = parse_chat_response(&v);
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].author, "@たぬき");
        assert_eq!(page.messages[0].text, "こんにちは😀:yt:");
        assert_eq!(page.messages[1].amount.as_deref(), Some("¥500"));
        assert_eq!(page.next, Some(("NEXT".into(), 10000)));
        let ended = parse_chat_response(&serde_json::json!({}));
        assert!(ended.messages.is_empty() && ended.next.is_none());
    }

    /// 実際のYouTubeのライブからチャットを取る(ネットワークに繋ぐので普段は実行しない)。
    #[test]
    #[ignore]
    fn fetches_real_youtube_live_chat() {
        tauri::async_runtime::block_on(async {
            let c = client();
            let html = get_text(&c, "https://www.youtube.com/@LofiGirl/live").await.unwrap();
            let id = extract_live_video_id(&html).expect("配信中のはず");
            let chat = get_text(&c, &format!("https://www.youtube.com/live_chat?is_popout=1&v={id}")).await.unwrap();
            let p = extract_chat_params(&chat).expect("チャットのパラメータ");
            let page = fetch_chat(&c, &p, &p.continuation).await.unwrap();
            assert!(page.next.is_some(), "次のcontinuationが返るはず");
            eprintln!("video={id} messages={}", page.messages.len());
        });
    }
}
