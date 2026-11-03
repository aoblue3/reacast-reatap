//! 視聴者数・コメント数(コメント設定の「視聴者数・コメント数」タブ、OBSの audience.html)。
//!
//! 視聴者数は、取得中だけ30秒おきに読む。どれもログインしないで誰でも見られる数字:
//! - YouTube: YouTubeのページ自身が使っている updated_metadata(「〇人が視聴中」)
//! - Twitch: アイコンの取得と同じ、Twitchのサイトの公開のGraphQL(stream.viewersCount)
//! - PeerCast: 配信者のPCで動いているPeerCastから読む(外のサイトには取りに行かない)。
//!   PeerCastStationはJSON-RPC(/api/1 の getChannels)、PeerCast YTなどは /admin?cmd=viewxml
//!
//! コメント数は、取得を始めてから届いた数(PeerCastは掲示板のレス)。

use serde::Serialize;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCount {
    /// "youtube" | "twitch" | "peercast"
    pub id: &'static str,
    /// 視聴者数(まだ分からない・配信していない時はNone)
    pub viewers: Option<u64>,
    pub comments: u64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Audience {
    pub running: bool,
    /// 使っている取得元だけ(YouTube・Twitch・PeerCastの順)
    pub sources: Vec<SourceCount>,
}

impl Audience {
    pub fn get_mut(&mut self, id: &str) -> Option<&mut SourceCount> {
        self.sources.iter_mut().find(|s| s.id == id)
    }
}

/// "1,163 人が視聴中" のような文から数字だけ取り出す
fn digits(s: &str) -> Option<u64> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

/// updated_metadataの応答から、今見ている人数を取り出す(ライブ中でなければNone)
pub fn parse_youtube_viewers(v: &serde_json::Value) -> Option<u64> {
    let actions = v.get("actions")?.as_array()?;
    let r = actions.iter().find_map(|a| a.pointer("/updateViewershipAction/viewCount/videoViewCountRenderer"))?;
    if r.get("isLive").and_then(|x| x.as_bool()) == Some(false) {
        return None;
    }
    r.get("originalViewCount")
        .and_then(|x| x.as_str())
        .and_then(digits)
        .or_else(|| r.pointer("/viewCount/simpleText").and_then(|x| x.as_str()).and_then(digits))
}

pub async fn fetch_youtube_viewers(client: &reqwest::Client, video_id: &str, client_version: &str) -> Option<u64> {
    let body = serde_json::json!({
        "context": { "client": { "clientName": "WEB", "clientVersion": client_version, "hl": "ja", "gl": "JP" } },
        "videoId": video_id,
    });
    let resp = client
        .post("https://www.youtube.com/youtubei/v1/updated_metadata?prettyPrint=false")
        .json(&body)
        .send()
        .await
        .ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    parse_youtube_viewers(&v)
}

pub async fn fetch_twitch_viewers(client: &reqwest::Client, login: &str) -> Option<u64> {
    if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let query = serde_json::json!({ "query": format!("query{{user(login:\"{login}\"){{stream{{viewersCount}}}}}}") });
    let resp = client
        .post("https://gql.twitch.tv/gql")
        .header("Client-Id", "kimne78kx3ncx6brgo4mv6wki5h1ko")
        .timeout(std::time::Duration::from_secs(6))
        .json(&query)
        .send()
        .await
        .ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    v.pointer("/data/user/stream/viewersCount").and_then(|x| x.as_u64())
}

/// PeerCastStationのgetChannelsの結果から、自分が配信しているチャンネルのリスナー数(合計)
pub fn parse_peercast_station(v: &serde_json::Value) -> Option<u64> {
    let channels = v.get("result")?.as_array()?;
    let mine: Vec<&serde_json::Value> =
        channels.iter().filter(|c| c.pointer("/status/isBroadcasting").and_then(|x| x.as_bool()) == Some(true)).collect();
    if mine.is_empty() {
        return None;
    }
    Some(mine.iter().map(|c| c.pointer("/status/totalDirects").and_then(|x| x.as_u64()).unwrap_or(0)).sum())
}

/// PeerCast YTなどの /admin?cmd=viewxml から、中継しているチャンネルのリスナー数(合計)。
/// チャンネルが1つも無ければNone
pub fn parse_peercast_viewxml(xml: &str) -> Option<u64> {
    let relayed = xml.split("<channels_relayed").nth(1)?.split("</channels_relayed>").next()?;
    let mut total = None;
    for ch in relayed.split("<channel ").skip(1) {
        // 全体の人数(hits)があればそれ、無ければこのPCの人数(relay)
        let pick = |tag: &str| -> Option<u64> {
            let t = ch.split(&format!("<{tag} ")).nth(1)?;
            let a = t.split("listeners=\"").nth(1)?;
            a.split('"').next()?.parse().ok()
        };
        if let Some(n) = pick("hits").or_else(|| pick("relay")) {
            total = Some(total.unwrap_or(0) + n);
        }
    }
    total
}

pub async fn fetch_peercast_listeners(client: &reqwest::Client, port: u16) -> Option<u64> {
    let base = format!("http://127.0.0.1:{port}");
    let rpc = client
        .post(format!("{base}/api/1"))
        .header("X-Requested-With", "XMLHttpRequest")
        .timeout(std::time::Duration::from_secs(3))
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "getChannels" }))
        .send()
        .await;
    if let Ok(resp) = rpc {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                if v.get("result").is_some() {
                    return parse_peercast_station(&v);
                }
            }
        }
    }
    let resp = client.get(format!("{base}/admin?cmd=viewxml")).timeout(std::time::Duration::from_secs(3)).send().await.ok()?;
    parse_peercast_viewxml(&resp.text().await.ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_viewers() {
        let v = serde_json::json!({ "actions": [{ "updateViewershipAction": { "viewCount": { "videoViewCountRenderer": {
            "viewCount": { "simpleText": "1,162 人が視聴中" }, "isLive": true, "originalViewCount": "1162" } } } }] });
        assert_eq!(parse_youtube_viewers(&v), Some(1162));
        let v = serde_json::json!({ "actions": [{ "updateViewershipAction": { "viewCount": { "videoViewCountRenderer": {
            "viewCount": { "simpleText": "1,162 人が視聴中" }, "isLive": true } } } }] });
        assert_eq!(parse_youtube_viewers(&v), Some(1162));
        assert_eq!(parse_youtube_viewers(&serde_json::json!({ "actions": [] })), None);
    }

    /// 本物のYouTube・Twitch・このPCのPeerCastに繋ぐ(手で動かす時だけ: cargo test fetches_real_audience -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn fetches_real_audience() {
        tauri::async_runtime::block_on(async {
            let yt = crate::comments::youtube::client();
            let html = crate::comments::youtube::get_text(&yt, "https://www.youtube.com/@LofiGirl/live").await.unwrap();
            let id = crate::comments::youtube::extract_live_video_id(&html).unwrap();
            let chat = crate::comments::youtube::get_text(&yt, &format!("https://www.youtube.com/live_chat?is_popout=1&v={id}")).await.unwrap();
            let params = crate::comments::youtube::extract_chat_params(&chat).unwrap();
            println!("YouTube {id}: {:?}", fetch_youtube_viewers(&yt, &id, &params.client_version).await);
            let c = reqwest::Client::new();
            println!("Twitch: {:?}", fetch_twitch_viewers(&c, "shroud").await);
            println!("PeerCast: {:?}", fetch_peercast_listeners(&c, 7144).await);
        });
    }

    #[test]
    fn parses_peercast() {
        let v = serde_json::json!({ "result": [
            { "status": { "isBroadcasting": true, "totalDirects": 12, "totalRelays": 3 } },
            { "status": { "isBroadcasting": false, "totalDirects": 99 } } ] });
        assert_eq!(parse_peercast_station(&v), Some(12));
        assert_eq!(parse_peercast_station(&serde_json::json!({ "result": [] })), None);
        let xml = r#"<peercast><channels_relayed total="1"><channel name="a" id="x"><relay listeners="2" relays="1" hosts="3" status="BROADCAST" /><hits hosts="5" listeners="7" relays="2" /></channel></channels_relayed><channels_found total="0" /></peercast>"#;
        assert_eq!(parse_peercast_viewxml(xml), Some(7));
        assert_eq!(parse_peercast_viewxml(r#"<peercast><channels_relayed total="0" /></peercast>"#), None);
    }
}
