//! コメント一覧の画像URLのサムネイル(配信者が確認してから見せる)。
//!
//! 配信中に性的な画像・グロ画像のURLを書かれても配信に映らないように、コメント一覧(OBSと
//! デスクトップの一覧。どちらも comments.html)では、画像は最初は暗幕(黒い四角)で出す。
//! 配信者には、画面キャプチャに映らない小さな窓(image_check.html)で1枚ずつ見せて、
//! 「見せる」のホットキー(またはボタン)を押した画像だけ、一覧の暗幕を外す。
//! 「見せない」か×を押すか、設定の秒数(style.list.imageCheckSeconds。既定は0=無制限)が
//! たつと、次の画像へ進み(窓の側で数える)、暗幕のまま。
//!
//! 画像はReaCastが1回だけダウンロードして、自分のHTTP(obs_bridgeの /cimg/<番号>)から配る。
//! 配信者が見た画像と、暗幕を外した時に映る画像が必ず同じになる(後から中身を差し替えられない)。
//! 同じURLは1つにまとめる(1回決めれば、そのURLは全部同じ)。
//! 画像はメモリの中だけに置いて、ディスクには保存しない(合計50MBまで。古いものから捨てる。
//! コメント一覧を空にした時とReaCastを閉じた時に全部消える)。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "imageCheck";
/// 1枚の大きさの上限(これより大きい画像は出さない)
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// 覚えておく画像の数(古いものから忘れる。忘れた画像は暗幕のまま)
const MAX_ENTRIES: usize = 100;
/// 覚えておく画像の合計の大きさ(メモリの中だけ。ディスクには保存しない)
const MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;
/// 確認待ちの列に残す数(一度にたくさん貼られた時は新しいものだけ。古いものは暗幕のまま)
const MAX_WAITING: usize = 10;
/// 1件のレスから拾う画像の数(comments.htmlと同じ)
const MAX_PER_RES: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// ダウンロード中
    Loading,
    /// 配信者の確認待ち
    Waiting,
    Shown,
    Hidden,
    /// 読めなかった・画像ではなかった(一覧からも消す)
    Failed,
}

struct Entry {
    id: u32,
    url: String,
    status: Status,
    data: Option<Arc<(Vec<u8>, &'static str)>>,
}

#[derive(Default)]
struct State {
    next_id: u32,
    entries: VecDeque<Entry>,
    /// 確認待ちの列(先頭が今窓に出している画像)
    queue: VecDeque<u32>,
}

/// 起動ごとに違う印。画像の番号は起動のたびに1から数え直すので、前に起動した時の同じ番号の
/// 画像が、画面(WebView2・OBS)に残っていて出てしまわないように、画像のアドレスに付ける
fn run_tag() -> u32 {
    static TAG: OnceLock<u32> = OnceLock::new();
    *TAG.get_or_init(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() ^ d.as_secs() as u32).unwrap_or(1)
    })
}

/// 画像のアドレス(obs_bridgeのHTTP)
fn src(id: u32) -> String {
    format!("http://127.0.0.1:18772/cimg/{id}?r={}", run_tag())
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { next_id: 1, ..Default::default() }))
}

static APP: OnceLock<AppHandle> = OnceLock::new();

/// 起動時に1回だけ呼ぶ
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// 本文の中の画像のURL(comments.htmlのC.IMAGE_URL_REと同じもの: 空白などで区切った
/// http(s)のURLのうち、.png .jpg .jpeg .gif .webp で終わるもの。後ろに?…が付いていてもよい)
pub fn image_urls(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in body.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>') {
        let Some(start) = token.find("http://").or_else(|| token.find("https://")) else { continue };
        let url = &token[start..];
        let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
        let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| path.ends_with(e));
        if is_image && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
            if out.len() >= MAX_PER_RES {
                break;
            }
        }
    }
    out
}

