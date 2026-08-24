mod config_store;
mod connect_code;
mod obs_bridge;
pub mod updater;

use config_store::ConfigStore;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
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

/// 「モニターの一部の範囲だけに表示する」機能用。選んだモニターに対する
/// 割合(0.0〜1.0)で範囲を持つ(解像度・モニター構成が変わっても壊れにくい
/// ようにするため、絶対ピクセル値ではなく割合で保存している)。
#[derive(Clone, Copy)]
struct RegionSpec {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl RegionSpec {
    /// 位置(0.0〜0.95)・サイズ(位置から見てモニターをはみ出さない範囲)に
    /// クランプする。呼び出し元(read_overlay_region/set_overlay_region)の
    /// どちらからも同じ規則で丸めるための共通処理。
    fn clamped(x: f64, y: f64, width: f64, height: f64) -> Self {
        let x = x.clamp(0.0, 0.95);
        let y = y.clamp(0.0, 0.95);
        let width = width.clamp(0.05, 1.0 - x);
        let height = height.clamp(0.05, 1.0 - y);
        Self { x, y, width, height }
    }
}

/// ConfigStoreに保存されている「表示範囲」設定を読む。無効化されている
/// (overlayRegionEnabledがtrueでない)場合はNoneを返し、呼び出し側は
/// モニター全体を使う(=従来通りの挙動)。
fn read_overlay_region(store: &ConfigStore) -> Option<RegionSpec> {
    let enabled = store
        .get("overlayRegionEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let x = store.get("overlayRegionX").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = store.get("overlayRegionY").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let width = store
        .get("overlayRegionWidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let height = store
        .get("overlayRegionHeight")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    Some(RegionSpec::clamped(x, y, width, height))
}

/// モニター全体の位置・サイズに対して、表示範囲(region。Noneならモニター
/// 全体をそのまま使う)を適用し、オーバーレイウィンドウに実際に設定すべき
/// 位置・サイズを計算する。
fn apply_overlay_region(
    monitor_pos: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    region: Option<RegionSpec>,
) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let Some(r) = region else {
        return (monitor_pos, monitor_size);
    };
    let x = monitor_pos.x + (monitor_size.width as f64 * r.x).round() as i32;
    let y = monitor_pos.y + (monitor_size.height as f64 * r.y).round() as i32;
    let width = ((monitor_size.width as f64 * r.width).round() as u32).max(50);
    let height = ((monitor_size.height as f64 * r.height).round() as u32).max(50);
    (PhysicalPosition::new(x, y), PhysicalSize::new(width, height))
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
        let (monitor_pos, monitor_size) = resolve_overlay_monitor(&app, monitor_id.as_deref());
        let region = read_overlay_region(&store);
        let (pos, size) = apply_overlay_region(monitor_pos, monitor_size, region);
        win.set_position(tauri::Position::Physical(pos))
            .map_err(|e| e.to_string())?;
        win.set_size(tauri::Size::Physical(size))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 「モニターの一部の範囲だけに表示する」設定を保存し、既に開いている
/// オーバーレイウィンドウの位置・サイズもその場で更新する。x/y/width/height
/// は選んだモニターに対する割合(0.0〜1.0)。enabledがfalseの場合は範囲設定
/// 自体は保存しつつ(次にONにした時のために)、ウィンドウはモニター全体に戻す。
#[tauri::command]
fn set_overlay_region(
    app: tauri::AppHandle,
    store: tauri::State<ConfigStore>,
    enabled: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let spec = RegionSpec::clamped(x, y, width, height);
    store
        .set("overlayRegionEnabled".to_string(), serde_json::json!(enabled))
        .map_err(|e| e.to_string())?;
    store
        .set("overlayRegionX".to_string(), serde_json::json!(spec.x))
        .map_err(|e| e.to_string())?;
    store
        .set("overlayRegionY".to_string(), serde_json::json!(spec.y))
        .map_err(|e| e.to_string())?;
    store
        .set("overlayRegionWidth".to_string(), serde_json::json!(spec.width))
        .map_err(|e| e.to_string())?;
    store
        .set("overlayRegionHeight".to_string(), serde_json::json!(spec.height))
        .map_err(|e| e.to_string())?;

    if let Some(win) = app.get_webview_window("overlay") {
        let saved_monitor_id = store
            .get("overlayMonitorId")
            .and_then(|v| v.as_str().map(String::from));
        let (monitor_pos, monitor_size) = resolve_overlay_monitor(&app, saved_monitor_id.as_deref());
        let region = if enabled { Some(spec) } else { None };
        let (pos, size) = apply_overlay_region(monitor_pos, monitor_size, region);
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
    // 取得できない場合は既定サイズにフォールバック)。「モニターの一部の範囲
    // だけに表示する」設定(overlayRegion*)が有効なら、そこからさらに絞り込む。
    let config_store = app.state::<ConfigStore>();
    let saved_monitor_id = config_store
        .get("overlayMonitorId")
        .and_then(|v| v.as_str().map(String::from));
    let (monitor_pos, monitor_size) = resolve_overlay_monitor(app, saved_monitor_id.as_deref());
    let region = read_overlay_region(&config_store);
    let (pos, size) = apply_overlay_region(monitor_pos, monitor_size, region);

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
    // 最小化した際は、タスクバーに最小化アイコンとして残す代わりにウィンドウ
    // ごと隠す(=タスクトレイのアイコンだけが残る状態にする)。配信中は
    // コントロールパネルを閉じずに(オーバーレイ・配信自体は継続したまま)
    // 邪魔にならないよう隠しておきたい、という要望への対応。
    // 詳しい理由はviewer-app-tauri/src-tauri/src/lib.rsのcreate_main_window
    // 内の同種のコメントを参照(Resizedイベントを使う回りくどい理由も同じ)。
    let win_for_minimize = win.clone();
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { .. } => {
            app_handle.exit(0);
        }
        tauri::WindowEvent::Resized(_) => {
            if win_for_minimize.is_minimized().unwrap_or(false) {
                let _ = win_for_minimize.hide();
            }
        }
        _ => {}
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
    store: tauri::State<ConfigStore>,
    emoji: String,
    viewer_id: Option<String>,
) -> Result<(), String> {
    let viewer_id = viewer_id.unwrap_or_default();
    // 「自分の画面には表示しない(OBS経由の配信画面にのみ表示)」設定。ONの間は
    // Tauri本体のオーバーレイウィンドウへの反映(=配信者自身のモニターに映る分)
    // だけを省略する。OBSの「ブラウザ」ソース向けの配信(下のobs_bridge呼び出し)は
    // この設定に関わらず常に行うため、視聴者が見る配信画面には通常通り表示される。
    let hide_local = store
        .get("hideLocalOverlay")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !hide_local {
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

            // タスクトレイに常駐アイコンを出す。コントロールパネルを最小化すると
            // ウィンドウごと隠れる(create_control_window参照)ため、再度開く・
            // 終了するための手段としてこのアイコンが唯一の入口になる
            // (viewer-app-tauri/src-tauri/src/lib.rsの同種の実装を踏襲)。
            let open_item =
                MenuItem::with_id(&handle, "open", "コントロールパネルを開く", true, None::<&str>)?;
            let minimize_item = MenuItem::with_id(
                &handle,
                "minimize",
                "コントロールパネルを隠す",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(
                &handle,
                "quit",
                "終了(配信の受信・オーバーレイ表示も終了します)",
                true,
                None::<&str>,
            )?;

            let tray_menu = Menu::with_items(&handle, &[&open_item, &minimize_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                // 重要: tray-iconクレートは既定でmenu_on_left_click=trueになっており、
                // 左クリックでもOS標準のメニューが自動的に開いてしまう。下記の
                // on_tray_icon_eventで「左クリックでウィンドウを復元する」独自の
                // 処理も別途行っているため、両方が同時に発火すると、メニューが
                // 開いた直後にウィンドウがフォーカスを奪ってメニューを閉じてしまい、
                // 「一瞬何か出てきてすぐ閉じる」ように見えるバグの原因になっていた
                // (viewer-app-tauri側の同種の実装・コメントも参照)。左クリックでの
                // メニュー表示自体を無効化する。
                .show_menu_on_left_click(false)
                .tooltip("ReaCast(起動中)\n左クリックでコントロールパネルを表示します\n右クリックでメニューを表示します")
                .on_menu_event(move |app, event| {
                    let id = event.id.as_ref();
                    match id {
                        "quit" => {
                            app.exit(0);
                        }
                        "minimize" => {
                            if let Some(w) = app.get_webview_window("control") {
                                let _ = w.hide();
                            }
                        }
                        "open" => {
                            if let Some(w) = app.get_webview_window("control") {
                                let _ = w.unminimize();
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("control") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                });
            if let Some(icon) = handle.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            // トレイアイコンはあくまで補助機能なので、環境によって作成に失敗しても
            // (例: デスクトップ環境にトレイが無い等)アプリ本体は起動を続ける。
            if let Err(e) = tray_builder.build(&handle) {
                log::warn!("tray icon creation failed (continuing without it): {e}");
            }

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
            set_overlay_region,
            updater::check_for_update,
            updater::download_and_apply_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
