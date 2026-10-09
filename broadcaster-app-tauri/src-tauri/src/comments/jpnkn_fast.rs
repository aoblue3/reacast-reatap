//! jpnknの「Fastインターフェース」(書き込みの即時通知)。
//!
//! jpnknは、板への書き込みをMQTT(WebSocket経由)で即座に配信している
//! (bbs.jpnkn.com/{板}/beta/fast のページと同じ仕組み)。これを購読すると、
//! datを定期的に取りに行くより早く、書き込まれた瞬間にレスを受け取れる。
//!
//! - 接続先: https://nf1.jpnkn.com/io/mq-server.json に載っているサーバー
//!   (暗号化ありのもの)に、wss://{node}:{port}/mqtt で繋ぐ
//! - 認証: jpnknのページに書かれている共通の接続情報(genkai / 7144)
//! - 購読: トピック "bbs/{板名}"
//! - 届くデータ: {"body":"名前<>メール<>日時<>本文<>タイトル","no":"番号",
//!   "bbsid":"板名","threadkey":"スレッドのキー"}
//!
//! MQTTのライブラリは使わず、必要な最小限(CONNECT/SUBSCRIBE/PINGREQ/PUBLISH)
//! だけをここで組み立てる(依存を増やさない方針)。

use super::source::RawPost;

pub const SERVER_LIST_URL: &str = "https://nf1.jpnkn.com/io/mq-server.json";
const USER: &str = "genkai";
const PASS: &str = "7144";
pub const KEEPALIVE_SECS: u16 = 60;

