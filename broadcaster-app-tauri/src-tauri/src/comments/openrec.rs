//! OPENREC.tvのコメント取得(ログイン不要)。
//!
//! 公開されている情報(public.openrec.tv/external/api/v5)を使う:
//! 1. チャンネルが指定されたら、`movies?channel_ids=…&onair_status=1`で配信中の番組を調べる
//! 2. 番組のチャット(chat.openrec.tv。socket.io)に繋ぐと、コメントが来た・視聴者数が変わった、
//!    という知らせが届く。知らせが来た時(と、念のため数秒ごと)に、`movies/{id}/chats`で
//!    新しいコメントを読む(コメントの中身は、この公開の情報の形だけを使う)
//!
//! OPENREC側の仕様変更で動かなくなる可能性がある。

use regex::Regex;
use std::sync::OnceLock;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const API: &str = "https://public.openrec.tv/external/api/v5";

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// チャンネル(ユーザー)のID。配信が始まると自動で読む
    Channel(String),
    /// 番組のID(URLの live/ の後ろ)
    Movie(String),
}

/// 入力(チャンネル・配信のURLか、チャンネルID)から、何を読むかを決める
pub fn parse_target(input: &str) -> Result<Target, String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    static ID: OnceLock<Regex> = OnceLock::new();
    let s = input.trim();
    let err = || "OPENRECのチャンネルのURL(https://www.openrec.tv/user/…)、配信のURL、またはチャンネルIDを入力してください".to_string();
    let re = RE.get_or_init(|| Regex::new(r"openrec\.tv/(user|live|movie)/([A-Za-z0-9_-]{1,64})").unwrap());
    if let Some(c) = re.captures(s) {
        let id = c[2].to_string();
        return Ok(if &c[1] == "user" { Target::Channel(id) } else { Target::Movie(id) });
    }
    if s.contains('/') || s.contains('.') {
        return Err(err());
    }
    let id = ID.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").unwrap());
    if id.is_match(s) {
        Ok(Target::Channel(s.to_string()))
    } else {
        Err(err())
    }
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 配信中の番組
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// 番組のID(文字。コメントを読む時に使う)
    pub id: String,
    /// 番組の番号(チャットに繋ぐ時に使う)
    pub movie_id: u64,
    pub title: String,
    pub viewers: Option<u64>,
    /// 1=配信中
    pub onair_status: i64,
}

fn parse_live(v: &serde_json::Value) -> Option<Live> {
    Some(Live {
        id: v["id"].as_str()?.to_string(),
        movie_id: v["movie_id"].as_u64()?,
        title: v["title"].as_str().unwrap_or_default().to_string(),
        viewers: v["live_views"].as_u64(),
        onair_status: v["onair_status"].as_i64().unwrap_or(0),
    })
}

