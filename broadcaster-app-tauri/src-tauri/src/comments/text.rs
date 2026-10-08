//! レス本文の加工(HTML→テキスト、AA判定、NGワード、読み方ルール、
//! 読み上げコマンドの引数組み立て)。どれも副作用の無い関数なので、
//! 単体テストで挙動を固定しておく。

use regex::{NoExpand, Regex};

/// datの本文(HTML断片)を表示・読み上げ用のプレーンテキストにする。
/// `<br>`は改行に、それ以外のタグ(アンカーの`<a>`等)は取り除き、
/// 文字参照(&gt;等)を元に戻す。2ch形式のdatは` <br> `のように前後に
/// 空白が入るので、各行の前後の空白も落とす。
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('>') {
            Some(end) => {
                let tag = after[1..end].trim().to_ascii_lowercase();
                if tag == "br" || tag.starts_with("br ") || tag == "br/" {
                    out.push('\n');
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let decoded = decode_entities(&out);
    decoded
        .lines()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_string()
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let semi = after.find(';').filter(|&i| i <= 10);
        let decoded = semi.and_then(|i| {
            let name = &after[1..i];
            let ch = match name {
                "gt" => Some('>'),
                "lt" => Some('<'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ if name.starts_with("#x") || name.starts_with("#X") => {
                    u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32)
                }
                _ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            ch.map(|c| (c, i))
        });
        match decoded {
            Some((c, i)) => {
                out.push(c);
                rest = &after[i + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 複数行のテキスト設定(NGワード・AA判定文字列など)を、空行を除いた一覧にする。
pub fn non_empty_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// AAモードにするかどうか。`threshold_chars`文字以上(0なら文字数では判定しない)、
/// または`patterns`のどれかを含む場合にAAとみなす(SpeechCastMeの
/// 「○文字以上または、下記文字列を含む場合」と同じ)。
pub fn is_aa(body: &str, threshold_chars: usize, patterns: &[String]) -> bool {
    if threshold_chars > 0 && body.chars().count() >= threshold_chars {
        return true;
    }
    patterns.iter().any(|p| !p.is_empty() && body.contains(p.as_str()))
}

/// NGワード(部分一致)を含むか。名前欄・本文のどちらかに含まれていればNG。
pub fn contains_ng(name: &str, body: &str, ng_words: &[String]) -> bool {
    ng_words
        .iter()
        .any(|w| !w.is_empty() && (body.contains(w.as_str()) || name.contains(w.as_str())))
}

/// 「」の中だけを取り出してつなげる(AAの台詞部分だけ読み上げる用)。
pub fn quoted_only(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '「' => {
                if depth > 0 {
                    out.push(c);
                }
                depth += 1;
            }
            '」' if depth > 0 => {
                depth -= 1;
                if depth > 0 {
                    out.push(c);
                } else {
                    out.push('、');
                }
            }
            _ if depth > 0 => out.push(c),
            _ => {}
        }
    }
    out.trim_end_matches('、').to_string()
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

/// SpeechCastMeの「読み方」と同じ形式(1行に`単語/読み`、単語部分は正規表現)。
/// 区切りは行の最後の`/`(正規表現側に`/`を含められるようにするため)。
/// 正規表現として不正な行は読み飛ばす。
pub struct ReadingRule {
    re: Regex,
    to: String,
}

pub fn parse_reading_rules(text: &str) -> Vec<ReadingRule> {
    non_empty_lines(text)
        .into_iter()
        .filter_map(|line| {
            let idx = line.rfind('/')?;
            let (pat, to) = (&line[..idx], &line[idx + 1..]);
            if pat.is_empty() {
                return None;
            }
            // SpeechCastMe(.NET)の書式では`¥`(円記号)をバックスラッシュとして
            // 書いている設定が多いので、ここで読み替える。
            let pat = pat.replace('¥', "\\");
            Regex::new(&pat).ok().map(|re| ReadingRule { re, to: to.to_string() })
        })
        .collect()
}

pub fn apply_reading_rules(text: &str, rules: &[ReadingRule]) -> String {
    let mut s = text.to_string();
    for r in rules {
        s = r.re.replace_all(&s, NoExpand(&r.to)).into_owned();
    }
    s
}

/// 読み上げコマンドの引数欄(例: `#Res#`、`/T:2 "#Res#"`)を引数の配列に分ける。
/// 空白区切りで、`"`で囲んだ部分は1つの引数として扱う。分けてから
/// トークンを置き換えるので、レス本文に空白や`"`が含まれていても
/// 引数がずれたり、別のオプションとして解釈されたりしない
/// (シェルを経由せず直接プロセスを起動するので、コマンドとして
/// 実行されることもない)。
pub fn split_args(template: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has = false;
    for c in template.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        args.push(cur);
    }
    args
}

pub struct TokenValues<'a> {
    pub res: &'a str,
    pub no: u32,
    pub name: &'a str,
    pub time: &'a str,
}

/// `#Res#`(読み上げ用に加工した本文)・`#No#`・`#Name#`・`#Time#`を置き換える。
/// 1回の走査で置き換えるので、レス本文に`#No#`等の文字列が含まれていても
/// それがさらに置き換えられることはない。
pub fn fill_tokens(arg: &str, v: &TokenValues) -> String {
    static TOKEN_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TOKEN_RE.get_or_init(|| Regex::new("#(Res|No|Name|Time)#").unwrap());
    re.replace_all(arg, |caps: &regex::Captures| match &caps[1] {
        "Res" => v.res.to_string(),
        "No" => v.no.to_string(),
        "Name" => v.name.to_string(),
        _ => v.time.to_string(),
    })
    .into_owned()
}

/// タイトルの最後に出てくる数字を1増やす(「539」→「540」、「雑談 part9」→
/// 「雑談 part10」、「009」→「010」は桁数を保つ)。全角数字にも対応する。
/// 数字が無ければNone。
pub fn increment_last_number(title: &str) -> Option<String> {
    let chars: Vec<char> = title.chars().collect();
    let is_digit = |c: char| c.is_ascii_digit() || ('０'..='９').contains(&c);
    let end = chars.iter().rposition(|&c| is_digit(c))? + 1;
    let mut start = end;
    while start > 0 && is_digit(chars[start - 1]) {
        start -= 1;
    }
    let fullwidth = chars[start] >= '０';
    let digits: String = chars[start..end]
        .iter()
        .map(|&c| if c >= '０' { char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap() } else { c })
        .collect();
    let n: u128 = digits.parse().ok()?;
    let next = format!("{:0width$}", n + 1, width = digits.len());
    let next: String = if fullwidth {
        next.chars().map(|c| char::from_u32(c as u32 - '0' as u32 + '０' as u32).unwrap()).collect()
    } else {
        next
    };
    let mut out: String = chars[..start].iter().collect();
    out.push_str(&next);
    out.extend(&chars[end..]);
    Some(out)
}

/// 本文の最初のアンカー(>>12、＞＞１２、≫12。>>12-15なら12)のレス番号。
pub fn first_anchor(body: &str) -> Option<u32> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:>>|＞＞|≫)\s*([0-9０-９]{1,4})").unwrap());
    let digits: String = re
        .captures(body)?
        .get(1)?
        .as_str()
        .chars()
        .map(|c| if c >= '０' { char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c) } else { c })
        .collect();
    digits.parse().ok().filter(|&n| n > 0)
}

/// タイトルの最後に出てくる数字(全角も含む)だけを取り出す。
pub fn last_number(title: &str) -> Option<String> {
    let is_digit = |c: char| c.is_ascii_digit() || ('０'..='９').contains(&c);
    let chars: Vec<char> = title.chars().collect();
    let end = chars.iter().rposition(|&c| is_digit(c))? + 1;
    let mut start = end;
    while start > 0 && is_digit(chars[start - 1]) {
        start -= 1;
    }
    Some(chars[start..end].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_anchor() {
        assert_eq!(first_anchor(">>12 それな"), Some(12));
        assert_eq!(first_anchor("＞＞１２３ 全角"), Some(123));
        assert_eq!(first_anchor("≫5"), Some(5));
        assert_eq!(first_anchor("前半 >>12-15 範囲"), Some(12));
        assert_eq!(first_anchor("アンカーなし 12"), None);
        assert_eq!(first_anchor(">>0"), None);
    }

    #[test]
    fn extracts_last_number() {
        assert_eq!(last_number("雑談 part10").as_deref(), Some("10"));
        assert_eq!(last_number("配信スレ★１３").as_deref(), Some("１３"));
        assert_eq!(last_number("なし"), None);
    }

    #[test]
    fn increments_last_number_in_title() {
        assert_eq!(increment_last_number("539").as_deref(), Some("540"));
        assert_eq!(increment_last_number("雑談 part9").as_deref(), Some("雑談 part10"));
        assert_eq!(increment_last_number("2026年 配信スレ 009").as_deref(), Some("2026年 配信スレ 010"));
        assert_eq!(increment_last_number("配信スレ★１２").as_deref(), Some("配信スレ★１３"));
        assert_eq!(increment_last_number("Part3 (避難所)").as_deref(), Some("Part4 (避難所)"));
        assert_eq!(increment_last_number("数字なし"), None);
    }

    #[test]
    fn html_to_text_handles_br_tags_and_entities() {
        let html = " ずんだ <br> お風呂 &gt;&gt;12 <a href=\"../test/read.cgi/x/1/12\" target=\"_blank\">&gt;&gt;12</a> &amp; &#65;&#x42; ";
        assert_eq!(html_to_text(html), "ずんだ\nお風呂 >>12 >>12 & AB");
    }

    #[test]
    fn html_to_text_keeps_unknown_ampersands() {
        assert_eq!(html_to_text("A&B &unknown; C"), "A&B &unknown; C");
    }

    #[test]
    fn aa_detection_by_length_and_pattern() {
        let pats = vec!["Д".to_string(), "( 人 )".to_string()];
        assert!(is_aa("(´Д｀)", 300, &pats));
        assert!(!is_aa("ふつうのレス", 300, &pats));
        assert!(is_aa(&"あ".repeat(300), 300, &pats));
        assert!(!is_aa(&"あ".repeat(299), 300, &pats));
        assert!(!is_aa(&"あ".repeat(500), 0, &[]));
    }

    #[test]
    fn ng_words_match_name_or_body() {
        let ng = vec!["荒らし".to_string()];
        assert!(contains_ng("名無し", "荒らしです", &ng));
        assert!(contains_ng("荒らし", "こんにちは", &ng));
        assert!(!contains_ng("名無し", "こんにちは", &ng));
        assert!(!contains_ng("名無し", "こんにちは", &["".to_string()]));
    }

    #[test]
    fn quoted_only_extracts_brackets() {
        assert_eq!(quoted_only("（´・ω・）「おはよう」\n　 「元気？」"), "おはよう、元気？");
        assert_eq!(quoted_only("台詞なし"), "");
    }

    #[test]
    fn reading_rules_use_last_slash_and_yen_backslash() {
        let rules = parse_reading_rules(
            "(https|ttps)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/リンク\nww+/ワラワラ\n今日/キョウ\n(壊れた/x\n",
        );
        assert_eq!(rules.len(), 3);
        assert_eq!(
            apply_reading_rules("今日 https://example.com/a?b=1 見てwww", &rules),
            "キョウ リンク 見てワラワラ"
        );
    }

    #[test]
    fn reading_rule_replacement_is_literal() {
        let rules = parse_reading_rules("a/$1x");
        assert_eq!(apply_reading_rules("abc", &rules), "$1xbc");
    }

    #[test]
    fn split_args_respects_quotes() {
        assert_eq!(split_args("#Res#"), vec!["#Res#"]);
        assert_eq!(split_args("/T:2  \"#No# #Res#\" x"), vec!["/T:2", "#No# #Res#", "x"]);
        assert_eq!(split_args("\"\""), vec![""]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn fill_tokens_does_not_reinterpret_body() {
        let v = TokenValues { res: "a \" -b #No#", no: 7, name: "名無し", time: "16:00" };
        // 本文中の空白や"で引数が増えたり、本文中の#No#が置き換えられたりしない
        let args: Vec<String> = split_args("#Res#").iter().map(|a| fill_tokens(a, &v)).collect();
        assert_eq!(args, vec!["a \" -b #No#"]);
        assert_eq!(fill_tokens("#No#:#Name#(#Time#)", &v), "7:名無し(16:00)");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate_chars("あいうえお", 3), "あいう");
        assert_eq!(truncate_chars("あいう", 0), "あいう");
    }
}
