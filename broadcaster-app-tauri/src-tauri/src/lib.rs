mod config_store;
mod connect_code;
mod obs_bridge;
mod updater;

use config_store::ConfigStore;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

/// オーバーレイの「最前面取り直し」を、リアクションが実際に飛んできた直後だけ
/// 高頻度(150ms間隔)に切り替えるための共有状態。
///
/// 背景: フルスクリーンで動かしつつ「常に最前面」に設定してあるゲーム(多くの
/// 配信ソフト・チャットオーバーレイツールが対象とする一般的な設定)は、ゲーム
/// 自身も定期的に自分のウィンドウを最前面へ再主張してくることがある。以前は
/// 2秒おきの固定間隔でしか取り直していなかったため、ちょうどリアクションの
/// アニメーション再生中(2〜5秒程度)にゲーム側の再主張が割り込むと、その間
/// オーバーレイが裏に回ってアニメーションが見えなくなる、という報告があった。
/// リアクションが飛んできた瞬間から数秒間だけ取り直し間隔を大幅に短くすることで、
/// アニメーション再生中に裏へ回ってしまう確率を下げる(常時150ms間隔にしないのは、
/// SetWindowPos相当の呼び出しを無駄に高頻度で回し続けるのを避けるため)。
///
/// 注意: これでも「DirectX排他フルスクリーン」(borderless windowedではなく、
/// ゲームがディスプレイを直接専有する昔ながらのフルスクリーンモード)で動いている
/// ゲームの上には、原理上どんなツールを使っても重ねて表示できない(OS標準の
/// スクリーンショットツールやDiscordのオーバーレイ等、他のツールも同じ制約を
/// 受ける)。その場合はゲーム側の設定を「ボーダレスウィンドウ」等に変更する
/// 必要がある(使い方ガイド参照)。
struct TopmostReasserter {
    burst_until: Mutex<Instant>,
}

impl TopmostReasserter {
    fn new() -> Self {
        // Instant::now()より前の値を初期値にしたいだけなので、生成直後の
        // "now"をそのまま入れておけば(=burst期間なし)十分。
        Self {
            burst_until: Mutex::new(Instant::now()),
        }
    }

    /// リアクション送出時に呼ぶ。向こう`dur`の間、高頻度取り直しモードに入る
    /// (既に高頻度モード中に新しいリアクションが来た場合は、その分だけ延長される)。
    fn bump(&self, dur: Duration) {
        let mut until = self.burst_until.lock().unwrap();
        let new_until = Instant::now() + dur;
        if new_until > *until {
            *until = new_until;
        }
    }

    fn is_bursting(&self) -> bool {
        Instant::now() < *self.burst_until.lock().unwrap()
    }
}

// 注意: option_env!/env! はコンパイル時(ビルド時)に環境変数を埋め込むマクロ。
// 以前はstd::env::var(実行時に読む)を使っていたため、「ビルド時にset REACTION_RELAY_HOST=...
// してからnpm run buildする」という手順書の説明と実際の挙動が食い違っていた
// (エクスプローラーからexeをダブルクリックした場合、ビルド時のコマンドプロンプトの環境変数は
// 一切引き継がれないため、常にダミー値relay.example.invalidにフォールバックしてしまい、
// 配信者アプリがずっと「未接続」になる原因になっていた)。ビルド時の値をexeに焼き込むことで、
// 配布後は誰の環境で実行しても正しいアドレスに繋がるようにする。
fn default_relay_host() -> String {
    option_env!("REACTION_RELAY_HOST")
        .unwrap_or("relay.example.invalid")
        .to_string()
}

fn default_relay_port() -> u16 {
    option_env!("REACTION_RELAY_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(39200)
}

/// 設定で選んだモニターの識別子(Monitor::name()の値。取得できない環境向けの
/// フォールバックとして"monitor-{index}"形式も許容する)から、実際の位置・
/// サイズを解決する。該当するモニターが見つからない場合(未設定/切断済みなど)は
/// 主モニターにフォールバックし、それも取れない場合は既定サイズを使う。
///
/// リアクションが「メインモニターだけに出てきてほしい」という要望に応えるため、
/// 既定(未設定)では常に主モニターを使う。ユーザーが明示的に別のモニターを
/// 選んだ場合のみそちらを使う。
fn resolve_overlay_monitor(
    app: &tauri::AppHandle,
    monitor_id: Option<&str>,
) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    if let Some(id) = monitor_id {
        if let Ok(monitors) = app.available_monitors() {
            for (i, m) in monitors.iter().enumerate() {
                let matches = m.name().map(|n| n.as_str()) == Some(id)
                    || format!("monitor-{i}") == id;
                if matches {
                    return (*m.position(), *m.size());
                }
            }
        }
    }
    match app.primary_monitor() {
        Ok(Some(m)) => (*m.position(), *m.size()),
        _ => (PhysicalPosition::new(0, 0), PhysicalSize::new(1920, 1080)),
    }
}

