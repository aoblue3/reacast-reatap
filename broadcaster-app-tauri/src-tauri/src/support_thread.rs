//! 「要望・質問スレを開く」: 作者の掲示板の要望・質問スレッドを、いつものブラウザで開く。
//!
//! スレッドは埋まるたびに「… Part2」「… Part3」と立て直すので、開く時に板のスレッド一覧
//! (subject.txt)を読んで、タイトルに「ReaTap」と「Part<番号>」があるスレッドのうち
//! 番号が一番大きいものを開く。一覧を読めない時・見つからない時は Part1 を開く。
//! 開くのは決まった板(下のBOARD)の中のスレッドだけ。
//! viewer-app-tauri/src-tauri/src/support_thread.rs と同じ内容。

use std::time::Duration;

const BOARD: &str = "https://bbs.jpnkn.com/ao33/";
const READ_CGI: &str = "https://bbs.jpnkn.com/test/read.cgi/ao33/";
/// 一覧を読めない時に開くスレッド(Part1)
const FALLBACK_KEY: &str = "1793780799";

#[tauri::command(async)]
pub async fn open_support_thread() -> Result<(), String> {
    let key = latest_key().await.unwrap_or_else(|e| {
        log::info!("要望・質問スレの一覧を読めなかったので、Part1を開きます: {e}");
        FALLBACK_KEY.to_string()
    });
    open_in_browser(&format!("{READ_CGI}{key}/"))
}

async fn latest_key() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(format!("{BOARD}subject.txt")).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    pick_latest(&bytes).ok_or_else(|| "スレッドが見つかりません".into())
}

/// subject.txt(Shift_JIS)から、要望・質問スレの一番新しいPartのスレッドキーを選ぶ。
/// 文字コードを変換しなくても済むように、タイトルの中の英字(「ReaTap」「Part」)と数字だけを見る
fn pick_latest(subject: &[u8]) -> Option<String> {
    let mut best: Option<(u32, &[u8])> = None;
    for line in subject.split(|&b| b == b'\n') {
        let Some(sep) = find(line, b".dat<>") else { continue };
        let key = &line[..sep];
        if key.is_empty() || !key.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let title = &line[sep + 6..];
        if find_ci(title, b"reatap").is_none() {
            continue;
        }
        let Some(part) = part_number(title) else { continue };
        // 番号が同じなら、後に立った(キーが大きい)方
        if best.is_none_or(|(p, k)| part > p || (part == p && key > k)) {
            best = Some((part, key));
        }
    }
    best.map(|(_, k)| String::from_utf8_lossy(k).into_owned())
}

/// 「Part12」の12(「Part」の後に続く数字。最後に見つかったもの)
fn part_number(title: &[u8]) -> Option<u32> {
    let mut found = None;
    let mut from = 0;
    while let Some(i) = find_ci(&title[from..], b"part") {
        let start = from + i + 4;
        let digits: Vec<u8> = title[start..].iter().copied().skip_while(|&b| b == b' ').take_while(u8::is_ascii_digit).collect();
        if let Ok(n) = std::str::from_utf8(&digits).unwrap_or_default().parse() {
            found = Some(n);
        }
        from = start;
    }
    found
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle))
}

fn open_in_browser(url: &str) -> Result<(), String> {
    // explorerにURLを渡すと「ドキュメント」フォルダが開いてしまうことがあるので、
    // URLを既定のブラウザで開く標準の方法(url.dllのFileProtocolHandler)を使う
    #[cfg(target_os = "windows")]
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map_err(|e| format!("ブラウザを開けませんでした: {e}"))?;
    #[cfg(not(target_os = "windows"))]
    let _ = url;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pick_latest;

    #[test]
    fn picks_the_newest_part() {
        let s = "1793780799.dat<>Reacast･ReaTap要望質問スレッド Part1 (1000)\n\
                 1793727555.dat<>554 (856)\n\
                 1793900000.dat<>Reacast･ReaTap要望質問スレッド Part2 (3)\n\
                 1793568455.dat<>Part9 (1001)\n";
        assert_eq!(pick_latest(s.as_bytes()).as_deref(), Some("1793900000"));
        assert_eq!(pick_latest("1.dat<>554 (1)\n".as_bytes()), None);
    }
}
