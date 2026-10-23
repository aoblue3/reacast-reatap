//! 荒らし対策。届いたコメントを順番待ちに入れる前に見て、荒らしらしいものを読まない。
//!
//! 連投するのは普通の良いリスナーであることが多いので、既定はOFF、ONにしても
//! 緩めの数字にしてある(CommentSettingsのspam_*)。読まなかったレスも
//! 「スレッドのレス」の画面では見られる(掲示板のレスは取得した全部を持っているため)。
//!
//! 「同じ人」の見分け方はランダムの声と同じ(Post::person: 掲示板はID、配信サイトは名前)。

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// 荒らし対策の設定(CommentSettingsから作る)
#[derive(Clone, Debug)]
pub struct SpamRules {
    /// 同じ人が同じ文を same_text_secs 秒以内に same_text_count 回目に書いたら読まない(0で使わない)
    pub same_text_count: usize,
    pub same_text_secs: u64,
    /// 同じ人が flood_secs 秒以内に flood_count 件書いたら、その人を mute_secs 秒読まない(0で使わない)
    pub flood_count: usize,
    pub flood_secs: u64,
    pub mute_secs: u64,
    /// 違う人が同じ長文(copypaste_min_chars 文字以上)を60秒以内に貼ったら、
    /// copypaste_people 人目から読まない(0で使わない)
    pub copypaste_people: usize,
    pub copypaste_min_chars: usize,
}

const COPYPASTE_SECS: u64 = 60;
/// 覚えておく書き込みの数の上限(コメントがとても多い時もメモリを使いすぎないように)
const MAX_RECENT: usize = 2000;

struct Recent {
    at: Instant,
    person: String,
    text: String,
}

#[derive(Default)]
pub struct SpamFilter {
    recent: VecDeque<Recent>,
    /// 書き込みが多すぎて、しばらく読まない人 → 読まない期限
    muted: HashMap<String, Instant>,
}