/// サーバー一覧から、暗号化ありで繋げるサーバーのURLを作る。
pub fn secure_servers(list: &serde_json::Value) -> Vec<String> {
    list.get("mq")
        .and_then(|m| m.as_array())
        .map(|nodes| {
            nodes
                .iter()
                .filter(|n| n.get("secure").and_then(|s| s.as_bool()).unwrap_or(false))
                .filter_map(|n| {
                    let node = n.get("node")?.as_str()?;
                    let port = n.get("port")?.as_u64()?;
                    Some(format!("wss://{node}:{port}/mqtt"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn mqtt_str(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() + 2);
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
    out
}

fn packet(header: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![header];
    let mut n = body.len();
    loop {
        let mut d = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            d |= 0x80;
        }
        out.push(d);
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    out
}

/// MQTT 3.1.1 のCONNECT(ユーザー名・パスワードあり、クリーンセッション)。
pub fn connect_packet(client_id: &str) -> Vec<u8> {
    let mut body = mqtt_str("MQTT");
    body.extend_from_slice(&[4, 0xC2]);
    body.extend_from_slice(&KEEPALIVE_SECS.to_be_bytes());
    body.extend(mqtt_str(client_id));
    body.extend(mqtt_str(USER));
    body.extend(mqtt_str(PASS));
    packet(0x10, &body)
}

pub fn subscribe_packet(topic: &str) -> Vec<u8> {
    let mut body = vec![0, 1]; // パケットID
    body.extend(mqtt_str(topic));
    body.push(0); // QoS 0
    packet(0x82, &body)
}

pub fn pingreq_packet() -> Vec<u8> {
    vec![0xC0, 0]
}

#[derive(Debug, PartialEq)]
pub enum Incoming {
    ConnAck(u8),
    SubAck,
    Publish { topic: String, payload: Vec<u8> },
    Other,
}

/// 受信したバイト列(WebSocketのフレームの区切りとMQTTのパケットの区切りは
/// 一致しないことがあるので、溜めておいたもの)から、完全なパケットを
/// 取り出せるだけ取り出す。残り(途中までのパケット)はbufに残す。
pub fn take_packets(buf: &mut Vec<u8>) -> Vec<Incoming> {
    let mut out = Vec::new();
    loop {
        if buf.len() < 2 {
            break;
        }
        let mut len: usize = 0;
        let mut mul: usize = 1;
        let mut i = 1;
        let mut complete_len = false;
        while i < buf.len() && i <= 4 {
            let b = buf[i];
            len += (b & 0x7F) as usize * mul;
            mul *= 128;
            i += 1;
            if b & 0x80 == 0 {
                complete_len = true;
                break;
            }
        }
        if !complete_len || buf.len() < i + len {
            break; // まだ全部届いていない
        }
        let header = buf[0];
        let body: Vec<u8> = buf[i..i + len].to_vec();
        buf.drain(..i + len);
        out.push(match header >> 4 {
            2 => Incoming::ConnAck(body.get(1).copied().unwrap_or(0xFF)),
            9 => Incoming::SubAck,
            3 => {
                let qos = (header >> 1) & 0x03;
                if body.len() < 2 {
                    Incoming::Other
                } else {
                    let tl = u16::from_be_bytes([body[0], body[1]]) as usize;
                    let start = 2 + tl + if qos > 0 { 2 } else { 0 };
                    if body.len() < 2 + tl || body.len() < start {
                        Incoming::Other
                    } else {
                        Incoming::Publish {
                            topic: String::from_utf8_lossy(&body[2..2 + tl]).into_owned(),
                            payload: body[start..].to_vec(),
                        }
                    }
                }
            }
            _ => Incoming::Other,
        });
    }
    out
}

/// 届いたデータ(JSON)から、スレッドのキーとレスを取り出す。
pub fn parse_payload(payload: &[u8]) -> Option<(String, RawPost)> {
    let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let thread_key = v.get("threadkey")?.as_str()?.to_string();
    let no: u32 = match v.get("no")? {
        serde_json::Value::String(s) => s.parse().ok()?,
        n => n.as_u64()? as u32,
    };
    let body = v.get("body")?.as_str()?;
    let f: Vec<&str> = body.split("<>").collect();
    Some((
        thread_key,
        RawPost {
            no,
            name: f.first().copied().unwrap_or_default().to_string(),
            date: f.get(2).copied().unwrap_or_default().to_string(),
            body_html: f.get(3).copied().unwrap_or_default().to_string(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_packets() {
        let c = connect_packet("id");
        assert_eq!(c[0], 0x10);
        assert_eq!(&c[2..8], &[0, 4, b'M', b'Q', b'T', b'T']);
        assert_eq!(c[8], 4);
        assert_eq!(c[9], 0xC2);
        assert_eq!(c[1] as usize, c.len() - 2);
        let s = subscribe_packet("bbs/example");
        assert_eq!(s[0], 0x82);
        assert_eq!(&s[2..4], &[0, 1]);
        assert_eq!(*s.last().unwrap(), 0);
        // 128バイト以上は残りの長さが2バイトになる
        assert_eq!(packet(0x30, &vec![0u8; 200])[1..3], [0xC8, 0x01]);
    }

    #[test]
    fn takes_split_and_joined_packets() {
        let publish = |topic: &str, payload: &str| {
            let mut body = mqtt_str(topic);
            body.extend_from_slice(payload.as_bytes());
            packet(0x30, &body)
        };
        let mut all = vec![0x20, 0x02, 0x00, 0x00]; // CONNACK
        all.extend(publish("bbs/x", "{\"a\":1}"));
        all.extend(publish("bbs/x", &"あ".repeat(100)));
        // 途中で切れて届いても、溜めておけば正しく取り出せる
        let (first, second) = all.split_at(9);
        let mut buf = first.to_vec();
        let got = take_packets(&mut buf);
        assert_eq!(got, vec![Incoming::ConnAck(0)]);
        buf.extend_from_slice(second);
        let got = take_packets(&mut buf);
        assert_eq!(got.len(), 2);
        assert!(buf.is_empty());
        match &got[1] {
            Incoming::Publish { topic, payload } => {
                assert_eq!(topic, "bbs/x");
                assert_eq!(String::from_utf8_lossy(payload), "あ".repeat(100));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_jpnkn_payload() {
        let p = r#"{"body":"名無しさん<>sage<>2026/10/09(金) 16:02:55.38 ID:abc<>&gt;&gt;552<br>テスト<>","no":"555","bbsid":"example","threadkey":"1700000001"}"#;
        let (key, post) = parse_payload(p.as_bytes()).unwrap();
        assert_eq!(key, "1700000001");
        assert_eq!(post.no, 555);
        assert_eq!(post.name, "名無しさん");
        assert_eq!(post.body_html, "&gt;&gt;552<br>テスト");
        assert!(parse_payload(b"not json").is_none());
    }

    /// 実際のjpnknの即時通知に繋いで、書き込みを1件受け取る(ネットワークに繋ぐので
    /// 普段は実行しない)。jpnkn全体("bbs/#")を購読するので、数十秒以内に届く。
    #[test]
    #[ignore]
    fn receives_real_jpnkn_notification() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;
        tauri::async_runtime::block_on(async {
            let list: serde_json::Value = reqwest::get(SERVER_LIST_URL).await.unwrap().json().await.unwrap();
            let url = secure_servers(&list).into_iter().next().expect("サーバー一覧");
            let mut req = url.as_str().into_client_request().unwrap();
            req.headers_mut().insert("Sec-WebSocket-Protocol", "mqtt".parse().unwrap());
            let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            let (mut write, mut read) = ws.split();
            write.send(Message::Binary(connect_packet("reacast-test"))).await.unwrap();
            let mut buf = Vec::new();
            let mut got = None;
            let end = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            while got.is_none() && tokio::time::Instant::now() < end {
                let Ok(Some(Ok(Message::Binary(d)))) = tokio::time::timeout(std::time::Duration::from_secs(5), read.next()).await else {
                    continue;
                };
                buf.extend_from_slice(&d);
                for p in take_packets(&mut buf) {
                    match p {
                        Incoming::ConnAck(rc) => {
                            assert_eq!(rc, 0, "接続が受け付けられるはず");
                            write.send(Message::Binary(subscribe_packet("bbs/#"))).await.unwrap();
                        }
                        Incoming::Publish { payload, .. } => {
                            if got.is_none() {
                                got = parse_payload(&payload);
                            }
                        }
                        _ => {}
                    }
                }
            }
            let (key, post) = got.expect("60秒以内に書き込みが届くはず");
            eprintln!("thread={key} no={} body={:?}", post.no, super::super::text::html_to_text(&post.body_html));
        });
    }

    #[test]
    fn picks_secure_servers() {
        let list = serde_json::json!({ "mq": [
            { "node": "a.mq.jpnkn.com", "port": 9091, "secure": true },
            { "node": "bbs.jpnkn.com", "port": 9090, "secure": false }
        ] });
        assert_eq!(secure_servers(&list), vec!["wss://a.mq.jpnkn.com:9091/mqtt"]);
    }
}