/// 中身の先頭のバイトから画像の種類を決める(サーバーの言うContent-Typeは信じない)
fn sniff(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if d.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if d.len() > 12 && &d[0..4] == b"RIFF" && &d[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// コメント一覧に1件届いた時(obs_bridgeのpush_comment)。サムネイルを出す設定で、
/// まだ見たことのない画像のURLがあれば、ダウンロードして確認待ちに並べる
pub fn on_comment(res: &serde_json::Value, style: Option<&serde_json::Value>) {
    let on = style.and_then(|s| s.pointer("/list/thumbnails")).and_then(|v| v.as_str()) == Some("chat");
    if !on || res.get("aa").and_then(|v| v.as_bool()) == Some(true) || res.get("system").and_then(|v| v.as_bool()) == Some(true) {
        return;
    }
    let urls = image_urls(res.get("body").and_then(|v| v.as_str()).unwrap_or_default());
    if urls.is_empty() {
        return;
    }
    let mut new = Vec::new();
    {
        let mut st = state().lock().unwrap();
        for url in urls {
            if st.entries.iter().any(|e| e.url == url) {
                continue;
            }
            let id = st.next_id;
            st.next_id += 1;
            st.entries.push_back(Entry { id, url: url.clone(), status: Status::Loading, data: None });
            new.push((id, url));
        }
        while st.entries.len() > MAX_ENTRIES {
            if let Some(old) = st.entries.pop_front() {
                st.queue.retain(|&q| q != old.id);
            }
        }
    }
    if new.is_empty() {
        return;
    }
    publish();
    for (id, url) in new {
        tauri::async_runtime::spawn(async move {
            let got = download(&url).await;
            {
                let mut st = state().lock().unwrap();
                let Some(e) = st.entries.iter_mut().find(|e| e.id == id) else { return };
                match got {
                    Some(d) => {
                        e.data = Some(Arc::new(d));
                        e.status = Status::Waiting;
                        st.queue.push_back(id);
                        // 多すぎる時は、今出している1枚(先頭)の次の、一番古いものから暗幕のままにする
                        while st.queue.len() > MAX_WAITING {
                            let dropped = st.queue.remove(1).unwrap_or_default();
                            if let Some(e) = st.entries.iter_mut().find(|e| e.id == dropped) {
                                e.status = Status::Hidden;
                            }
                        }
                        trim_memory(&mut st);
                    }
                    None => e.status = Status::Failed,
                }
            }
            publish();
            show_window();
        });
    }
}

/// 覚えている画像の合計がMAX_TOTAL_BYTESを超えたら、古い画像から捨てる(今窓に出している1枚は残す)。
/// 捨てた画像は、確認待ちなら暗幕のまま、見せていたものは一覧から消える
fn trim_memory(st: &mut State) {
    let current = st.queue.front().copied();
    let mut total: usize = st.entries.iter().filter_map(|e| e.data.as_ref()).map(|d| d.0.len()).sum();
    let mut i = 0;
    while total > MAX_TOTAL_BYTES && i < st.entries.len() {
        let e = &mut st.entries[i];
        i += 1;
        if Some(e.id) == current {
            continue;
        }
        let Some(d) = e.data.take() else { continue };
        total -= d.0.len();
        let id = e.id;
        e.status = match e.status {
            Status::Shown => Status::Failed,
            _ => Status::Hidden,
        };
        st.queue.retain(|&q| q != id);
    }
}

/// インターネット上の普通の場所か(このPC・家のネットワーク・特別な用途のアドレスではない)。
/// 視聴者が書いたURLで、配信者のルーターやPCの中のソフトにアクセスさせられないように
fn is_public(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..128).contains(&o[1])))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// つないでいいURLか(http(s)で、アドレスが直接書かれている時はインターネット上の場所)。
/// 名前で書かれている時は、名前を引いた結果をPublicOnlyで確かめる
fn url_ok(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    // URLの決まりで、「2130706433」「0x7f.1」のような書き方も、ここでは普通のアドレスの形になっている
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => is_public(ip),
        Err(_) => host != "localhost" && !host.ends_with(".localhost"),
    }
}