#[derive(serde::Serialize)]
struct MonitorInfo {
    id: String,
    label: String,
    #[serde(rename = "isPrimary")]
    is_primary: bool,
}

/// 設定パネルの「表示モニター」欄用。今つながっている全モニターの一覧を返す。
#[tauri::command]
fn list_monitors(app: tauri::AppHandle) -> Vec<MonitorInfo> {
    let primary_pos = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| *m.position());
    let monitors = app.available_monitors().unwrap_or_default();
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let id = m.name().cloned().unwrap_or_else(|| format!("monitor-{i}"));
            let size = m.size();
            let pos = m.position();
            let is_primary = primary_pos.map(|p| p == *pos).unwrap_or(i == 0);
            MonitorInfo {
                label: format!(
                    "モニター{}: {}x{}{}",
                    i + 1,
                    size.width,
                    size.height,
                    if is_primary { "(メイン)" } else { "" }
                ),
                id,
                is_primary,
            }
        })
        .collect()
}

/// オーバーレイを表示するモニターを切り替える。選択をConfigStoreに保存し、
/// 既に開いているオーバーレイウィンドウの位置・サイズもその場で更新する。
/// monitor_idがNone(または一覧に無い値)の場合は「メインモニターに従う」既定動作。
#[tauri::command]
fn set_overlay_monitor(
    app: tauri::AppHandle,
    store: tauri::State<ConfigStore>,
    monitor_id: Option<String>,
) -> Result<(), String> {
    match &monitor_id {
        Some(id) => {
            store
                .set("overlayMonitorId".to_string(), serde_json::json!(id))
                .map_err(|e| e.to_string())?;
        }
        None => {
            store
                .set("overlayMonitorId".to_string(), serde_json::Value::Null)
                .map_err(|e| e.to_string())?;
        }
    }
    if let Some(win) = app.get_webview_window("overlay") {
        let (pos, size) = resolve_overlay_monitor(&app, monitor_id.as_deref());
        win.set_position(tauri::Position::Physical(pos))
            .map_err(|e| e.to_string())?;
        win.set_size(tauri::Size::Physical(size))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn create_overlay_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.get_webview_window("overlay").is_some() {
        return Ok(());
    }

    // 設定で選んだモニターに合わせる(未設定なら主モニターいっぱいに広げる。
    // 取得できない場合は既定サイズにフォールバック)。
    let saved_monitor_id = app
        .state::<ConfigStore>()
        .get("overlayMonitorId")
        .and_then(|v| v.as_str().map(String::from));
    let (pos, size) = resolve_overlay_monitor(app, saved_monitor_id.as_deref());

    let win = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("ReaCast overlay")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(true)
        // クリックを透過させるだけのオーバーレイなのでOS入力フォーカスは不要。
        // 初期フォーカスを持つと、配信中のゲーム画面側がフォーカスを失ってしまう
        // ことがあるため、フォーカスを与えないようにする
        .focused(false)
        .build()?;

    win.set_position(tauri::Position::Physical(pos))?;
    win.set_size(tauri::Size::Physical(size))?;
    // ゲーム画面の操作を邪魔しないよう、オーバーレイ自体はクリックを透過させる
    // (仕様書 v0.4 未確定事項: フルスクリーン排他モードのゲームとの相性はWindows実機での検証が必要)
    let _ = win.set_ignore_cursor_events(true);

    // 「リアクションが他のウィンドウの裏に回って見えなくなる」という報告への対応。
    // .always_on_top(true)はウィンドウ作成時に一度「最前面グループ」に入れるだけで、
    // その後に他のアプリ(ゲーム、配信ソフトの別ウィンドウ、他の常時最前面系ツールなど)
    // が自分自身を最前面化すると、Windowsの仕様上そちらが最前面グループの中で
    // 後勝ちしてしまい、こちらのオーバーレイがその裏に回ってしまうことがある
    // (視聴者アプリのリアクションバー側では、追従処理のたびに最前面を取り直す
    // 保険が既に入っていたが、配信者アプリのオーバーレイ側には無かったため、
    // 同じ考え方で定期的に最前面を取り直す処理を追加する)。
    //
    // 通常時は2秒おき、リアクションが実際に飛んできてから数秒間はTopmostReasserter
    // (上部の定義・emit_overlay_reaction参照)により150ms間隔に切り替わる。
    // アニメーション再生中に「常に最前面」設定のゲーム側が自分を再主張してくる
    // 競争に負けにくくするための対策。
    let reassert_handle = app.clone();
    std::thread::spawn(move || loop {
        let bursting = reassert_handle
            .try_state::<TopmostReasserter>()
            .map(|s| s.is_bursting())
            .unwrap_or(false);
        std::thread::sleep(if bursting {
            Duration::from_millis(150)
        } else {
            Duration::from_secs(2)
        });
        match reassert_handle.get_webview_window("overlay") {
            Some(win) => {
                let _ = win.set_always_on_top(true);
            }
            // オーバーレイウィンドウが無くなっている(アプリ終了中など)場合は
            // このスレッドも静かに終了する。
            None => break,
        }
    });

    Ok(())
}

fn create_control_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.get_webview_window("control").is_some() {
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(app, "control", WebviewUrl::App("control.html".into()))
        .title("ReaCast - コントロールパネル")
        // リアクションが増えるにつれてテスト送信ボタンの行数も増えていくため、
        // 画面に収まりやすい高さ(700px)にしつつ、それでも収まらない場合は
        // bodyのoverflow-y:auto(control.html側)でスクロールできるようにしてある。
        .inner_size(560.0, 700.0)
        .build()?;

    // コントロールパネルを×で閉じても、裏で常時起動している透明な全画面の
    // オーバーレイウィンドウが残ったままだと、Tauriは「全ウィンドウが閉じたら終了」
    // なのでアプリ全体が終了せず、タスクマネージャーで強制終了しないと落とせない
    // 状態になっていた。コントロールパネルを閉じたらアプリ全体を終了するようにする。
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            app_handle.exit(0);
        }
    });

    Ok(())
}

