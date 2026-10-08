//! 掲示板からのレス取得。
//!
//! - 2ch互換の掲示板(jpnkn等): `板/dat/スレキー.dat`(Shift_JIS)を、前回までに
//!   読んだバイト数からの差分だけHTTPのRangeで取得する(新着が無ければ
//!   ほぼ通信しない。jpnknがRangeに対応していることは実際に確認済み)。
//! - したらば: `rawmode.cgi/カテゴリ/板/スレキー/開始番号-`(EUC-JP)で、
//!   前回の続きのレス番号から取得する。

use encoding_rs::{Encoding, EUC_JP, SHIFT_JIS};
use reqwest::header::{HeaderValue, RANGE};
use reqwest::{StatusCode, Url};

#[derive(Clone, Debug, PartialEq)]
pub enum Board {
    /// 2ch互換。origin例: `https://bbs.jpnkn.com`、board例: `ao33`
    Nch { origin: String, board: String },
    /// したらば。`https://jbbs.shitaraba.jp/bbs/read.cgi/{category}/{board}/{key}/`
    Shitaraba { category: String, board: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRef {
    pub board: Board,
    pub key: String,
}

impl ThreadRef {
    pub fn read_url(&self) -> String {
        match &self.board {
            Board::Nch { origin, board } => format!("{origin}/test/read.cgi/{board}/{}/", self.key),
            Board::Shitaraba { category, board } => {
                format!("https://jbbs.shitaraba.jp/bbs/read.cgi/{category}/{board}/{}/", self.key)
            }
        }
    }
}

/// スレッドのURL(または板のURL)を解釈する。板のURLだった場合はスレキーが
/// Noneになり、呼び出し側が板の一番新しいスレッドを選ぶ。
pub fn parse_url(input: &str) -> Result<(Board, Option<String>), String> {
    let url = Url::parse(input.trim()).map_err(|_| "URLの形式が正しくありません".to_string())?;
    let host = url.host_str().ok_or("URLにホスト名がありません")?.to_string();
    let segs: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();

    if host == "jbbs.shitaraba.jp" || host == "jbbs.livedoor.jp" {
        // /bbs/read.cgi/{cat}/{board}/{key}/  または  /{cat}/{board}/
        let rest: Vec<&str> = match segs.as_slice() {
            ["bbs", "read.cgi" | "rawmode.cgi", rest @ ..] => rest.to_vec(),
            rest => rest.to_vec(),
        };
        return match rest.as_slice() {
            [cat, board, key, ..] if is_key(key) => Ok((
                Board::Shitaraba { category: cat.to_string(), board: board.to_string() },
                Some(key.to_string()),
            )),
            [cat, board, ..] => Ok((
                Board::Shitaraba { category: cat.to_string(), board: board.to_string() },
                None,
            )),
            _ => Err("したらばのスレッド(または板)のURLを入力してください".into()),
        };
    }

    let origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default())
        + &url.port().map(|p| format!(":{p}")).unwrap_or_default();
    match segs.as_slice() {
        ["test", "read.cgi", board, key, ..] if is_key(key) => Ok((
            Board::Nch { origin, board: board.to_string() },
            Some(key.to_string()),
        )),
        [board, "dat", file] if file.ends_with(".dat") && is_key(&file[..file.len() - 4]) => Ok((
            Board::Nch { origin, board: board.to_string() },
            Some(file[..file.len() - 4].to_string()),
        )),
        [board] | [board, ""] => Ok((Board::Nch { origin, board: board.to_string() }, None)),
        _ => Err("掲示板のスレッド(または板)のURLを入力してください".into()),
    }
}

fn is_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Clone, Debug)]
pub struct RawPost {
    pub no: u32,
    pub name: String,
    pub date: String,
    pub body_html: String,
}

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub key: String,
    pub title: String,
    pub count: u32,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("Monazilla/1.00 ReaCast/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 1スレッド分の読み込み状態。pollを呼ぶたびに前回より後のレスだけを返す。
pub struct ThreadReader {
    pub thread: ThreadRef,
    pub title: Option<String>,
    pub last_no: u32,
    /// 2ch互換のdatで、ここまで(改行で終わる完全な行まで)読んだバイト数
    bytes_read: u64,
}

impl ThreadReader {
    pub fn new(thread: ThreadRef) -> Self {
        Self { thread, title: None, last_no: 0, bytes_read: 0 }
    }

    pub async fn poll(&mut self, client: &reqwest::Client) -> Result<Vec<RawPost>, String> {
        match self.thread.board.clone() {
            Board::Nch { origin, board } => self.poll_nch(client, &origin, &board).await,
            Board::Shitaraba { category, board } => self.poll_shitaraba(client, &category, &board).await,
        }
    }

    async fn poll_nch(
        &mut self,
        client: &reqwest::Client,
        origin: &str,
        board: &str,
    ) -> Result<Vec<RawPost>, String> {
        let url = format!("{origin}/{board}/dat/{}.dat", self.thread.key);
        let mut req = client.get(&url);
        if self.bytes_read > 0 {
            req = req.header(RANGE, HeaderValue::from_str(&format!("bytes={}-", self.bytes_read)).unwrap());
        }
        let resp = req.send().await.map_err(|e| format!("取得に失敗しました: {e}"))?;
        let status = resp.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE || status == StatusCode::NOT_MODIFIED {
            return Ok(Vec::new());
        }
        if !(status == StatusCode::OK || status == StatusCode::PARTIAL_CONTENT) {
            return Err(format!("取得に失敗しました(HTTP {})", status.as_u16()));
        }
        let partial = status == StatusCode::PARTIAL_CONTENT;
        let bytes = resp.bytes().await.map_err(|e| format!("取得に失敗しました: {e}"))?;
        // 最後の改行までの完全な行だけを扱う(書き込み途中で切れた行は次回に回す)
        let complete = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(i) => &bytes[..=i],
            None => return Ok(Vec::new()),
        };
        let (start_no, consumed_before) = if partial { (self.last_no, self.bytes_read) } else { (0, 0) };
        self.bytes_read = consumed_before + complete.len() as u64;

        let text = decode(SHIFT_JIS, complete);
        let mut posts = Vec::new();
        for (i, line) in text.split('\n').filter(|l| !l.is_empty()).enumerate() {
            let no = start_no + i as u32 + 1;
            let f: Vec<&str> = line.split("<>").collect();
            if no == 1 {
                if let Some(t) = f.get(4).filter(|t| !t.trim().is_empty()) {
                    self.title = Some(super::text::html_to_text(t));
                }
            }
            if no <= self.last_no {
                continue; // サーバーがRangeを無視して全体を返してきた場合
            }
            posts.push(RawPost {
                no,
                name: f.first().copied().unwrap_or_default().to_string(),
                date: f.get(2).copied().unwrap_or_default().to_string(),
                body_html: f.get(3).copied().unwrap_or_default().to_string(),
            });
        }
        if let Some(p) = posts.last() {
            self.last_no = p.no;
        }
        Ok(posts)
    }

    async fn poll_shitaraba(
        &mut self,
        client: &reqwest::Client,
        category: &str,
        board: &str,
    ) -> Result<Vec<RawPost>, String> {
        let url = format!(
            "https://jbbs.shitaraba.jp/bbs/rawmode.cgi/{category}/{board}/{}/{}-",
            self.thread.key,
            self.last_no + 1
        );
        let resp = client.get(&url).send().await.map_err(|e| format!("取得に失敗しました: {e}"))?;
        if let Some(err) = resp.headers().get("ERROR") {
            let msg = err.to_str().unwrap_or("");
            // 新着が無いだけの場合も取得範囲外として返ってくることがある
            if msg.contains("STORAGE IN") || msg.contains("BBS NOT FOUND") {
                return Err(format!("スレッドが見つかりません({msg})"));
            }
        }
        if !resp.status().is_success() {
            return Err(format!("取得に失敗しました(HTTP {})", resp.status().as_u16()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("取得に失敗しました: {e}"))?;
        let text = decode(EUC_JP, &bytes);
        let mut posts = Vec::new();
        for line in text.split('\n').filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split("<>").collect();
            let Some(no) = f.first().and_then(|n| n.trim().parse::<u32>().ok()) else {
                continue;
            };
            if let Some(t) = f.get(5).filter(|t| !t.trim().is_empty()) {
                self.title = Some(super::text::html_to_text(t));
            }
            if no <= self.last_no {
                continue;
            }
            let id = f.get(6).copied().unwrap_or_default();
            let date = f.get(3).copied().unwrap_or_default();
            posts.push(RawPost {
                no,
                name: f.get(1).copied().unwrap_or_default().to_string(),
                date: if id.is_empty() { date.to_string() } else { format!("{date} ID:{id}") },
                body_html: f.get(4).copied().unwrap_or_default().to_string(),
            });
        }
        if let Some(p) = posts.iter().map(|p| p.no).max() {
            self.last_no = p;
        }
        Ok(posts)
    }
}

/// 板のスレッド一覧(subject.txt)を取得する。
pub async fn list_threads(client: &reqwest::Client, board: &Board) -> Result<Vec<ThreadInfo>, String> {
    let (url, enc) = match board {
        Board::Nch { origin, board } => (format!("{origin}/{board}/subject.txt"), SHIFT_JIS),
        Board::Shitaraba { category, board } => {
            (format!("https://jbbs.shitaraba.jp/{category}/{board}/subject.txt"), EUC_JP)
        }
    };
    let resp = client.get(&url).send().await.map_err(|e| format!("スレッド一覧の取得に失敗しました: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("スレッド一覧の取得に失敗しました(HTTP {})", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("スレッド一覧の取得に失敗しました: {e}"))?;
    Ok(parse_subject(&decode(enc, &bytes), matches!(board, Board::Shitaraba { .. })))
}

fn parse_subject(text: &str, shitaraba: bool) -> Vec<ThreadInfo> {
    let mut out: Vec<ThreadInfo> = Vec::new();
    for line in text.lines() {
        let (file, rest) = if shitaraba {
            match line.split_once(',') {
                Some(x) => x,
                None => continue,
            }
        } else {
            match line.split_once("<>") {
                Some(x) => x,
                None => continue,
            }
        };
        let key = file.trim_end_matches(".dat").trim_end_matches(".cgi");
        if !is_key(key) || out.iter().any(|t| t.key == key) {
            continue; // したらばは末尾に先頭行と同じ行が重複して入っている
        }
        let rest = rest.trim_end();
        let (title, count) = match rest.rfind('(') {
            Some(i) if rest.ends_with(')') => (
                rest[..i].trim_end().to_string(),
                rest[i + 1..rest.len() - 1].trim().parse().unwrap_or(0),
            ),
            _ => (rest.to_string(), 0),
        };
        out.push(ThreadInfo { key: key.to_string(), title: super::text::html_to_text(&title), count });
    }
    out
}

/// 板の中から「一番新しい、まだ埋まっていないスレッド」を選ぶ(板のURLが
/// 入力された時用)。
pub fn pick_newest(threads: &[ThreadInfo]) -> Option<&ThreadInfo> {
    threads
        .iter()
        .filter(|t| t.count < 1000)
        .max_by_key(|t| t.key.parse::<u64>().unwrap_or(0))
}

/// 今のスレッドより後に立った、まだ埋まっていないスレッドのうち一番古いもの
/// (=次スレ)を選ぶ。keywordが空でなければ、タイトルにそれを含むものに限る。
pub fn pick_next(threads: &[ThreadInfo], current_key: &str, keyword: &str) -> Option<ThreadInfo> {
    let cur: u64 = current_key.parse().unwrap_or(0);
    threads
        .iter()
        .filter(|t| t.key.parse::<u64>().unwrap_or(0) > cur)
        .filter(|t| t.count < 1000)
        .filter(|t| keyword.is_empty() || t.title.contains(keyword))
        .min_by_key(|t| t.key.parse::<u64>().unwrap_or(0))
        .cloned()
}

fn decode(enc: &'static Encoding, bytes: &[u8]) -> String {
    enc.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jpnkn_thread_and_board_urls() {
        let (b, k) = parse_url("https://bbs.jpnkn.com/test/read.cgi/ao33/1791357173/").unwrap();
        assert_eq!(b, Board::Nch { origin: "https://bbs.jpnkn.com".into(), board: "ao33".into() });
        assert_eq!(k.as_deref(), Some("1791357173"));
        let (_, k) = parse_url("https://bbs.jpnkn.com/test/read.cgi/ao33/1791357173/l50").unwrap();
        assert_eq!(k.as_deref(), Some("1791357173"));
        let (b, k) = parse_url("https://bbs.jpnkn.com/ao33/").unwrap();
        assert_eq!(b, Board::Nch { origin: "https://bbs.jpnkn.com".into(), board: "ao33".into() });
        assert_eq!(k, None);
    }

    #[test]
    fn parses_shitaraba_urls() {
        let (b, k) = parse_url("https://jbbs.shitaraba.jp/bbs/read.cgi/game/12345/1700000000/").unwrap();
        assert_eq!(b, Board::Shitaraba { category: "game".into(), board: "12345".into() });
        assert_eq!(k.as_deref(), Some("1700000000"));
        let (_, k) = parse_url("https://jbbs.shitaraba.jp/game/12345/").unwrap();
        assert_eq!(k, None);
        let t = ThreadRef { board: b, key: "1700000000".into() };
        assert_eq!(t.read_url(), "https://jbbs.shitaraba.jp/bbs/read.cgi/game/12345/1700000000/");
    }

    #[test]
    fn rejects_garbage_urls() {
        assert!(parse_url("not a url").is_err());
        assert!(parse_url("https://bbs.jpnkn.com/a/b/c/d").is_err());
    }

    /// 実際のjpnknの板からスレッド一覧とレスを取得する(ネットワークに繋ぐので
    /// 普段は実行しない。`cargo test -- --ignored`で実行する)。差分取得
    /// (2回目のpollで新着が無ければ空)も確認する。
    #[test]
    #[ignore]
    fn fetches_real_jpnkn_board() {
        tauri::async_runtime::block_on(async {
            let client = http_client();
            let (board, _) = parse_url("https://bbs.jpnkn.com/ao33/").unwrap();
            let list = list_threads(&client, &board).await.unwrap();
            let newest = pick_newest(&list).expect("スレッドがあるはず");
            let mut reader = ThreadReader::new(ThreadRef { board, key: newest.key.clone() });
            let posts = reader.poll(&client).await.unwrap();
            assert!(!posts.is_empty());
            assert_eq!(posts[0].no, 1);
            assert_eq!(reader.last_no as usize, posts.len());
            assert!(reader.title.is_some());
            let again = reader.poll(&client).await.unwrap();
            assert!(again.is_empty(), "新着が無ければ差分は空");
            eprintln!(
                "title={:?} posts={} last={:?}",
                reader.title,
                posts.len(),
                posts.last().map(|p| super::super::text::html_to_text(&p.body_html))
            );
        });
    }

    #[test]
    fn parses_subject_lines_and_picks_threads() {
        let nch = parse_subject(
            "1791357173.dat<>539 (69)\n1790705505.dat<>538 (1001)\n1790309432.dat<>雑談 (part2) (12)\n",
            false,
        );
        assert_eq!(nch.len(), 3);
        assert_eq!(nch[2].title, "雑談 (part2)");
        assert_eq!(nch[2].count, 12);
        assert_eq!(pick_newest(&nch).unwrap().key, "1791357173");
        assert_eq!(pick_next(&nch, "1790705505", "").unwrap().key, "1791357173");
        assert!(pick_next(&nch, "1791357173", "").is_none());
        assert!(pick_next(&nch, "1790705505", "存在しない").is_none());

        let sh = parse_subject("1700000002.cgi,次のスレ(3)\n1700000001.cgi,前のスレ(1000)\n1700000002.cgi,次のスレ(3)\n", true);
        assert_eq!(sh.len(), 2);
        assert_eq!(pick_next(&sh, "1700000001", "").unwrap().title, "次のスレ");
    }
}