/// 名前を引いた結果から、インターネット上ではないアドレスを外す(全部外れたらつながない)。
/// 転送された先も、ここを通る
struct PublicOnly;
impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.filter(|a| is_public(a.ip())).collect();
            if addrs.is_empty() {
                return Err("インターネット上のアドレスではありません".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// 画像の縦横(ファイルの最初の方だけを読む。分からなければNone)
fn dimensions(d: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| -> Option<u32> { Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]) as u32) };
    let le16 = |i: usize| -> Option<u32> { Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]) as u32) };
    let be32 = |i: usize| -> Option<u32> { Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?)) };
    match sniff(d)? {
        "image/png" => Some((be32(16)?, be32(20)?)),
        "image/gif" => Some((le16(6)?, le16(8)?)),
        "image/webp" => match d.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let b = d.get(21..25)?;
                let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => {
                let w = u32::from_le_bytes([*d.get(24)?, *d.get(25)?, *d.get(26)?, 0]) + 1;
                let h = u32::from_le_bytes([*d.get(27)?, *d.get(28)?, *d.get(29)?, 0]) + 1;
                Some((w, h))
            }
            _ => None,
        },
        _ => {
            // JPEG: 印(FF xx)を順に見て、大きさの書いてある所(SOF)を探す
            let mut i = 2;
            while i + 9 < d.len() {
                if d[i] != 0xff {
                    return None;
                }
                let m = d[i + 1];
                if m == 0xff {
                    i += 1;
                    continue;
                }
                if (0xc0..=0xcf).contains(&m) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
                    return Some((be16(i + 7)?, be16(i + 5)?));
                }
                i += 2 + be16(i + 2)? as usize;
            }
            None
        }
    }
}

/// 縦横の上限。ファイルは小さいのに縦横がとても大きい画像(画像爆弾)は、表示する時に
/// メモリを食い尽くしてOBSやReaCastを落とすことがあるので出さない
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 4096 * 4096;

async fn download(url: &str) -> Option<(Vec<u8>, &'static str)> {
    // 連投された時に、たくさん同時に取りに行かない
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);
    let _slot = SLOTS.acquire().await.ok()?;
    let url = reqwest::Url::parse(url).ok()?;
    if !url_ok(&url) {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(concat!("Mozilla/5.0 ReaCast/", env!("CARGO_PKG_VERSION")))
        .dns_resolver(Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 3 || !url_ok(a.url()) {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .build()
        .ok()?;
    let mut resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n as usize > MAX_BYTES) {
        return None;
    }
    let mut data = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_BYTES {
            return None;
        }
    }
    let mime = sniff(&data)?;
    let (w, h) = dimensions(&data)?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    Some((data, mime))
}

/// obs_bridgeのHTTP(/cimg/<番号>)が返す画像
pub fn data(id: u32) -> Option<Arc<(Vec<u8>, &'static str)>> {
    state().lock().unwrap().entries.iter().find(|e| e.id == id).and_then(|e| e.data.clone())
}

/// コメント一覧に送る、URLごとの状態({ url: { id, status, src } })
fn list_state(st: &State) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for e in &st.entries {
        let status = match e.status {
            Status::Loading | Status::Waiting => "wait",
            Status::Shown => "shown",
            Status::Hidden => "hidden",
            Status::Failed => "failed",
        };
        m.insert(e.url.clone(), serde_json::json!({ "id": e.id, "status": status, "src": src(e.id) }));
    }
    serde_json::Value::Object(m)
}

/// 確認の窓に出すもの(今の1枚と、あと何枚待っているか)
fn window_state(st: &State) -> serde_json::Value {
    let current = st.queue.front().and_then(|id| st.entries.iter().find(|e| e.id == *id));
    serde_json::json!({
        "current": current.map(|e| serde_json::json!({ "id": e.id, "url": e.url, "src": src(e.id) })),
        "waiting": st.queue.len().saturating_sub(1),
    })
}

fn publish() {
    let Some(app) = APP.get() else { return };
    let (list, win) = {
        let st = state().lock().unwrap();
        (list_state(&st), window_state(&st))
    };
    app.state::<crate::obs_bridge::ObsBridge>().set_image_state(list);
    let _ = app.emit_to(LABEL, "imagecheck:changed", win);
}