async fn get_json(client: &reqwest::Client, url: &str, query: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let resp = client
        .get(url)
        .query(query)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("OPENRECに繋がりません: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("OPENRECの返事を読めません: {e}"))?;
    if !status.is_success() {
        let msg = v["message"].as_str().unwrap_or_default();
        return Err(format!("OPENRECに断られました(HTTP {status}){msg}"));
    }
    Ok(v)
}

/// 配信中の番組を調べる(配信していなければNone)
pub async fn find_live(client: &reqwest::Client, target: &Target) -> Result<Option<Live>, String> {
    match target {
        Target::Channel(ch) => {
            let v = get_json(client, &format!("{API}/movies"), &[("channel_ids", ch), ("onair_status", "1")]).await?;
            Ok(v.as_array().and_then(|a| a.iter().find_map(parse_live)).filter(|l| l.onair_status == 1))
        }
        Target::Movie(id) => match get_json(client, &format!("{API}/movies/{id}"), &[]).await {
            Ok(v) => Ok(parse_live(&v).filter(|l| l.onair_status == 1)),
            Err(e) if e.contains("存在しません") => Err("その配信が見つかりません(URLを確かめてください)".into()),
            Err(e) => Err(e),
        },
    }
}

/// 番組の今の状態(配信が終わったかを確かめる)
pub async fn fetch_live(client: &reqwest::Client, id: &str) -> Result<Option<Live>, String> {
    get_json(client, &format!("{API}/movies/{id}"), &[]).await.map(|v| parse_live(&v))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chat {
    pub id: u64,
    pub name: String,
    pub message: String,
    pub icon: Option<String>,
    /// スタンプだけのコメント
    pub stamp: bool,
    /// エール(投げ銭)のポイント
    pub yell: Option<u64>,
    /// 書き込まれた時刻(次に読む時の起点。OPENRECの形のまま)
    pub posted_at: String,
}

pub fn parse_chats(v: &serde_json::Value) -> Vec<Chat> {
    let Some(list) = v.as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|c| {
            let user = &c["user"];
            let name = user["nickname"].as_str().filter(|s| !s.is_empty()).or(user["id"].as_str()).unwrap_or("名無し");
            Some(Chat {
                id: c["id"].as_u64()?,
                name: name.to_string(),
                message: c["message"].as_str().unwrap_or_default().to_string(),
                icon: user["icon_image_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
                stamp: c["stamp"].is_object(),
                yell: c["yell"]["yells"].as_u64().or(c["yell"]["points"].as_u64()),
                posted_at: c["posted_at"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// `from`(OPENRECの時刻の形)より後に書かれたコメントを、古い順に最大100件
pub async fn fetch_chats(client: &reqwest::Client, id: &str, from: &str) -> Result<Vec<Chat>, String> {
    let v = get_json(
        client,
        &format!("{API}/movies/{id}/chats"),
        &[("from_created_at", from), ("is_including_system_message", "false")],
    )
    .await?;
    Ok(parse_chats(&v))
}

/// チャットから届く知らせ
#[derive(Debug, PartialEq)]
pub enum Notice {
    /// 新しいコメントが来た(中身は公開の情報から読む)
    Chat,
    Viewers(u64),
    Other,
}

/// socket.ioのメッセージ(`42["message","{…}"]`)を読む
pub fn parse_notice(frame: &str) -> Notice {
    let Some(rest) = frame.strip_prefix("42") else { return Notice::Other };
    let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(rest) else { return Notice::Other };
    let Some(inner) = a.get(1).and_then(|x| x.as_str()) else { return Notice::Other };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(inner) else { return Notice::Other };
    match v["type"].as_i64() {
        Some(0) => Notice::Chat,
        Some(1) => v["data"]["live_viewers"].as_u64().map_or(Notice::Other, Notice::Viewers),
        _ => Notice::Other,
    }
}

pub fn chat_url(movie_id: u64) -> String {
    format!("wss://chat.openrec.tv/socket.io/?movieId={movie_id}&EIO=3&transport=websocket")
}

/// 配信前チェック・接続確認用: 配信中か確かめる(コメントの取得は始めない)。(配信中か, 説明)
pub async fn check(input: &str) -> (bool, String) {
    let target = match parse_target(input) {
        Ok(t) => t,
        Err(e) => return (false, e),
    };
    match find_live(&client(), &target).await {
        Ok(Some(l)) => (true, format!("配信中「{}」", l.title)),
        Ok(None) => (false, "今は配信していません(始まると自動で読みます)".into()),
        Err(e) => (false, e),
    }
}

/// 今の時刻(OPENRECのfrom_created_atに渡す形)
pub fn now_iso() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // 1970-01-01からの日数を年月日にする(Howard Hinnantのやり方)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(parse_target("https://www.openrec.tv/user/Suzuka_minase").unwrap(), Target::Channel("Suzuka_minase".into()));
        assert_eq!(parse_target("https://www.openrec.tv/live/fuks4hpidbz").unwrap(), Target::Movie("fuks4hpidbz".into()));
        assert_eq!(parse_target("Suzuka_minase").unwrap(), Target::Channel("Suzuka_minase".into()));
        assert!(parse_target("").is_err());
        assert!(parse_target("https://example.com/user/x").is_err());
    }

    #[test]
    fn parses_chats() {
        let v = serde_json::json!([
            { "id": 1, "message": "こんにちは", "posted_at": "2026-11-04T21:01:07+09:00", "stamp": null, "yell": null,
              "user": { "id": "negi", "nickname": "ねぎ", "icon_image_url": "" } },
            { "id": 2, "message": "", "posted_at": "2026-11-04T21:01:08+09:00", "stamp": { "id": 4941 }, "yell": null,
              "user": { "id": "abc", "nickname": "", "icon_image_url": "https://x/icon.png" } },
            { "id": 3, "message": "応援", "posted_at": "2026-11-04T21:02:05+09:00", "stamp": null,
              "yell": { "name": "sun", "points": 250, "yells": 200 }, "user": { "id": "akiha", "nickname": "あきは" } }
        ]);
        let c = parse_chats(&v);
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].name.as_str(), c[0].message.as_str(), c[0].icon.clone()), ("ねぎ", "こんにちは", None));
        assert_eq!((c[1].name.as_str(), c[1].stamp, c[1].icon.as_deref()), ("abc", true, Some("https://x/icon.png")));
        assert_eq!(c[2].yell, Some(200));
        assert_eq!(c[2].posted_at, "2026-11-04T21:02:05+09:00");
    }

    #[test]
    fn parses_notices() {
        assert_eq!(
            parse_notice(r#"42["message","{\"type\":1,\"room\":\"1\",\"data\":{\"movie_id\":1,\"viewers\":205,\"live_viewers\":92}}"]"#),
            Notice::Viewers(92)
        );
        assert_eq!(parse_notice(r#"42["message","{\"type\":0,\"room\":\"1\",\"data\":{}}"]"#), Notice::Chat);
        assert_eq!(parse_notice("40"), Notice::Other);
        assert_eq!(parse_notice("3"), Notice::Other);
    }

    #[test]
    fn makes_iso_time() {
        let s = now_iso();
        assert_eq!(s.len(), 24, "{s}");
        assert!(s.starts_with("20") && s.ends_with(".000Z"), "{s}");
    }

    /// 本物のOPENRECに繋ぐ(手で動かす時だけ: cargo test reads_real_openrec -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn reads_real_openrec() {
        tauri::async_runtime::block_on(async {
            let c = client();
            let v = get_json(&c, &format!("{API}/movies"), &[("onair_status", "1"), ("limit", "40")]).await.unwrap();
            let live = v.as_array().unwrap().iter().filter_map(parse_live).max_by_key(|l| l.viewers.unwrap_or(0)).unwrap();
            println!("{live:?}");
            let chats = fetch_chats(&c, &live.id, "2020-01-01T00:00:00.000Z").await.unwrap();
            println!("chats {}", chats.len());
            for ch in chats.iter().take(3) {
                println!("{ch:?}");
            }
        });
    }
}
