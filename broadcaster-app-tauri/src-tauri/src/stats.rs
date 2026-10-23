//! リアクションの統計と盛り上がりマーカー(既定OFF。コントロールパネルでONにする)。
//!
//! 届いたリアクション・読み上げたコメントの数を1分ごとに数えて、短い時間にたくさん
//! 届いた瞬間を「盛り上がりマーカー」として記録する(切り抜き・見返し用)。
//! 記録は1回の配信ごとに1つのJSONファイル(数KB)。30分何も届かなかったら次の配信とみなし、
//! ファイルは新しいものから MAX_SESSIONS 個だけ残す。
//! 保存するのは時刻と数だけで、コメントの本文や視聴者を見分ける情報は残さない。

use crate::config_store::ConfigStore;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const ENABLED_KEY: &str = "statsEnabled";
/// これだけ何も届かなかったら、次に届いた時から別の配信として記録する
const SESSION_GAP: Duration = Duration::from_secs(30 * 60);
/// 盛り上がりを見る幅
const WINDOW: Duration = Duration::from_secs(10);
/// マーカーとマーカーの間は最低これだけ空ける(同じ盛り上がりで何個も付けない)
const MARKER_GAP: Duration = Duration::from_secs(60);
/// 10秒にこれ以上届いたら盛り上がり(少ない配信でも付くように低め。多い配信では平均の3倍)
const MIN_MARKER_SCORE: u32 = 8;
const MAX_SESSIONS: usize = 30;
const SAVE_EVERY: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Minute {
    /// リアクションのID → 数(視聴者から・コメント連動・話した言葉の合計)
    pub reactions: BTreeMap<String, u32>,
    pub comments: u32,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Marker {
    /// 盛り上がった時刻(UNIXミリ秒)と、記録の開始からの秒数
    pub at_ms: u64,
    pub offset_secs: u64,
    /// 10秒間に届いたリアクション・コメントの数(盛り上がりの大きさ)
    pub score: u32,
    /// 一番多かったリアクション(無ければ空)
    pub top: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Session {
    pub id: String,
    pub started_ms: u64,
    pub last_ms: u64,
    /// 視聴者から届いたリアクション・コメント連動・話した言葉で飛んだリアクションの数
    pub viewer_reactions: u32,
    pub linked_reactions: u32,
    pub comments: u32,
    pub minutes: Vec<Minute>,
    pub markers: Vec<Marker>,
}

/// 一覧用(中身の1分ごとの数は含めない)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub started_ms: u64,
    pub last_ms: u64,
    pub viewer_reactions: u32,
    pub linked_reactions: u32,
    pub comments: u32,
    pub markers: usize,
}

struct Live {
    session: Session,
    started: Instant,
    last: Instant,
    /// 直近10秒に届いたもの(届いた時刻、リアクションのID。コメントはNone)
    window: VecDeque<(Instant, Option<String>)>,
    last_marker: Option<Instant>,
    saved: Instant,
    dirty: bool,
}

#[derive(Default)]
pub struct Stats {
    live: Mutex<Option<Live>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("stats"))
}