/// 見せる・見せないを決めて、次の画像へ進む
fn decide(id: u32, show: bool) {
    let empty = {
        let mut st = state().lock().unwrap();
        st.queue.retain(|&q| q != id);
        if let Some(e) = st.entries.iter_mut().find(|e| e.id == id) {
            if e.status == Status::Waiting {
                e.status = if show { Status::Shown } else { Status::Hidden };
            }
        }
        st.queue.is_empty()
    };
    publish();
    if empty {
        if let Some(w) = APP.get().and_then(|a| a.get_webview_window(LABEL)) {
            let _ = w.hide();
        }
    }
}

/// 「見せる」「見せない」のホットキー: 今窓に出している画像の暗幕を外す / 暗幕のまま次へ
pub fn decide_current(show: bool) {
    let id = state().lock().unwrap().queue.front().copied();
    if let Some(id) = id {
        decide(id, show);
    }
}

/// コメント一覧を空にした時(取得の開始・やり直し)。覚えている画像も忘れる
pub fn clear() {
    {
        let mut st = state().lock().unwrap();
        st.entries.clear();
        st.queue.clear();
    }
    publish();
    if let Some(w) = APP.get().and_then(|a| a.get_webview_window(LABEL)) {
        let _ = w.hide();
    }
}

/// 確認の窓を出す(無ければ作る)。画面キャプチャ・画面共有には映らない窓にする
fn show_window() {
    let Some(app) = APP.get() else { return };
    if state().lock().unwrap().queue.is_empty() {
        return;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        if !w.is_visible().unwrap_or(true) {
            let _ = w.show();
            crate::keep_off_taskbar(&w);
        }
        return;
    }
    let (w, h) = crate::window_size::saved(app, LABEL, (300.0, 290.0), (200.0, 180.0));
    // ReaCastの窓はWebView2を共有しているので、ほかの窓と同じ起動オプションにする(違うと作れない)
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("image_check.html".into()))
        .additional_browser_args(crate::WEBVIEW_ARGS)
        .title("ReaCast 画像の確認")
        .inner_size(w, h)
        .min_inner_size(200.0, 180.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build();
    let win = match built {
        Ok(w) => w,
        Err(e) => {
            log::warn!("画像の確認の窓を作れませんでした: {e}");
            return;
        }
    };
    exclude_from_capture(&win);
    // 最初は、メインのモニターの右下(覚えた位置があればそこ。window_size::track)
    if let (Ok(Some(m)), Ok(size)) = (win.primary_monitor(), win.outer_size()) {
        let a = m.work_area();
        let x = a.position.x + a.size.width as i32 - size.width as i32 - 16;
        let y = a.position.y + a.size.height as i32 - size.height as i32 - 16;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
    crate::window_size::track(&win);
    let _ = win.show();
    crate::keep_off_taskbar(&win);
}

/// OBSの画面キャプチャや画面共有に、この窓を映さない(Windows 10 2004以降。
/// それより前のWindowsでは、映る所が黒くなる)
fn exclude_from_capture(win: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR};
        if let Ok(h) = win.hwnd() {
            let hwnd = HWND(h.0 as isize as *mut core::ffi::c_void);
            unsafe {
                if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
                    let _ = SetWindowDisplayAffinity(hwnd, WDA_MONITOR);
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = win;
}

/// 確認の窓が開いた時に、今の状態を読む(ホットキーと、待つ秒数も)
#[tauri::command]
pub fn image_check_state(engine: tauri::State<Arc<crate::comments::CommentEngine>>) -> serde_json::Value {
    let mut v = window_state(&state().lock().unwrap());
    let s = engine.settings();
    v["hotkey"] = serde_json::Value::String(s.hotkey_show_image.clone());
    v["hotkeyHide"] = serde_json::Value::String(s.hotkey_hide_image.clone());
    v["seconds"] = serde_json::json!(s.style.pointer("/list/imageCheckSeconds").and_then(|x| x.as_f64()).unwrap_or(0.0));
    v
}

/// 確認の窓のボタン(見せる / 見せない / ×)と、待ち時間がたった時
#[tauri::command]
pub fn image_check_decide(id: u32, show: bool) {
    decide(id, show);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_image_urls_like_the_comment_list() {
        assert_eq!(
            image_urls("見て https://i.imgur.com/abc.PNG と http://x.jp/a.jpg?w=1 と https://x.jp/a.html"),
            vec!["https://i.imgur.com/abc.PNG", "http://x.jp/a.jpg?w=1"]
        );
        assert_eq!(image_urls("ttps://a.jp/b.png"), Vec::<String>::new());
        assert_eq!(image_urls("https://a.jp/b.png https://a.jp/b.png"), vec!["https://a.jp/b.png"]);
        assert_eq!(image_urls("<https://a.jp/c.webp>"), vec!["https://a.jp/c.webp"]);
    }

    #[test]
    fn forgets_old_images_when_memory_is_full() {
        let big = || Some(Arc::new((vec![0u8; 20 * 1024 * 1024], "image/png")));
        let mut st = State { next_id: 5, ..Default::default() };
        for (id, status) in [(1, Status::Shown), (2, Status::Waiting), (3, Status::Waiting), (4, Status::Waiting)] {
            st.entries.push_back(Entry { id, url: format!("u{id}"), status, data: big() });
        }
        st.queue.extend([2, 3, 4]);
        trim_memory(&mut st);
        // 80MB → 50MB以下: 古い1(見せていた)と、今出している2の次の3を捨てる
        let kept: Vec<u32> = st.entries.iter().filter(|e| e.data.is_some()).map(|e| e.id).collect();
        assert_eq!(kept, vec![2, 4]);
        assert!(st.entries[0].status == Status::Failed && st.entries[2].status == Status::Hidden);
        assert_eq!(st.queue, vec![2, 4]);
    }

    #[test]
    fn does_not_reach_into_the_streamers_network() {
        let ok = |u: &str| url_ok(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://i.imgur.com/a.png"));
        assert!(ok("http://8.8.8.8/a.png"));
        for bad in [
            "http://127.0.0.1:50080/a.png",
            "http://localhost/a.png",
            "http://a.localhost/a.png",
            "http://192.168.1.1/a.png",
            "http://10.0.0.5/a.png",
            "http://172.16.3.4/a.png",
            "http://169.254.169.254/a.png",
            "http://100.64.0.1/a.png",
            "http://0.0.0.0/a.png",
            "http://2130706433/a.png",
            "http://[::1]/a.png",
            "http://[fe80::1]/a.png",
            "http://[fd00::1]/a.png",
            "http://[::ffff:192.168.0.1]/a.png",
            "file:///C:/a.png",
        ] {
            assert!(!ok(bad), "{bad}");
        }
    }

    /// 本物のサイトから取ってくる(手で動かす時だけ: cargo test downloads_real_images -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn downloads_real_images() {
        tauri::async_runtime::block_on(async {
            let got = download("https://www.google.com/images/branding/googlelogo/2x/googlelogo_color_272x92dp.png").await;
            println!("google: {:?}", got.as_ref().map(|d| (d.0.len(), d.1, dimensions(&d.0))));
            assert!(got.is_some());
            // 普通のサイトの名前でも、引いた先がこのPCならつながない
            assert!(download("http://localtest.me/a.png").await.is_none());
        });
    }

    #[test]
    fn reads_image_sizes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 1, 0x2c, 0, 0, 0, 200]);
        assert_eq!(dimensions(&png), Some((300, 200)));
        assert_eq!(dimensions(b"GIF89a\x40\x01\xf0\x00"), Some((320, 240)));
        // JPEG: SOI, APP0(長さ4), SOF0(高さ480・幅640)
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 17, 8, 0x01, 0xe0, 0x02, 0x80, 3, 0, 0];
        assert_eq!(dimensions(&jpg), Some((640, 480)));
        assert_eq!(dimensions(b"<html>"), None);
    }

    #[test]
    fn tells_images_from_the_first_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"<html>"), None);
    }
}
