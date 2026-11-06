//! 配信前のチェック(コントロールパネルの「配信前チェック」)。掲示板・YouTube・
//! Twitch・ニコ生・読み上げソフト・OBSのブラウザソース・ホットキーが使える状態かを
//! まとめて確かめる。中継サーバーとの接続はコントロールパネル(JS)側で足す。
//! 時間のかかる確認(Twitchは8秒)は同時に行う。

use crate::comments::{self, source, CommentEngine};
use crate::obs_bridge::ObsBridge;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
pub struct CheckItem {
    label: String,
    /// "ok" | "warn" | "ng" | "skip"(使っていない)
    state: &'static str,
    message: String,
}

fn item(label: &str, state: &'static str, message: impl Into<String>) -> CheckItem {
    CheckItem { label: label.into(), state, message: message.into() }
}

async fn check_bbs(url: String, sub: bool) -> CheckItem {
    let l = if sub { "2つ目の掲示板" } else { "掲示板" };
    if url.trim().is_empty() {
        return item(l, "skip", "URLが入っていません(掲示板を使わない場合はこのままで大丈夫です)");
    }
    let (board, key) = match source::parse_url(&url) {
        Ok(v) => v,
        Err(e) => return item(l, "ng", e),
    };
    let list = match source::list_threads(&source::http_client(), &board).await {
        Ok(list) => list,
        Err(e) => return item(l, "ng", format!("板を読み込めませんでした: {e}")),
    };
    match key {
        None => item(l, "ok", format!("板を読み込めました(スレッド{}件)", list.len())),
        Some(k) => match list.iter().find(|t| t.key == k) {
            None => item(l, "warn", "指定したスレッドが板の一覧にありません(落ちたか、別の板のURLかもしれません)"),
            Some(t) if t.count >= 1000 => item(l, "warn", format!("「{}」は1000を超えています(次スレのURLにしてください)", t.title)),
            Some(t) => item(l, "ok", format!("「{}」(レス{})", t.title, t.count)),
        },
    }
}

async fn check_site(label: &str, target: String, live: Option<comments::SourceStatus>, site: &str) -> CheckItem {
    if target.trim().is_empty() {
        return item(label, "skip", "使っていません");
    }
    // 取得中なら、実際に繋いでいる状態をそのまま使う
    if let Some(s) = live {
        let state = match s.state.as_str() {
            "ok" => "ok",
            "error" => "ng",
            _ => "warn",
        };
        return item(label, state, s.message);
    }
    if site == "twitch" {
        let r = comments::comments_check_twitch(target).await;
        let ok = r["ok"].as_bool().unwrap_or(false);
        item(label, if ok { "ok" } else { "ng" }, r["message"].as_str().unwrap_or_default())
    } else if site == "niconico" {
        let (ok, message) = comments::niconico::check(&target).await;
        item(label, if ok { "ok" } else { "warn" }, message)
    } else {
        let r = comments::youtube::check(&target).await;
        item(label, if r.ok { "ok" } else { "warn" }, r.message)
    }
}

#[tauri::command]
pub async fn preflight_check(app: AppHandle) -> Vec<CheckItem> {
    let engine = app.state::<Arc<CommentEngine>>().inner().clone();
    let s = engine.settings();
    let status = engine.status();
    let live = |id: &str| status.sources.iter().find(|x| x.id == id).cloned();

    // 「使う」のチェックを外したものは、確かめずに「使わない設定」と出す
    let used = |on: bool, v: &String| if on { v.clone() } else { String::new() };
    let (mut bbs, sub_bbs, mut youtube, mut twitch, mut niconico) = tokio::join!(
        check_bbs(used(s.use_bbs, &s.thread_url), false),
        check_bbs(used(s.use_bbs, &s.sub_thread_url), true),
        check_site("YouTube", used(s.use_youtube, &s.youtube_target), live("youtube"), "youtube"),
        check_site("Twitch", used(s.use_twitch, &s.twitch_channel), live("twitch"), "twitch"),
        check_site("ニコ生", used(s.use_niconico, &s.niconico_target), live("niconico"), "niconico"),
    );
    const OFF: &str = "使わない設定です(「かんたん設定」のチェック)";
    if !s.use_bbs {
        bbs = item("掲示板", "skip", OFF);
    }
    if !s.use_youtube {
        youtube = item("YouTube", "skip", OFF);
    }
    if !s.use_twitch {
        twitch = item("Twitch", "skip", OFF);
    }
    if !s.use_niconico {
        niconico = item("ニコ生", "skip", OFF);
    }

    let mut out = vec![
        if status.running {
            item("コメントの取得", "ok", "取得中です")
        } else {
            item("コメントの取得", "warn", "まだ開始していません(コメント読み上げ・字幕の設定の「開始」)")
        },
        bbs,
    ];
    // 2つ目の掲示板は、使っている人にだけ出す
    if s.use_bbs && !s.sub_thread_url.trim().is_empty() {
        out.push(sub_bbs);
    }
    out.extend([youtube, twitch, niconico]);

    let speech = if !s.speech_enabled || s.command_path.trim().is_empty() {
        item("読み上げソフト", "skip", "読み上げはOFFです")
    } else if std::path::Path::new(s.command_path.trim()).exists() {
        item("読み上げソフト", "ok", s.command_path.trim())
    } else {
        item("読み上げソフト", "ng", format!("「{}」が見つかりません", s.command_path.trim()))
    };
    out.push(speech);

    out.push(if !s.voice_enabled {
        item("自分の声の字幕", "skip", "使っていません")
    } else if crate::voice::is_running(&app) {
        let st = app.state::<crate::voice::VoiceManager>().inner().status_message();
        let state = match st.0.as_str() {
            "error" | "unsupported" => "ng",
            // マイクを止めたままになっている時は気付けるように
            "muted" => "warn",
            _ => "ok",
        };
        item("自分の声の字幕", state, st.1)
    } else {
        item("自分の声の字幕", "ng", "音声認識の窓(Edge)が閉じています(設定画面の「音声認識の窓を開き直す」)")
    });

    let clients = app.state::<ObsBridge>().client_count();
    out.push(if clients > 0 {
        item("OBSのブラウザソース", "ok", format!("{clients}個つながっています"))
    } else {
        item("OBSのブラウザソース", "warn", "つながっていません(OBSを使わない場合はこのままで大丈夫です)")
    });

    let hotkeys = [
        s.hotkey_skip.trim(),
        s.hotkey_pause.trim(),
        s.hotkey_voice_mute.trim(),
        s.hotkey_voice_unmute.trim(),
    ]
        .iter()
        .filter(|k| !k.is_empty())
        .count();
    out.push(match (hotkeys, comments::hotkeys::apply(&app, &s)) {
        (0, _) => item("ホットキー", "skip", "割り当てていません"),
        (_, Ok(())) => item("ホットキー", "ok", format!("{hotkeys}個使えます")),
        (_, Err(e)) => item("ホットキー", "ng", e),
    });
    out
}
