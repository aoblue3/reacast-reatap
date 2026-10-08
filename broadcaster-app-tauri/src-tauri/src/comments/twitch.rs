//! Twitchのチャット取得(ログイン不要)。
//!
//! Twitchのチャットは、`justinfan<数字>`という匿名の名前でログインすると、
//! 読み取り専用で誰でも参加できる(Twitch自身が公開しているチャットの仕組み)。
//! WebSocket(wss://irc-ws.chat.twitch.tv)で繋ぎ、IRCの形式で届くメッセージを読む。

/// 入力(チャンネル名、またはhttps://www.twitch.tv/チャンネル名)からチャンネル名を取り出す。
pub fn parse_channel(input: &str) -> Result<String, String> {
    let s = input.trim().trim_end_matches('/');
    let name = if let Ok(url) = reqwest::Url::parse(s) {
        if !url.host_str().unwrap_or_default().ends_with("twitch.tv") {
            return Err("TwitchのURLではありません".into());
        }
        url.path_segments()
            .and_then(|mut p| p.find(|x| !x.is_empty()))
            .unwrap_or_default()
            .to_string()
    } else {
        s.trim_start_matches('#').to_string()
    };
    let name = name.to_ascii_lowercase();
    if name.is_empty() || name.len() > 25 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("Twitchのチャンネル名(またはチャンネルのURL)を入力してください".into());
    }
    Ok(name)
}

#[derive(Debug, PartialEq)]
pub struct ChatMessage {
    pub author: String,
    pub text: String,
}

/// IRCタグの値のエスケープを戻す(\s=空白 など)。
fn unescape_tag(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some(':') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 1行のIRCメッセージがチャットの発言(PRIVMSG)なら取り出す。
pub fn parse_privmsg(line: &str) -> Option<ChatMessage> {
    let (tags, rest) = match line.strip_prefix('@') {
        Some(l) => {
            let (t, r) = l.split_once(' ')?;
            (Some(t), r)
        }
        None => (None, line),
    };
    // rest: ":login!login@login.tmi.twitch.tv PRIVMSG #channel :本文"
    let rest = rest.strip_prefix(':')?;
    let (prefix, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix("PRIVMSG ")?;
    let (_channel, text) = rest.split_once(" :")?;
    let login = prefix.split('!').next().unwrap_or_default();
    let display = tags
        .and_then(|t| t.split(';').find_map(|kv| kv.strip_prefix("display-name=")))
        .map(unescape_tag)
        .filter(|n| !n.is_empty());
    // /me で発言した時は \x01ACTION 本文\x01 になる
    let text = text
        .strip_prefix("\u{1}ACTION ")
        .and_then(|t| t.strip_suffix('\u{1}'))
        .unwrap_or(text);
    Some(ChatMessage { author: display.unwrap_or_else(|| login.to_string()), text: text.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_channel_inputs() {
        assert_eq!(parse_channel("kato_junichi0817").unwrap(), "kato_junichi0817");
        assert_eq!(parse_channel("https://www.twitch.tv/StylishNoob4/").unwrap(), "stylishnoob4");
        assert_eq!(parse_channel("#shaka").unwrap(), "shaka");
        assert!(parse_channel("https://example.com/x").is_err());
        assert!(parse_channel("bad name!").is_err());
        assert!(parse_channel("").is_err());
    }

    #[test]
    fn parses_privmsg_lines() {
        let line = "@badge-info=;color=#1E90FF;display-name=ケミカル甘味;emotes=;id=x :chem!chem@chem.tmi.twitch.tv PRIVMSG #kato_junichi0817 :今日から風呂入らずに行こうかな";
        assert_eq!(
            parse_privmsg(line),
            Some(ChatMessage { author: "ケミカル甘味".into(), text: "今日から風呂入らずに行こうかな".into() })
        );
        // display-nameが空ならログイン名、本文中の " :" で切れない
        let line2 = "@display-name=;id=y :abc!abc@abc.tmi.twitch.tv PRIVMSG #ch :a : b";
        assert_eq!(parse_privmsg(line2).unwrap(), ChatMessage { author: "abc".into(), text: "a : b".into() });
        let me = "@display-name=Foo\\sBar :foo!foo@foo.tmi.twitch.tv PRIVMSG #ch :\u{1}ACTION waves\u{1}";
        assert_eq!(parse_privmsg(me).unwrap(), ChatMessage { author: "Foo Bar".into(), text: "waves".into() });
        assert_eq!(parse_privmsg("PING :tmi.twitch.tv"), None);
        assert_eq!(parse_privmsg(":tmi.twitch.tv 001 justinfan1 :Welcome"), None);
    }
}
