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
    /// 2ch互換。origin例: `https://bbs.jpnkn.com`、board例: `example`
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

/// 板の中から「今使っている(現行の)スレッド」を選ぶ(板のURLが入力された時用)。
///
/// 埋まった(1000に達した)スレッドのうち一番新しいものの、すぐ次に立った
/// まだ埋まっていないスレッドを選ぶ。以前は「一番新しい、まだ埋まっていない
/// スレッド」を選んでいたため、現行スレが終わる前に次スレを建てておくと、
/// 開始・再起動した時に現行スレではなく建てたばかりの次スレを読んでしまっていた。
/// keywordが空でなければタイトルにそれを含むスレッドだけで判断する(合うものが
/// 無ければ条件なしで選び直す)。
pub fn pick_current<'a>(threads: &'a [ThreadInfo], keyword: &str) -> Option<&'a ThreadInfo> {
    let key = |t: &ThreadInfo| t.key.parse::<u64>().unwrap_or(0);
    let choose = |use_keyword: bool| {
        let pool: Vec<&ThreadInfo> = threads
            .iter()
            .filter(|t| !use_keyword || keyword.is_empty() || t.title.contains(keyword))
            .collect();
        let last_full = pool.iter().filter(|t| t.count >= 1000).map(|t| key(t)).max().unwrap_or(0);
        pool.into_iter()
            .filter(|t| t.count < 1000 && key(t) > last_full)
            .min_by_key(|t| key(t))
    };
    choose(true).or_else(|| choose(false))
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