fn enabled(app: &AppHandle) -> bool {
    app.state::<ConfigStore>().get(ENABLED_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 何か届いた時の記録(reaction: リアクションのID。コメントならNone)。
/// 盛り上がったかどうかもここで判定する。
fn record(live: &mut Live, now: Instant, wall_ms: u64, reaction: Option<&str>, linked: bool) {
    let s = &mut live.session;
    let minute = (now.duration_since(live.started).as_secs() / 60) as usize;
    if s.minutes.len() <= minute {
        s.minutes.resize(minute + 1, Minute::default());
    }
    match reaction {
        Some(id) => {
            *s.minutes[minute].reactions.entry(id.to_string()).or_insert(0) += 1;
            if linked {
                s.linked_reactions += 1;
            } else {
                s.viewer_reactions += 1;
            }
        }
        None => {
            s.minutes[minute].comments += 1;
            s.comments += 1;
        }
    }
    s.last_ms = wall_ms;
    live.last = now;
    live.dirty = true;

    live.window.push_back((now, reaction.map(str::to_string)));
    while live.window.front().is_some_and(|(t, _)| now.duration_since(*t) > WINDOW) {
        live.window.pop_front();
    }
    let score = live.window.len() as u32;
    // 今までの平均(10秒あたり)の3倍、少なくともMIN_MARKER_SCOREを超えたら盛り上がり
    let elapsed_windows = (now.duration_since(live.started).as_secs() / WINDOW.as_secs()).max(1) as u32;
    let total = s.viewer_reactions + s.linked_reactions + s.comments;
    let threshold = MIN_MARKER_SCORE.max(total / elapsed_windows * 3);
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for (_, id) in &live.window {
        if let Some(id) = id {
            *counts.entry(id.as_str()).or_insert(0) += 1;
        }
    }
    let top = counts.iter().max_by_key(|(_, n)| **n).map(|(id, _)| id.to_string()).unwrap_or_default();
    match live.last_marker {
        // 同じ盛り上がりが続いている間は、マーカーの大きさだけ更新する
        Some(t) if now.duration_since(t) < MARKER_GAP => {
            if let Some(m) = s.markers.last_mut() {
                if score > m.score {
                    m.score = score;
                    m.top = top;
                }
            }
        }
        _ if score >= threshold => {
            s.markers.push(Marker {
                at_ms: wall_ms,
                offset_secs: now.duration_since(live.started).as_secs(),
                score,
                top,
            });
            live.last_marker = Some(now);
        }
        _ => {}
    }
}

impl Stats {
    fn add(&self, app: &AppHandle, reaction: Option<&str>, linked: bool) {
        if !enabled(app) {
            return;
        }
        let now = Instant::now();
        let wall = now_ms();
        let mut guard = self.live.lock().unwrap();
        // 30分何も無かったら、前の配信の記録を閉じて新しく始める
        if guard.as_ref().is_some_and(|l| now.duration_since(l.last) > SESSION_GAP) {
            if let Some(old) = guard.take() {
                save(app, &old.session);
            }
        }
        let live = guard.get_or_insert_with(|| Live {
            session: Session { id: format!("{wall}"), started_ms: wall, last_ms: wall, ..Default::default() },
            started: now,
            last: now,
            window: VecDeque::new(),
            last_marker: None,
            saved: now,
            dirty: false,
        });
        record(live, now, wall, reaction, linked);
        if now.duration_since(live.saved) >= SAVE_EVERY {
            live.saved = now;
            live.dirty = false;
            save(app, &live.session);
        }
    }

    /// 視聴者から届いたリアクション(control.jsから。表示率・クールタイムで間引く前に数える)
    pub fn viewer_reaction(&self, app: &AppHandle, id: &str) {
        self.add(app, Some(id), false);
    }

    /// コメント連動・話した言葉で飛んだリアクション
    pub fn linked_reaction(&self, app: &AppHandle, id: &str) {
        self.add(app, Some(id), true);
    }

    /// 字幕に出したコメント(NGで飛ばしたもの・お知らせは数えない)
    pub fn comment(&self, app: &AppHandle) {
        self.add(app, None, false);
    }

    /// まだ保存していない分を保存する(定期的に・アプリを閉じる時に呼ぶ)
    pub fn flush(&self, app: &AppHandle) {
        let mut guard = self.live.lock().unwrap();
        if let Some(live) = guard.as_mut() {
            if live.dirty {
                live.dirty = false;
                live.saved = Instant::now();
                save(app, &live.session);
            }
        }
    }

    fn current(&self) -> Option<Session> {
        self.live.lock().unwrap().as_ref().map(|l| l.session.clone())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.chars().all(|c| c.is_ascii_digit())
}

fn save(app: &AppHandle, session: &Session) {
    let Some(dir) = dir(app) else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("統計の保存先を作れませんでした: {e}");
        return;
    }
    match serde_json::to_string(session) {
        Ok(text) => {
            if let Err(e) = std::fs::write(dir.join(format!("{}.json", session.id)), text) {
                log::warn!("統計を保存できませんでした: {e}");
            }
        }
        Err(e) => log::warn!("統計を保存できませんでした: {e}"),
    }
    // 古い記録を消す(新しいものからMAX_SESSIONS個だけ残す)
    let mut ids = list_ids(&dir);
    ids.sort_unstable_by(|a, b| b.cmp(a));
    for old in ids.iter().skip(MAX_SESSIONS) {
        let _ = std::fs::remove_file(dir.join(format!("{old}.json")));
    }
}

fn list_ids(dir: &std::path::Path) -> Vec<u64> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse::<u64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load(app: &AppHandle, id: &str) -> Option<Session> {
    if !valid_id(id) {
        return None;
    }
    let path = dir(app)?.join(format!("{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// 30秒おきに、まだ保存していない分を保存する
pub fn start_flusher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(SAVE_EVERY).await;
            app.state::<Stats>().flush(&app);
        }
    });
}

// ---- コマンド ----

#[tauri::command]
pub fn stats_viewer_reaction(app: AppHandle, emoji: String) {
    if emoji.len() <= 32 {
        app.state::<Stats>().viewer_reaction(&app, &emoji);
    }
}

/// 記録の一覧(新しい順)。記録中のものは今の中身で返す
#[tauri::command(async)]
pub fn stats_list(app: AppHandle) -> Vec<SessionSummary> {
    let stats = app.state::<Stats>();
    stats.flush(&app);
    let Some(dir) = dir(&app) else { return Vec::new() };
    let mut ids = list_ids(&dir);
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.into_iter()
        .filter_map(|id| load(&app, &id.to_string()))
        .map(|s| SessionSummary {
            id: s.id,
            started_ms: s.started_ms,
            last_ms: s.last_ms,
            viewer_reactions: s.viewer_reactions,
            linked_reactions: s.linked_reactions,
            comments: s.comments,
            markers: s.markers.len(),
        })
        .collect()
}

#[tauri::command(async)]
pub fn stats_get(app: AppHandle, id: String) -> Option<Session> {
    let stats = app.state::<Stats>();
    if let Some(cur) = stats.current().filter(|s| s.id == id) {
        return Some(cur);
    }
    load(&app, &id)
}

#[tauri::command(async)]
pub fn stats_delete(app: AppHandle, id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("記録が見つかりません".into());
    }
    {
        // 記録中のものを消したら、次に届いた時から新しく記録する
        let stats = app.state::<Stats>();
        let mut guard = stats.live.lock().unwrap();
        if guard.as_ref().is_some_and(|l| l.session.id == id) {
            *guard = None;
        }
    }
    let path = dir(&app).ok_or("記録が見つかりません")?.join(format!("{id}.json"));
    std::fs::remove_file(path).map_err(|e| format!("消せませんでした: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(at: Instant) -> Live {
        Live {
            session: Session { id: "1".into(), ..Default::default() },
            started: at,
            last: at,
            window: VecDeque::new(),
            last_marker: None,
            saved: at,
            dirty: false,
        }
    }

    #[test]
    fn counts_per_minute() {
        let t = Instant::now();
        let mut l = live(t);
        record(&mut l, t, 0, Some("kusa"), false);
        record(&mut l, t + Duration::from_secs(30), 0, None, false);
        record(&mut l, t + Duration::from_secs(61), 0, Some("kusa"), true);
        record(&mut l, t + Duration::from_secs(62), 0, Some("clap"), false);
        let s = &l.session;
        assert_eq!(s.minutes.len(), 2);
        assert_eq!(s.minutes[0].reactions["kusa"], 1);
        assert_eq!(s.minutes[0].comments, 1);
        assert_eq!(s.minutes[1].reactions["kusa"], 1);
        assert_eq!(s.minutes[1].reactions["clap"], 1);
        assert_eq!((s.viewer_reactions, s.linked_reactions, s.comments), (2, 1, 1));
        assert!(s.markers.is_empty());
    }

    #[test]
    fn marker_on_burst_once() {
        let t = Instant::now();
        let mut l = live(t);
        // 最初はぽつぽつ
        for i in 0..5 {
            record(&mut l, t + Duration::from_secs(i * 20), 0, Some("heart"), false);
        }
        assert!(l.session.markers.is_empty());
        // 3分後に草が10秒で12個
        let burst = t + Duration::from_secs(180);
        for i in 0..12 {
            record(&mut l, burst + Duration::from_millis(i * 500), 1000, Some("kusa"), false);
        }
        assert_eq!(l.session.markers.len(), 1);
        let m = &l.session.markers[0];
        assert_eq!(m.top, "kusa");
        assert_eq!(m.offset_secs, 183);
        assert!(m.score >= 8);
        // 続いている間は増えない
        for i in 0..20 {
            record(&mut l, burst + Duration::from_secs(10) + Duration::from_millis(i * 300), 1000, Some("clap"), false);
        }
        assert_eq!(l.session.markers.len(), 1);
        // 間が空いてからの次の盛り上がりは別のマーカー
        let later = burst + Duration::from_secs(300);
        for i in 0..15 {
            record(&mut l, later + Duration::from_millis(i * 300), 2000, Some("cake"), false);
        }
        assert_eq!(l.session.markers.len(), 2);
        assert_eq!(l.session.markers[1].top, "cake");
    }

    #[test]
    fn ids_are_numbers_only() {
        assert!(valid_id("1729000000000"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
    }
}