/// 空白の違いだけの文は同じ文とみなす
fn normalize(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl SpamFilter {
    /// 読まないレスなら、その理由(ログ用)を返す。読むレスも含めて全部覚えておく。
    pub fn check(&mut self, now: Instant, person: &str, body: &str, rules: &SpamRules) -> Option<&'static str> {
        let longest = [rules.same_text_secs, rules.flood_secs, COPYPASTE_SECS].into_iter().max().unwrap_or(0);
        while let Some(r) = self.recent.front() {
            if now.duration_since(r.at) > Duration::from_secs(longest) || self.recent.len() > MAX_RECENT {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        self.muted.retain(|_, until| *until > now);

        let text = normalize(body);
        let within = |r: &Recent, secs: u64| now.duration_since(r.at) <= Duration::from_secs(secs);
        let verdict = if self.muted.contains_key(person) {
            Some("書き込みが多すぎるため、しばらく読みません")
        } else if rules.flood_count > 0
            && self.recent.iter().filter(|r| r.person == person && within(r, rules.flood_secs)).count() + 1 >= rules.flood_count
        {
            self.muted.insert(person.to_string(), now + Duration::from_secs(rules.mute_secs));
            Some("書き込みが多すぎるため、しばらく読みません")
        } else if rules.same_text_count > 0
            && !text.is_empty()
            && self
                .recent
                .iter()
                .filter(|r| r.person == person && r.text == text && within(r, rules.same_text_secs))
                .count()
                + 1
                >= rules.same_text_count
        {
            Some("同じ人の同じ文の連投")
        } else if rules.copypaste_people > 0 && text.chars().count() >= rules.copypaste_min_chars.max(1) && {
            let mut people: Vec<&str> = self
                .recent
                .iter()
                .filter(|r| r.text == text && r.person != person && within(r, COPYPASTE_SECS))
                .map(|r| r.person.as_str())
                .collect();
            people.sort_unstable();
            people.dedup();
            people.len() + 1 >= rules.copypaste_people
        } {
            Some("みんなで同じコピペ")
        } else {
            None
        };
        self.recent.push_back(Recent { at: now, person: person.to_string(), text });
        verdict
    }

    pub fn clear(&mut self) {
        self.recent.clear();
        self.muted.clear();
    }
}

/// 同じ文字が長く続く所を短くする(読み上げ用。「wwwwwwwwwwwwwwww」→「wwwww」)。
/// max_run文字より長く続いたら、keep文字にする(0で使わない)
pub fn compress_repeats(text: &str, max_run: usize, keep: usize) -> String {
    if max_run == 0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut run = 0usize;
    let mut run_text = String::new();
    let flush = |out: &mut String, run_text: &mut String, run: usize| {
        if run > max_run {
            out.extend(run_text.chars().take(keep));
        } else {
            out.push_str(run_text);
        }
        run_text.clear();
    };
    for c in text.chars() {
        if Some(c) == prev {
            run += 1;
        } else {
            flush(&mut out, &mut run_text, run);
            prev = Some(c);
            run = 1;
        }
        run_text.push(c);
    }
    flush(&mut out, &mut run_text, run);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> SpamRules {
        SpamRules {
            same_text_count: 3,
            same_text_secs: 60,
            flood_count: 10,
            flood_secs: 10,
            mute_secs: 60,
            copypaste_people: 3,
            copypaste_min_chars: 20,
        }
    }

    #[test]
    fn same_text_from_third_time() {
        let mut f = SpamFilter::default();
        let t = Instant::now();
        assert_eq!(f.check(t, "ID:a", "草", &rules()), None);
        assert_eq!(f.check(t + Duration::from_secs(5), "ID:a", "草 ", &rules()), None);
        assert!(f.check(t + Duration::from_secs(10), "ID:a", "草", &rules()).is_some());
        // 別の人なら読む
        assert_eq!(f.check(t + Duration::from_secs(11), "ID:b", "草", &rules()), None);
        // 時間が経てば読む
        let mut f = SpamFilter::default();
        f.check(t, "ID:a", "草", &rules());
        f.check(t + Duration::from_secs(1), "ID:a", "草", &rules());
        assert_eq!(f.check(t + Duration::from_secs(120), "ID:a", "草", &rules()), None);
    }

    #[test]
    fn flood_mutes_for_a_while() {
        let mut f = SpamFilter::default();
        let t = Instant::now();
        for i in 0..9 {
            assert_eq!(f.check(t + Duration::from_millis(i * 500), "ID:a", &format!("レス{i}"), &rules()), None, "{i}");
        }
        // 10秒以内に10件目
        assert!(f.check(t + Duration::from_millis(4600), "ID:a", "レス10", &rules()).is_some());
        // ミュート中はふつうのレスも読まない
        assert!(f.check(t + Duration::from_secs(30), "ID:a", "こんにちは", &rules()).is_some());
        // ほかの人は読む
        assert_eq!(f.check(t + Duration::from_secs(30), "ID:b", "こんにちは", &rules()), None);
        // 期限が過ぎたら読む
        assert_eq!(f.check(t + Duration::from_secs(70), "ID:a", "ただいま", &rules()), None);
    }

    #[test]
    fn copypaste_from_third_person() {
        let mut f = SpamFilter::default();
        let t = Instant::now();
        let long = "これはとても長いコピペの文章です。みんなで貼りましょう";
        assert_eq!(f.check(t, "ID:a", long, &rules()), None);
        assert_eq!(f.check(t + Duration::from_secs(1), "ID:b", long, &rules()), None);
        assert!(f.check(t + Duration::from_secs(2), "ID:c", long, &rules()).is_some());
        // 短い文(おめでとう等)はみんなが書いても読む
        let mut f = SpamFilter::default();
        for p in ["a", "b", "c", "d"] {
            assert_eq!(f.check(t, p, "おめでとう！", &rules()), None);
        }
    }

    #[test]
    fn off_when_zero() {
        let off = SpamRules { same_text_count: 0, flood_count: 0, copypaste_people: 0, ..rules() };
        let mut f = SpamFilter::default();
        let t = Instant::now();
        for i in 0..30 {
            assert_eq!(f.check(t + Duration::from_millis(i * 10), "ID:a", "同じ文同じ文同じ文同じ文同じ文同じ文同じ文", &off), None);
        }
    }

    #[test]
    fn compress_long_runs() {
        assert_eq!(compress_repeats("草wwwwwwwwwwwwwwwwwwww", 10, 5), "草wwwww");
        assert_eq!(compress_repeats("wwwwwwwwww", 10, 5), "wwwwwwwwww");
        assert_eq!(compress_repeats("あーーーーーーーーーーーーーーーーーーーーーーい", 10, 5), "あーーーーーい");
        assert_eq!(compress_repeats("8888888888888888", 10, 5), "88888");
        assert_eq!(compress_repeats("ふつうの文", 10, 5), "ふつうの文");
        assert_eq!(compress_repeats("wwwwwwwwwwwwwww", 0, 5), "wwwwwwwwwwwwwww");
    }
}