/// 次スレを建てる(2ch互換の掲示板のみ。`/test/bbs.cgi`にShift_JISのフォームで
/// POSTする、ブラウザの「スレッド作成」ボタンと同じ送り方)。
pub async fn create_thread(
    client: &reqwest::Client,
    board: &Board,
    title: &str,
    name: &str,
    mail: &str,
    body: &str,
) -> Result<(), String> {
    let Board::Nch { origin, board } = board else {
        return Err("スレッドの自動作成は、今のところ2ちゃんねる互換の掲示板(jpnkn等)だけに対応しています".into());
    };
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let form = [
        ("bbs", board.as_str()),
        ("subject", title),
        ("FROM", name),
        ("mail", mail),
        ("MESSAGE", body),
        ("time", &time.to_string()),
        ("submit", "スレッド作成"),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", sjis_form_encode(v)))
    .collect::<Vec<_>>()
    .join("&");
    let resp = client
        .post(format!("{origin}/test/bbs.cgi"))
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::REFERER, format!("{origin}/{board}/"))
        .body(form)
        .send()
        .await
        .map_err(|e| format!("スレッドの作成に失敗しました: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| format!("スレッドの作成に失敗しました: {e}"))?;
    let html = decode(SHIFT_JIS, &bytes);
    check_post_result(status.is_success(), &html)
}

/// bbs.cgiの応答から成功/失敗を判定する(2ch互換の掲示板は、成功時に
/// タイトルが「書きこみました」のページを返す)。
fn check_post_result(http_ok: bool, html: &str) -> Result<(), String> {
    let title = html
        .find("<title>")
        .and_then(|i| html[i + 7..].find("</title>").map(|j| html[i + 7..i + 7 + j].trim().to_string()))
        .unwrap_or_default();
    if http_ok && (title.contains("書きこみました") || title.contains("書き込みました")) {
        return Ok(());
    }
    let detail = super::text::html_to_text(&html.replace('\n', " "));
    let detail: String = detail.chars().take(120).collect();
    Err(format!("スレッドの作成に失敗しました: {}", if title.is_empty() { detail } else { title }))
}

/// Shift_JISにしてからパーセントエンコードする(2ch互換のbbs.cgiの送り方)。
fn sjis_form_encode(s: &str) -> String {
    let (bytes, _, _) = SHIFT_JIS.encode(s);
    let mut out = String::with_capacity(bytes.len() * 3);
    for &b in bytes.iter() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(enc: &'static Encoding, bytes: &[u8]) -> String {
    enc.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jpnkn_thread_and_board_urls() {
        let (b, k) = parse_url("https://bbs.jpnkn.com/test/read.cgi/example/1700000001/").unwrap();
        assert_eq!(b, Board::Nch { origin: "https://bbs.jpnkn.com".into(), board: "example".into() });
        assert_eq!(k.as_deref(), Some("1700000001"));
        let (_, k) = parse_url("https://bbs.jpnkn.com/test/read.cgi/example/1700000001/l50").unwrap();
        assert_eq!(k.as_deref(), Some("1700000001"));
        let (b, k) = parse_url("https://bbs.jpnkn.com/example/").unwrap();
        assert_eq!(b, Board::Nch { origin: "https://bbs.jpnkn.com".into(), board: "example".into() });
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

    /// 実際の2ch互換の掲示板からスレッド一覧とレスを取得する(ネットワークに
    /// 繋ぐので普段は実行しない)。使う板は環境変数で指定する:
    /// `REACAST_TEST_BOARD_URL=https://bbs.jpnkn.com/板名/ cargo test -- --ignored`
    /// 差分取得(2回目のpollで新着が無ければ空)も確認する。
    #[test]
    #[ignore]
    fn fetches_real_board() {
        let Ok(board_url) = std::env::var("REACAST_TEST_BOARD_URL") else {
            eprintln!("REACAST_TEST_BOARD_URLが未設定のため省略");
            return;
        };
        tauri::async_runtime::block_on(async {
            let client = http_client();
            let (board, _) = parse_url(&board_url).unwrap();
            let list = list_threads(&client, &board).await.unwrap();
            let newest = pick_current(&list, "").expect("スレッドがあるはず");
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
    fn sjis_form_encoding_and_post_result() {
        assert_eq!(sjis_form_encode("540"), "540");
        assert_eq!(sjis_form_encode("あ a&"), "%82%A0+a%26");
        assert!(check_post_result(true, "<html><title>書きこみました。</title></html>").is_ok());
        let err = check_post_result(true, "<html><title>ＥＲＲＯＲ！</title><body>スレ立てすぎです</body></html>");
        assert!(err.unwrap_err().contains("ＥＲＲＯＲ"));
        assert!(check_post_result(false, "<title>書きこみました</title>").is_err());
    }

    #[test]
    fn parses_subject_lines_and_picks_threads() {
        let nch = parse_subject(
            "1700000003.dat<>539 (69)\n1700000002.dat<>538 (1001)\n1700000001.dat<>雑談 (part2) (12)\n",
            false,
        );
        assert_eq!(nch.len(), 3);
        assert_eq!(nch[2].title, "雑談 (part2)");
        assert_eq!(nch[2].count, 12);
        assert_eq!(pick_current(&nch, "").unwrap().key, "1700000003");
        assert_eq!(pick_next(&nch, "1700000002", "").unwrap().key, "1700000003");
        assert!(pick_next(&nch, "1700000003", "").is_none());
        assert!(pick_next(&nch, "1700000002", "存在しない").is_none());

        // 現行スレ(539)が終わる前に次スレ(540)を建てていても、現行スレを選ぶ
        let early = parse_subject(
            "1700000004.dat<>540 (1)\n1700000003.dat<>539 (69)\n1700000002.dat<>538 (1001)\n1700000001.dat<>537 (1001)\n",
            false,
        );
        assert_eq!(pick_current(&early, "").unwrap().title, "539");
        // 現行スレが1000に達したら、次スレへ(既存のpick_next)
        assert_eq!(pick_next(&early, "1700000003", "").unwrap().title, "540");
        // 埋まったスレッドが一覧に無い板では、まだ埋まっていない一番古いスレッド
        let fresh = parse_subject("1700000006.dat<>2 (3)\n1700000005.dat<>1 (500)\n", false);
        assert_eq!(pick_current(&fresh, "").unwrap().title, "1");
        // キーワードがあれば、それを含むスレッドだけで判断する(避難所などを避ける)
        let mixed = parse_subject(
            "1700000009.dat<>避難所 (5)\n1700000008.dat<>配信スレ12 (40)\n1700000007.dat<>配信スレ11 (1001)\n",
            false,
        );
        assert_eq!(pick_current(&mixed, "配信スレ").unwrap().title, "配信スレ12");
        assert_eq!(pick_current(&mixed, "存在しない").unwrap().title, "配信スレ12");

        let sh = parse_subject("1700000002.cgi,次のスレ(3)\n1700000001.cgi,前のスレ(1000)\n1700000002.cgi,次のスレ(3)\n", true);
        assert_eq!(sh.len(), 2);
        assert_eq!(pick_next(&sh, "1700000001", "").unwrap().title, "次のスレ");
    }
}