#[derive(serde::Serialize)]
struct Credentials {
    #[serde(rename = "roomId")]
    room_id: String,
    #[serde(rename = "broadcasterToken")]
    broadcaster_token: String,
}

#[derive(serde::Serialize)]
struct RelayAddress {
    host: String,
    port: u16,
}

#[derive(serde::Serialize)]
struct OverlaySettings {
    #[serde(rename = "glyphScale")]
    glyph_scale: f64,
    #[serde(rename = "glyphOpacity")]
    glyph_opacity: f64,
    #[serde(rename = "comboGrowthEnabled")]
    combo_growth_enabled: bool,
}

/// ConfigStoreから「絵文字の大きさ」を読む。未設定(初回起動時など)は
/// 既定の1.0(=100%)を返す。壊れた値(0以下やNaNなど)が入っていた場合も
/// 安全側の既定値にフォールバックする。
fn read_glyph_scale(store: &ConfigStore) -> f64 {
    store
        .get("glyphScale")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
}

/// ConfigStoreから「スタンプの透明度」を読む。未設定時は既定の1.0(=不透明)。
fn read_glyph_opacity(store: &ConfigStore) -> f64 {
    store
        .get("glyphOpacity")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
}

/// ConfigStoreから「連打で大きくなる」設定を読む。未設定時は既定でON
/// (従来通りの挙動)。
fn read_combo_growth_enabled(store: &ConfigStore) -> bool {
    store
        .get("comboGrowthEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

#[tauri::command]
fn get_credentials(store: tauri::State<ConfigStore>) -> Credentials {
    Credentials {
        room_id: store
            .get("roomId")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
        broadcaster_token: store
            .get("broadcasterToken")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn get_relay_address() -> RelayAddress {
    RelayAddress {
        host: default_relay_host(),
        port: default_relay_port(),
    }
}

#[tauri::command]
fn get_overlay_settings(store: tauri::State<ConfigStore>) -> OverlaySettings {
    OverlaySettings {
        glyph_scale: read_glyph_scale(&store),
        glyph_opacity: read_glyph_opacity(&store),
        combo_growth_enabled: read_combo_growth_enabled(&store),
    }
}

/// 設定パネルの「絵文字の大きさ」スライダーから呼ばれる。値を保存した上で、
/// 今開いているTauriのオーバーレイウィンドウとOBS側の両方に即座に反映する
/// (どちらもウィンドウ/ブラウザソースを開き直さなくてもその場で変わる)。
#[tauri::command]
fn set_overlay_glyph_scale(
    app: tauri::AppHandle,
    store: tauri::State<ConfigStore>,
    obs_bridge: tauri::State<obs_bridge::ObsBridge>,
    scale: f64,
) -> Result<(), String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("scaleは正の数で指定してください".to_string());
    }
    // 極端な値で表示が壊れないよう範囲を絞る(overlay.js側のapplyGlyphScaleの
    // クランプ範囲と合わせてある)。
    let clamped = scale.clamp(0.3, 3.0);
    store
        .set("glyphScale".to_string(), serde_json::json!(clamped))
        .map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window("overlay") {
        win.emit(
            "overlay:settings",
            serde_json::json!({ "glyphScale": clamped }),
        )
        .map_err(|e| e.to_string())?;
    }
    obs_bridge.set_glyph_scale(clamped);
    Ok(())
}

/// 設定パネルの「スタンプの透明度」スライダーから呼ばれる。set_overlay_glyph_scale
/// と同じ構造(保存→Tauriオーバーレイへemit→OBS側へも反映)。
#[tauri::command]
fn set_overlay_glyph_opacity(
    app: tauri::AppHandle,
    store: tauri::State<ConfigStore>,
    obs_bridge: tauri::State<obs_bridge::ObsBridge>,
    opacity: f64,
) -> Result<(), String> {
    if !opacity.is_finite() || opacity <= 0.0 {
        return Err("opacityは正の数で指定してください".to_string());
    }
    // 0(完全に見えない)は事実上「表示しない」と同じで意味が無く、誤操作で
    // そこまで下げてしまうと「リアクションが届いているのに何も見えない」と
    // 誤解される恐れがあるため、下限を0.1(10%)に絞っておく。
    let clamped = opacity.clamp(0.1, 1.0);
    store
        .set("glyphOpacity".to_string(), serde_json::json!(clamped))
        .map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window("overlay") {
        win.emit(
            "overlay:settings",
            serde_json::json!({ "glyphOpacity": clamped }),
        )
        .map_err(|e| e.to_string())?;
    }
    obs_bridge.set_glyph_opacity(clamped);
    Ok(())
}

/// 設定パネルの「連打すると大きくなる」チェックボックスから呼ばれる。
#[tauri::command]
fn set_overlay_combo_growth(
    app: tauri::AppHandle,
    store: tauri::State<ConfigStore>,
    obs_bridge: tauri::State<obs_bridge::ObsBridge>,
    enabled: bool,
) -> Result<(), String> {
    store
        .set("comboGrowthEnabled".to_string(), serde_json::json!(enabled))
        .map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window("overlay") {
        win.emit(
            "overlay:settings",
            serde_json::json!({ "comboGrowthEnabled": enabled }),
        )
        .map_err(|e| e.to_string())?;
    }
    obs_bridge.set_combo_growth_enabled(enabled);
    Ok(())
}

// 視聴者アプリ(viewer-app-tauri)側で見つかった不具合と同種の予防策として、
// こちらもcfg_get/cfg_setを非同期化しておく。ConfigStore::set()は呼ばれる
// たびに設定ファイル全体を同期的にディスクへ書き込む実装で、素の`fn`の
// ままだとメインスレッド(イベントループ)上で実行されてしまう。配信者アプリ
// では現状これらを高頻度に呼ぶポーリング処理は無いため実害は出ていなかったが、
// 今後の変更で頻度が上がっても同じ問題(操作への反応が遅れる・終了が
// もたつく)が再発しないよう、揃って非同期実行に変更しておく。
#[tauri::command(async)]
fn cfg_get(store: tauri::State<ConfigStore>, key: String) -> Option<serde_json::Value> {
    store.get(&key)
}

#[tauri::command(async)]
fn cfg_set(
    store: tauri::State<ConfigStore>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    store.set(key, value).map_err(|e| e.to_string())
}

/// 中継サーバーから届いた(または動作確認用にコントロールパネルから送った)
/// リアクションを、オーバーレイウィンドウに転送する。
#[tauri::command]
fn emit_overlay_reaction(
    app: tauri::AppHandle,
    emoji: String,
    viewer_id: Option<String>,
) -> Result<(), String> {
    let viewer_id = viewer_id.unwrap_or_default();
    if let Some(win) = app.get_webview_window("overlay") {
        // リアクションが実際に表示される瞬間に合わせて最前面を取り直しておく
        // (2秒おきの定期的な取り直し(create_overlay_window参照)だけだと、
        // ちょうど裏に回っている数秒の間にリアクションが飛んできて見逃されて
        // しまう可能性があるため、表示のたびにも念のため取り直す)。
        let _ = win.set_always_on_top(true);
        // アニメーション自体は最長でも5秒程度で終わる(overlay.jsのanimationDurationMs
        // 参照)。この間はTopmostReasserterのバーストモードに入り、150ms間隔で
        // 最前面を取り直し続けることで、アニメーション再生中に「常に最前面」設定の
        // ゲーム側へ裏返されてしまう確率を下げる。
        app.state::<TopmostReasserter>().bump(Duration::from_millis(5000));
        win.emit(
            "overlay:reaction",
            serde_json::json!({ "emoji": emoji, "viewerId": viewer_id }),
        )
        .map_err(|e| e.to_string())?;
    }
    // OBSの「ブラウザ」ソースが繋がっていれば、そちらにも同じリアクションを配信する
    // (obs_bridge.rs参照。誰も繋いでいなくても問題なく無視される)。
    app.state::<obs_bridge::ObsBridge>()
        .broadcast_reaction(&emoji, &viewer_id);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 多重起動防止。2回目の起動を検知したら、既存のコントロールパネルを
            // 前面に出すだけにする(オーバーレイ・接続が重複するのを防ぐ)。
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                if let Some(w) = app.get_webview_window("control") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }))?;

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let app_data_dir = app.path().app_data_dir()?;
            let config_path = app_data_dir.join("broadcaster-config.json");
            let store = ConfigStore::load(config_path);

            // 初回起動時のみ部屋の認証情報を生成し、以後は同じものを使い続ける
            // (配信者が一度伝えた合言葉がずっと有効であるようにするため)
            let has_credentials =
                store.get("roomId").is_some() && store.get("broadcasterToken").is_some();
            if !has_credentials {
                let (room_id, broadcaster_token) = connect_code::generate_room_credentials();
                store
                    .set("roomId".into(), serde_json::json!(room_id))
                    .ok();
                store
                    .set(
                        "broadcasterToken".into(),
                        serde_json::json!(broadcaster_token),
                    )
                    .ok();
            }
            // 合言葉も初回起動時に適当な初期値を用意しておく(配信者は設定
            // パネルからいつでも好きな文字列に変更できる)。
            if store.get("passphrase").is_none() {
                store
                    .set(
                        "passphrase".into(),
                        serde_json::json!(connect_code::generate_default_passphrase()),
                    )
                    .ok();
            }

            let initial_glyph_scale = read_glyph_scale(&store);
            let initial_glyph_opacity = read_glyph_opacity(&store);
            let initial_combo_growth_enabled = read_combo_growth_enabled(&store);
            app.manage(store);
            app.manage(obs_bridge::start(
                initial_glyph_scale,
                initial_glyph_opacity,
                initial_combo_growth_enabled,
            ));
            // create_overlay_window内で立ち上げる最前面取り直しスレッドが参照する
            // ため、ウィンドウを作る前に必ず管理下に置いておく。
            app.manage(TopmostReasserter::new());

            let handle = app.handle().clone();
            create_overlay_window(&handle)?;
            create_control_window(&handle)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_credentials,
            get_relay_address,
            get_overlay_settings,
            set_overlay_glyph_scale,
            set_overlay_glyph_opacity,
            set_overlay_combo_growth,
            cfg_get,
            cfg_set,
            emit_overlay_reaction,
            list_monitors,
            set_overlay_monitor,
            updater::check_for_update,
            updater::download_and_apply_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
