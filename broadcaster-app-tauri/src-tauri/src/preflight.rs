//! 配信前のチェック(コントロールパネルの「配信前チェック」)。掲示板・YouTube・
//! Twitch・読み上げソフト・OBSのブラウザソース・ホットキーが使える状態かを
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

async fn check_bbs(url: String) -> CheckItem {
    const L: &str = "掲示板";
    if url.trim().is_empty() {
        return item(L, "skip", "URLが入っていません(掲示板を使わない場合はこのままで大丈夫です)");
    }
    let (board, key) = match source::parse_url(&url) {
        Ok(v) => v,
        Err(e) => return item(L, "ng", e),
    };
    let list = match source::list_threads(&source::http_client(), &board).await {
        Ok(l) => l,
        Err(e) => return item(L, "ng", format!("板を読み込めませんでした: {e}")),
    };
    match key {
        None => item(L, "ok", format!("板を読み込めました(スレッド{}件)", list.len())),
        Some(k) => match list.iter().find(|t| t.key == k) {
            None => item(L, "warn", "指定したスレッドが板の一覧にありません(落ちたか、別の板のURLかもしれません)"),
            Some(t) if t.count >= 1000 => item(L, "warn", format!("「{}」は1000を超えています(次スレのURLにしてください)", t.title)),
            Some(t) => item(L, "ok", format!("「{}」(レス{})", t.title, t.count)),
        },
    }
}

async fn check_site(label: &str, target: String, live: Option<comments::SourceStatus>, twitch: bool) -> CheckItem {
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
    if twitch {
        let r = comments::comments_check_twitch(target).await;
        let ok = r["ok"].as_bool().unwrap_or(false);
        item(label, if ok { "ok" } else { "ng" }, r["message"].as_str().unwrap_or_default())
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

    let (bbs, youtube, twitch) = tokio::join!(
        check_bbs(s.thread_url.clone()),
        check_site("YouTube", s.youtube_target.clone(), live("youtube"), false),
        check_site("Twitch", s.twitch_channel.clone(), live("twitch"), true),
    );

    let mut out = vec![
        if status.running {
            item("コメントの取得", "ok", "取得中です")
        } else {
            item("コメントの取得", "warn", "まだ開始していません(コメント読み上げ・字幕の設定の「開始」)")
        },
        bbs,
        youtube,
        twitch,
    ];

    let speech = if !s.speech_enabled || s.command_path.trim().is_empty() {
        item("読み上げソフト", "skip", "読み上げはOFFです")
    } else if std::path::Path::new(s.command_path.trim()).exists() {
        item("読み上げソフト", "ok", s.command_path.trim())
    } else {
        item("読み上げソフト", "ng", format!("「{}」が見つかりません", s.command_path.trim()))
    };
    out.push(speech);

    out.push(if !s.voice_enabled {
        item("話した字幕", "skip", "使っていません")
    } else if crate::voice::is_running(&app) {
        let st = app.state::<crate::voice::VoiceManager>().inner().status_message();
        let state = match st.0.as_str() {
            "error" | "unsupported" => "ng",
            // マイクを止めたままになっている時は気付けるように
            "muted" => "warn",
            _ => "ok",
        };
        item("話した字幕", state, st.1)
    } else {
        item("話した字幕", "ng", "音声認識の窓(Edge)が閉じています(設定画面の「音声認識の窓を開き直す」)")
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
