mod config_store;
mod pcwmp;
pub mod updater;

use config_store::ConfigStore;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const MANUAL_PICK_TIMEOUT_MS: u128 = 15_000;
const MANUAL_PICK_POLL_MS: u64 = 300;
const MAIN_WINDOW_LABEL: &str = "main";
// 複数配信者への同時接続機能用。プロファイルごとのリアクションバーは
// "bar-{プロファイルid}" というラベルの独立ウィンドウとして作る
// (open_bar_window/close_bar_window参照)。各コマンドは`label`引数を
// 省略するとMAIN_WINDOW_LABELを対象にする(既存の単一ウィンドウ時代の
// 呼び出し元との互換性のため)。
fn resolve_label(label: Option<String>) -> String {
    label.unwrap_or_else(|| MAIN_WINDOW_LABEL.to_string())
}

// 以前は接続コード(中継サーバーのアドレス・部屋ID・視聴者トークンを暗号化した
// 文字列)から視聴者アプリ側が実行時にアドレスを読み取っていたが、合言葉方式
// への移行に伴い、配信者アプリと同じ考え方(ビルド時にset REACTION_RELAY_HOST=...
// してからnpm run build)で中継サーバーのアドレスをexeに焼き込むようにした。
// 配信者アプリ・視聴者アプリを同じ人が同じ中継サーバー向けにビルドして配布する
// 運用を想定しているので、環境変数名も揃えてある。
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

#[derive(serde::Serialize)]
struct RelayAddress {
    host: String,
    port: u16,
}

/// ビルド時に焼き込まれた中継サーバーのアドレスを返す。設定パネルの「詳細設定」で
/// 上書きされている場合はJS側(main.js)でその値を優先して使う(ここではあくまで
/// ビルド時の既定値のみを返す)。
#[tauri::command]
fn get_relay_address() -> RelayAddress {
    RelayAddress {
        host: default_relay_host(),
        port: default_relay_port(),
    }
}

// 表示位置の設定値('comment'/'right'/'bottom'/'top'/'overlap-bottom')は
// 以前はここ(Rust側)にもタスクトレイのサブメニュー用に一覧を持っていたが、
// 複数配信者同時接続対応でタスクトレイの「表示位置」サブメニューを廃止した
// (配信者ごとに配置が異なりうるため、一覧画面側の各プロファイル設定に
// 一本化した)。値の一覧はフロントエンド側(main.html/main.jsのplacementSelect)
// にのみ存在する。

struct ManualPickState {
    cancelled: AtomicBool,
}

/// 以前はこのアプリを「初期設定(setup)」「リアクションバー(bar)」という2つの
/// 別ウィンドウで作り分け、接続完了時に設定側を閉じてバー側に切り替える設計に
/// していた。だが、これが原因で
///   - ×ボタンで閉じても裏にプロセスが残って完全終了しない
///   - 検出できていない間バーウィンドウ自体が非表示になり「何も出ない」ように見える
///   - 透過ウィンドウとして作っていたバーが、環境によっては単なる白い矩形として
///     描画されてしまう(WebView2の透過対応は環境依存で、常に効くとは限らない)
///   - ウィンドウの作成・破棄そのものに不具合が入り込みやすい
/// といった不具合が繰り返し発生していた。
///
/// そこで、ウィンドウは起動から終了まで常に1つだけ(常に表示・枠あり・不透明の
/// 普通のウィンドウ)にし、「設定パネル」と「リアクションパネル」の切り替えは
/// 同じウィンドウの中でフロントエンド(main.js)側が行うだけ、というシンプルな
/// 構成に作り直した。ウィンドウの作成・close()はこのファイルでは起動時の1回と
/// 終了時しか行わないため、以前あった「意図的なclose()かユーザーの×クリックか
/// を見分けるためのCloseGuard」も不要になった。
fn create_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(MAIN_WINDOW_LABEL).is_some() {
        return Ok(());
    }
    // .transparent(true)をビルド時に指定しておく(重要)。Windowsでは、ウィンドウを
    // 後から透過に「切り替える」ことは基本的にできず、透過に対応した状態
    // (レイヤードウィンドウ等)で最初から作っておく必要がある。実際の見た目の
    // 透明/不透明はset_overlay_mode側でset_background_colorを使って動的に
    // 切り替える(詳しくはそちらのコメント参照)。
    let win = WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("main.html".into()))
        .title("ReaTap")
        .inner_size(440.0, 620.0)
        .min_inner_size(320.0, 200.0)
        .decorations(true)
        .transparent(true)
        .always_on_top(true)
        .resizable(true)
        .visible(true)
        // このウィンドウが最初からOS入力フォーカスを奪ってしまうと、視聴側で見ている
        // pcwmp/PCRPlayer側がフォーカスを失って(設定によっては)最小化・タスクバーに
        // 落ちてしまうことがあるため、初期フォーカスを与えないようにする
        .focused(false)
        // v1.3.2で追加した「配信者一覧・並び順」画面のドラッグ&ドロップ並び替え
        // (main.js: 行の掴みアイコンをドラッグして順番を変える機能)が、掴みアイコン
        // (cursor:grab)は出るのに実際にドラッグを始めるとOSの「禁止」カーソルに
        // 変わって何も動かせない、という不具合として報告された。
        //
        // 原因: TauriはWindows/WebView2向けに、OSのファイルをウィンドウへドラッグ
        // &ドロップした際にファイルパスを受け取るための、ネイティブ側のドラッグ&
        // ドロップハンドラを既定で有効にしている。このネイティブハンドラが
        // HTML5のドラッグ&ドロップAPI(dragstart/dragover/drop等)よりも先に
        // ドラッグ操作を横取りしてしまい、「実際のファイルではない」ドラッグは
        // OS側に拒否される(=禁止カーソル)ため、ページ側のJSのドラッグ&ドロップ
        // 処理まで一切届いていなかった。このウィンドウではファイルのドラッグ&
        // ドロップを受け取る用途は無く、並び替え機能でHTML5側のドラッグ&ドロップ
        // APIを使う必要があるため、Tauri側のネイティブハンドラを無効化しておく。
        .disable_drag_drop_handler()
        .build()?;

    // 起動直後は「設定パネル」(不透明・枠あり)として使うので、明示的に不透明の
    // 背景色を設定しておく。.transparent(true)で作ってはいるが、背景色を
    // 明示しないとWebView2の既定色(環境依存)のまま描画されてしまう可能性が
    // あるため、ここで確実に指定する。
    let _ = win.set_background_color(Some(tauri::window::Color(0x17, 0x18, 0x1c, 255)));

    // ×ボタンを押したら常にアプリ全体を終了する。複数配信者同時接続に対応した
    // ことで、このタイミングで"bar-*"の透明オーバーレイウィンドウが同時に何枚も
    // 開いていることがある。app_handle.exit()でプロセスを一気に落とす前に、
    // まずそれらのウィンドウを1枚ずつ明示的にclose()しておく。理由: WebView2は
    // ウィンドウごとに裏で描画用の子プロセスを持っており、いきなりプロセス全体を
    // 終了させると、開いているウィンドウ数が多いほどこれらの後始末が間に合わず
    // 残ってしまい、タスクマネージャーで強制終了しないと完全には終了できない
    // (ように見える)ことがある、という報告があったための対策。
    let app_handle = app.clone();
    // 最小化(OS標準の「_」ボタン、またはWin+Downなど)した瞬間を検知して、
    // タスクバーに最小化アイコンとして残す代わりにウィンドウごと隠す
    // (=タスクトレイのアイコンだけが残る状態にする)。「最小化した際に
    // 右下のタスクバーに収まるようにしてほしい」という要望への対応。
    //
    // 重要: Tauriには「最小化された」を直接表す専用のWindowEventが無いため、
    // Resizedイベント(最小化されると呼ばれる。OSが最小化時に極小サイズへの
    // リサイズを行うため)をフックし、その時点でis_minimized()を確認して
    // 実際に最小化状態になっていた場合だけhide()する、という回りくどい方法を
    // 取る。hide()されたウィンドウはis_minimized()が意味を持たない状態になり、
    // タスクバーからも消える(タスクトレイのアイコン左クリック/「配信者一覧を
    // 開く」メニューで復元する。tray_builder参照)。
    let win_for_minimize = win.clone();
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { .. } => {
            close_all_bar_windows(&app_handle);
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

/// "bar-*"ラベルの全ウィンドウ(接続中の各配信者のリアクションバー)を
/// 明示的に閉じる。アプリ終了直前の後始末用(create_main_windowの
/// コメント参照)。既に閉じている/存在しない場合は何もしない。
fn close_all_bar_windows(app: &tauri::AppHandle) {
    for (label, win) in app.webview_windows() {
        if label.starts_with("bar-") {
            let _ = win.close();
        }
    }
}

/// 複数配信者への同時接続機能用の、プロファイル1件分のリアクションバー
/// ウィンドウ。以前はmainウィンドウ1つを「設定パネル/リアクションパネル」に
/// 動的に切り替えていたが(set_overlay_mode。今は廃止)、複数の配信者に
/// 同時接続してそれぞれ別の位置に独立したバーを出すにはウィンドウそのものを
/// 複数持つ必要があるため、mainウィンドウは常に「配信者一覧・設定」パネル専用
/// (枠あり・不透明)とし、実際にアイコンを表示する透明なオーバーレイは
/// プロファイルごとにこの関数で作る(ラベルは"bar-{プロファイルid}")。
///
/// 配信者アプリのcreate_overlay_window(broadcaster-app-tauri/src-tauri/src/lib.rs)
/// と同じ考え方で、最初から透明・枠なしで作る(Windowsでは透過を後から
/// 切り替えられないため)。クリックはこのアプリでは必要(ボタンを押す)なので
/// 配信者アプリのオーバーレイと違いset_ignore_cursor_eventsは呼ばない。
fn create_bar_window(app: &tauri::AppHandle, label: &str) -> tauri::Result<()> {
    if app.get_webview_window(label).is_some() {
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::App("bar.html".into()))
        .title("ReaTap")
        .inner_size(200.0, 200.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        // この枠なし透明ウィンドウの余白部分にはdata-tauri-drag-region
        // (ドラッグ移動用)を付けてあり、そこをダブルクリックするとOS側の
        // 「最大化」動作が発動してしまい、「追従なしでバーを動かそうと
        // ダブルクリックしたら判定領域が画面いっぱいに巨大化して左上に移動する」
        // という不具合があった。修正としてmaximizable(false)だけを指定する
        // (最大化ボックスを持たないウィンドウはOSもダブルクリック最大化を
        // 発動しない)。
        //
        // 重要: 以前はここに.resizable(false)も併せて指定していたが、これが
        // 原因でこのウィンドウをdata-tauri-drag-regionでドラッグ移動すること
        // 自体ができなくなる(=表示位置を変更できなくなる)不具合が実際に
        // 発生した。サイズ自体は常にresize_windowコマンド(bar.js)経由で
        // プログラム側から決めており、ユーザーが手でリサイズする用途は元々
        // 無いためresizableをfalseにしても実害は無いつもりだったが、Windows上の
        // 挙動としてリサイズ不可のウィンドウはドラッグでの移動そのものも
        // 一緒に効かなくなることがあるようだったため、resizableはtrueのまま
        // (既定値)にして、最大化だけをmaximizable(false)でピンポイントに
        // 抑止する形に改めた。
        .maximizable(false)
        .shadow(false)
        // 起動直後は対象ウィンドウの検出がまだ済んでいないため、いきなり
        // 200x200の未配置状態で一瞬表示されてしまうのを避けるためfalseで作る。
        // bar.js側のdetectAndFollowOnce()が初回のポーリングで検出結果に応じて
        // show()/hide()を呼び、実際に表示すべき状態になってから表示する。
        .visible(false)
        // 対象(pcwmp/PCRPlayer)側のフォーカスを奪わないよう、mainウィンドウと
        // 同じく初期フォーカスを与えない。
        .focused(false)
        .build()?;

    win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))?;
    // mainウィンドウ(旧enabled=falseの設定パネル)と違い、このウィンドウは
    // 常にアイコン列ぴったりの小さいサイズで使うため、最小サイズ制限を
    // 実質無しにしておく(既定の最小サイズのままだと、アイコン列より小さくは
    // 縮められずに透明な余白がクリックを奪ってしまう不具合が以前発生したため)。
    win.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(1.0, 1.0))))?;

    // 「絵文字ボタンを押すたびに配信ウィンドウが非アクティブになり、連打すると
    // 枠の色がちらつく」という報告への対応。.focused(false)は「作成直後だけ
    // フォーカスを与えない」設定でしかなく、その後の実際のクリックによる
    // フォーカス奪取は防げない。WS_EX_NOACTIVATEを立てることで、このウィンドウは
    // クリックされても構造的に一切アクティブ化されなくなる(pcwmp::set_no_activate
    // 参照)。
    #[cfg(target_os = "windows")]
    {
        if let Ok(hwnd) = win.hwnd() {
            pcwmp::set_no_activate(hwnd.0 as isize);
        }
    }

    Ok(())
}

/// 配信者一覧画面(main.js)の「接続する」から呼ばれる。既に同じラベルの
/// バーが存在する場合は何もしない(create_bar_window内で判定済み)。
///
/// cfg_get/cfg_setと同じ理由(上のコメント参照)でasync化している。ウィンドウの
/// 作成・操作そのものはTauriが内部で常にメインスレッドへ処理を回すため、
/// どのスレッドから呼んでも安全(detect_target_window/start_manual_pickで
/// 既に確立済みの前提)。
#[tauri::command(async)]
fn open_bar_window(app: tauri::AppHandle, label: String) -> Result<(), String> {
    create_bar_window(&app, &label).map_err(|e| e.to_string())
}

/// 配信者一覧画面の「切断する」から呼ばれる。ウィンドウが既に無い場合も
/// エラーにはしない(二重切断や、ユーザーが何らかの理由で先に閉じていた
/// 場合でも一覧側の状態更新自体は問題なく進められるようにするため)。
#[tauri::command(async)]
fn close_bar_window(app: tauri::AppHandle, label: String) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(&label) {
        win.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 配信者一覧画面(main.js)でプロファイル個別設定(表示位置・行数・
/// ホットキー・手動選択した対象パスなど)を変更した際に呼ばれる。対象の
/// バーウィンドウ("bar-{id}")がまだ開いていれば、そのウィンドウだけに
/// "profile-settings-changed"イベントを送り、bar.js側で再接続せずその場で
/// 新しい設定を読み直させる(既に開いていなければ何もしない。次に「接続する」
/// した時に新しい設定で開かれるので問題ない)。
#[tauri::command(async)]
fn notify_profile_settings_changed(app: tauri::AppHandle, label: String) -> Result<(), String> {
    if app.get_webview_window(&label).is_some() {
        // 重要: WebviewWindow::emit()は全ウィンドウへのブロードキャストになって
        // しまう(このアプリのように複数のbar-*ウィンドウが同時に存在しうる場合、
        // 別プロファイルのバーにまで誤って「設定が変わった」と伝わってしまう)。
        // 特定のラベルのウィンドウだけに送るにはAppHandle::emit_to()を使う必要がある。
        app.emit_to(&label, "profile-settings-changed", ())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ホットキー発火のルーティング用。
///
/// 重要な設計上の理由: tauri-plugin-global-shortcutは、登録済みショートカットを
/// 「ウィンドウ単位」ではなく「アプリ全体で1つだけ」のHashMapとして管理している
/// (プラグイン内部のGlobalShortcut構造体はapp.manage()で1つだけ生成される)。
/// そのため、あるウィンドウ(バー)がgs.unregisterAll()を呼ぶと、他のプロファイルの
/// バーが登録したショートカットまで巻き添えで消えてしまう。この問題を避けるため、
/// ホットキーの登録・解除はメインウィンドウ(main.js)からのみ一括で行う設計にし、
/// 各バーウィンドウ(bar.js)は一切gs.register/unregisterを呼ばない。
///
/// メインウィンドウで押されたホットキーは、対応するプロファイルのバーウィンドウに
/// このコマンド経由で"hotkey-reaction"イベントとして転送し、実際のリアクション
/// 送信(連打防止のdebounceやWebSocket送信)はそのバー自身のJSコンテキストに
/// 任せる(バーごとに独立したrelayClientを持っているため)。
#[tauri::command(async)]
fn send_hotkey_reaction(app: tauri::AppHandle, label: String, emoji_id: String) -> Result<(), String> {
    if app.get_webview_window(&label).is_some() {
        app.emit_to(&label, "hotkey-reaction", emoji_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

#[tauri::command]
fn is_auto_detect_available() -> bool {
    pcwmp::is_available()
}

/// Win32のウィンドウ列挙・タイトル取得等を伴うため、メインスレッド(イベント
/// ループ)をブロックしないよう非同期実行にしている(start_manual_pickで
/// 見つかった不具合と同種の問題を避けるため)。
///
/// 「だめもと」のオーナーウィンドウ実験機能: 対象ウィンドウが見つかるたびに、
/// このアプリ自身のウィンドウのオーナーを対象ウィンドウに設定する
/// (SetWindowLongPtr + GWLP_HWNDPARENT)。Windowsは「オーナーを持つウィンドウは
/// 常にオーナーより手前に表示される」ことを構造的に保証するため、これまでの
/// 「お互いにHWND_TOPMOSTを取り合う」方式より安定して手前に表示できる可能性が
/// ある。見つからなかった場合はオーナー設定を解除する(0を渡す)。
///
/// 注意: この効果は実機(Windows)でしか確認できない(このリポジトリの開発・
/// 検証環境はLinuxのため、Windows専用コードはコンパイルもテストもできない)。
/// 対象が本物のDirectX排他フルスクリーンで描画している場合は、この方法でも
/// 効果が無い可能性がある。
#[tauri::command(async)]
fn detect_target_window(
    app: tauri::AppHandle,
    override_path: Option<String>,
    // 「配信者名」による絞り込み(PeerCastの多重起動対策)。空文字/Noneなら
    // 従来通り絞り込み無し。
    broadcaster_name: Option<String>,
    // オーナーウィンドウ設定の対象となる、呼び出し元ウィンドウのラベル。
    // 複数配信者同時接続では、プロファイルごとに別のバーウィンドウ
    // ("bar-{id}")がそれぞれ自分の対象を検出するため、常にmainウィンドウを
    // 対象にしていた以前の実装のままだと、後から検出した方のオーナー設定で
    // 前の分が上書きされてしまう。省略時はMAIN_WINDOW_LABEL(後方互換)。
    window_label: Option<String>,
) -> Option<pcwmp::WindowInfo> {
    let result = pcwmp::detect_target_window(override_path.as_deref(), broadcaster_name.as_deref());

    #[cfg(target_os = "windows")]
    {
        let label = resolve_label(window_label);
        if let Some(main_win) = app.get_webview_window(&label) {
            if let Ok(our_hwnd) = main_win.hwnd() {
                // Tauriが内部で使っているwindowsクレートのバージョンと、この
                // クレート自身が依存しているwindowsクレートのバージョンが
                // 異なる可能性があるため(実際に確認済み: Tauri内部は0.61系、
                // こちらは0.62系)、HWND型を直接やり取りせず、必ずisize
                // (ポインタの整数値)に変換してモジュール境界を越える。
                let our_raw = our_hwnd.0 as isize;
                let owner_raw = result.as_ref().map(|w| w.raw_handle).unwrap_or(0);
                pcwmp::set_window_owner(our_raw, owner_raw);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &app;
        let _ = window_label;
    }

    result
}

/// リアクションバーのボタンをクリックした直後に呼ばれる。バー自身は
/// `focused(false)`で作っていても、ボタンのクリックそのものでOSがバー側に
/// キーボードフォーカスを渡してしまい、pcwmp/PCRPlayerのコメント入力欄から
/// フォーカスが外れてしまう、というテスター報告への対応。呼び出し元
/// (bar.js)が持っているraw_handle(detect_target_windowが返したもの)を
/// そのまま渡してもらい、Win32のSetForegroundWindowでそちらへフォーカスを
/// 戻す。対象が見つかっていない(raw_handle=0や既に閉じられた等)場合は
/// 何もしない。
#[tauri::command(async)]
fn focus_target_window(raw_handle: isize) {
    if raw_handle == 0 {
        return;
    }
    // pcwmp::set_foreground_window自体がWindows以外では何もしないスタブに
    // なっているため、ここでcfg分岐する必要は無い(set_window_ownerと同じ
    // パターン)。
    pcwmp::set_foreground_window(raw_handle);
}

/// bar.jsのonClickEmojiから、focus_target_windowを呼ぶ前に確認用として
/// 呼ばれる。「クリックした時点で対象(pcwmp/PCRPlayer)が本当にフォアグラウンド
/// だったか」をここで判定し、trueの場合のみ呼び出し側がfocus_target_window
/// (=SetForegroundWindow)を呼ぶことで、「元々フォーカスが無かった場合にまで
/// 無条件でフォーカスを奪い返しに行く」ことによる連打時のちらつきを避けつつ、
/// 「コメント欄にフォーカスがある状態で押した時は戻ってほしい」という要望を
/// 両立させる。
#[tauri::command(async)]
fn is_window_foreground(raw_handle: isize) -> bool {
    if raw_handle == 0 {
        return false;
    }
    pcwmp::is_window_foreground(raw_handle)
}

/// 「候補一覧から選ぶ」機能用。クラス名・パスの自動判定に頼らず、今動いている
/// ウィンドウの一覧をそのままフロントエンドに渡し、人間が目で見て選べるようにする。
///
/// 重要: 以前はこの関数が素の`fn`(非同期指定なし)だったため、Tauriの仕組み上
/// メインスレッド(イベントループのスレッド)上でそのまま実行されていた。
/// ウィンドウ数が多い環境や、応答が遅いウィンドウが混ざっている環境では、
/// この列挙処理がイベントループを長時間ブロックしてしまい、「一覧を更新」を
/// 押しても「取得中...」のまま返ってこないように見える不具合の原因になっていた
/// (start_manual_pickで見つかったのと同じ種類の問題)。
/// #[tauri::command(async)]を付けて別スレッドで実行させることで解消する。
#[tauri::command(async)]
fn list_candidate_windows() -> Vec<pcwmp::WindowInfo> {
    let self_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()));
    pcwmp::filter_candidate_windows(pcwmp::list_windows(), self_path.as_deref())
}

// 重要: 以前はcfg_get/cfg_setが素の`fn`(非同期指定なし)だったため、
// start_manual_pick/list_candidate_windowsで既に見つかっていたのと同じ理由で
// メインスレッド(イベントループ)上でそのまま実行されていた。ConfigStore::set()は
// 呼ばれるたびに設定ファイル全体をディスクに同期書き込みする実装であり、
// bar.js側が接続中のプロファイルごとに250msおき(reportStatus)に、main.js側も
// 1秒おき(状態ポーリング)にcfg_get/cfg_setを呼び続けるため、接続中の配信者が
// いる間はメインスレッドがこの同期IPC呼び出しでほぼ埋まってしまい、他の操作
// (詳細設定を開く・削除する等のクリック、ウィンドウを閉じる際のapp.exit()自体)
// が長時間(場合によっては秒単位)待たされ、「反応がない」「終了できず強制終了が
// 必要」という不具合として現れていた(Windowsではディスク書き込みがアンチ
// ウイルスのリアルタイムスキャン等でさらに遅くなりやすく、症状が悪化しやすい)。
// #[tauri::command(async)]を付けて非同期ランタイムのワーカースレッドで実行させる
// ことで、メインスレッドを塞がないようにする(detect_target_window等、既存の
// 他コマンドで確立済みの対策パターンと同じ考え方)。
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

// 以下のウィンドウ操作系コマンドも、接続中は250msおきのポーリング
// (detectAndFollowOnce → positionOverWindow/shrinkToIconSize)から頻繁に
// 呼ばれる。上記と同じ理由でメインスレッドを長時間専有しうるため、揃って
// 非同期化しておく(WebviewWindowの各メソッド自体は元々どのスレッドから
// 呼んでも安全な設計になっている。start_manual_pickで既に同じ前提で
// win.hide()/win.show()をメインスレッド外から呼んでいるのと同様)。
#[tauri::command(async)]
fn position_bar_window(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    label: Option<String>,
) -> Result<(), String> {
    let label = resolve_label(label);
    let win = app
        .get_webview_window(&label)
        .ok_or("window not found")?;
    // 重要: pcwmp::window_info()(Win32のGetWindowRect)が返す座標・サイズは
    // 物理ピクセル(実ディスプレイ上のピクセル数)であり、Windowsの表示スケール
    // (125%/150%など)が100%でない環境では、OSの論理ピクセルとは値が異なる。
    // 以前はLogicalとして扱っていたため、表示スケールが100%でない環境では
    // ウィンドウが対象とズレた位置・サイズになっていた可能性がある(100%環境では
    // 論理=物理なので問題が表面化しない)。物理ピクセルとして正しく扱うよう修正。
    win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
        x, y,
    )))
    .map_err(|e| e.to_string())?;
    win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
        width, height,
    )))
    .map_err(|e| e.to_string())?;
    // 対象ソフト(pcwmp/PCRPlayer等)側が「常に最前面」に設定されていると、
    // お互いに最前面を取り合ってウィンドウが裏に隠れてしまうことがあるため、
    // 位置を更新するたび(0.25秒おき)に最前面を取り直す。ただし根本的な対策は
    // フロントエンド側で「対象と重ならない位置に配置する」ことにしてあるので、
    // これはあくまで最後の保険(画面の都合でどうしても重ねて置くしかない場合)。
    let _ = win.set_always_on_top(true);
    Ok(())
}

/// サイズだけをアイコン列にぴったり合わせて縮める(位置はそのまま)。
///
/// 対象ウィンドウ(pcwmp/PCRPlayer)がまだ検出できていない間、リアクション
/// パネルは(重ねる配置で位置合わせできないので)以前設定パネルとして使っていた
/// 440x620のサイズのまま透明になってしまう。中身が見えないので気付きにくいが、
/// その透明な範囲がクリックを奪ってしまい、下にある配信画面やコメント欄の操作を
/// 妨げてしまう。検出できるまでの間は、この関数でアイコン列の実寸まで先に
/// 縮めておくことで、透明な当たり判定を最小限にする。
#[tauri::command(async)]
fn resize_window(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
    label: Option<String>,
) -> Result<(), String> {
    let label = resolve_label(label);
    let win = app
        .get_webview_window(&label)
        .ok_or("window not found")?;
    win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
        width, height,
    )))
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// リアクションパネルを対象ウィンドウの隣・上に配置する際、画面外にはみ出さない
/// ようにするための画面情報。
///
/// 重要: 以前はwin.current_monitor()(このウィンドウ自身が今乗っているモニター
/// だけ)の範囲を返していたため、pcwmpをドラッグして別のモニターへ移動させると、
/// 「(このウィンドウがまだ古いモニター上にいる間に計算される)古いモニターの
/// 範囲」でclampされてしまい、新しいモニター側にはみ出す座標が古いモニターの
/// 端に強制的に引き戻されてしまう(=モニターをまたいで移動できているように
/// 見えない)不具合があった。available_monitors()で全モニターを取得し、それらを
/// 包む最大の矩形(仮想デスクトップ全体の範囲)を返すようにすることで、
/// どのモニターに対象が移動してもその範囲内になら追従できるようにした。
#[tauri::command(async)]
fn get_screen_bounds(app: tauri::AppHandle, label: Option<String>) -> Result<serde_json::Value, String> {
    let label = resolve_label(label);
    let win = app
        .get_webview_window(&label)
        .ok_or("window not found")?;
    let monitors = win.available_monitors().map_err(|e| e.to_string())?;

    if monitors.is_empty() {
        // 取得できない環境向けのフォールバック: 今のモニターだけを使う
        let monitor = win
            .current_monitor()
            .map_err(|e| e.to_string())?
            .ok_or("no monitor found")?;
        let pos = monitor.position();
        let size = monitor.size();
        return Ok(serde_json::json!({
            "x": pos.x,
            "y": pos.y,
            "width": size.width,
            "height": size.height,
        }));
    }

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for m in &monitors {
        let pos = m.position();
        let size = m.size();
        min_x = min_x.min(pos.x);
        min_y = min_y.min(pos.y);
        max_x = max_x.max(pos.x + size.width as i32);
        max_y = max_y.max(pos.y + size.height as i32);
    }

    Ok(serde_json::json!({
        "x": min_x,
        "y": min_y,
        "width": (max_x - min_x).max(0) as u32,
        "height": (max_y - min_y).max(0) as u32,
    }))
}

/// 「常に最前面」チェックボックスの切り替え用。ユーザー自身が明示的にオフに
/// できるようにしておくことで、最前面表示が邪魔な場合に手動で解除できるようにする。
#[tauri::command(async)]
fn set_always_on_top(
    app: tauri::AppHandle,
    enabled: bool,
    label: Option<String>,
) -> Result<(), String> {
    let label = resolve_label(label);
    let win = app
        .get_webview_window(&label)
        .ok_or("window not found")?;
    win.set_always_on_top(enabled).map_err(|e| e.to_string())
}

/// 「対象ウィンドウを手動選択」機能。
/// 呼び出し元のウィンドウ(main)を一時的に隠し、ユーザーが次にクリックした
/// ウィンドウ(アクティブウィンドウ)を最大15秒待って拾う。
///
/// 重要: 以前は「ブロッキング処理なのでTauriが自動的に専用スレッドで実行する」と
/// 誤解してこの属性を付けていなかったが、これは誤り。Tauriは#[tauri::command]の
/// 関数が素の`fn`(asyncではない)の場合、既定では**メインスレッド(イベントループ
/// のスレッド)上でそのまま実行する**(async runtime::spawnされるのは`async fn`か
/// #[tauri::command(async)]を明示した場合のみ)。素のfnのままここでstd::thread::sleep
/// を最大15秒ループしていたため、その間イベントループそのものが完全に止まっていた。
/// win.hide()/win.show()はイベントループがメッセージを処理して初めて画面に反映される
/// ため、「隠れると書いてあるのに実際には全く隠れない」(hide()もshow()も要求だけ
/// キューに積まれ、関数が返ってイベントループが再開した瞬間にほぼ同時に処理されて
/// しまい見た目上何も起きない)という不具合の直接の原因になっていた。
/// #[tauri::command(async)]を付けてasync runtimeの別スレッドで実行させることで、
/// メインスレッド(イベントループ)を止めずにhide()が即座に画面へ反映されるようにする。
#[tauri::command(async)]
fn start_manual_pick(
    app: tauri::AppHandle,
    state: tauri::State<ManualPickState>,
    window_label: String,
) -> serde_json::Value {
    if !pcwmp::is_available() {
        return serde_json::json!({
            "ok": false,
            "message": "この機能はWindows上でのみ利用できます(現在の実行環境では非対応です)"
        });
    }

    state.cancelled.store(false, Ordering::SeqCst);
    let win = app.get_webview_window(&window_label);
    if let Some(w) = &win {
        let _ = w.hide();
    }
    std::thread::sleep(Duration::from_millis(500));

    let start = Instant::now();
    let result = loop {
        if state.cancelled.load(Ordering::SeqCst) {
            break serde_json::json!({ "ok": false, "message": "キャンセルされました" });
        }
        if let Some(active) = pcwmp::get_active_window() {
            if !active.path.is_empty() {
                break serde_json::json!({
                    "ok": true,
                    "title": active.title,
                    "path": active.path,
                });
            }
        }
        if start.elapsed().as_millis() > MANUAL_PICK_TIMEOUT_MS {
            break serde_json::json!({
                "ok": false,
                "message": "タイムアウトしました。もう一度お試しください"
            });
        }
        std::thread::sleep(Duration::from_millis(MANUAL_PICK_POLL_MS));
    };

    if let Some(w) = &win {
        let _ = w.show();
    }
    result
}

#[tauri::command]
fn cancel_manual_pick(state: tauri::State<ManualPickState>) {
    state.cancelled.store(true, Ordering::SeqCst);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 「タスクトレイから終了したはずなのに、後で開いたら前回のリアクションが
            // まだ出ていた」という報告への対策。app.exit(0)での強制終了時に
            // WebView2の子プロセス等が完全には終了しきれず残ってしまう可能性への
            // 保険として、Windows Job Objectで「このプロセスが終わったら子プロセスも
            // 道連れに終了させる」ことをOSに保証させる。起動直後、他の何よりも
            // 先に一度だけ設定しておく(詳細はpcwmp::harden_process_termination参照)。
            #[cfg(target_os = "windows")]
            pcwmp::harden_process_termination();

            // 多重起動防止。2回目の起動を検知したら、既存のウィンドウを
            // (最小化されていれば復元して)前面に出す。これが無いと二重起動時に
            // 「ダブルクリックしても何も出てこない」ように見えて、ユーザーが
            // 何度もクリックしてタスクマネージャー上にプロセスが積み上がってしまう。
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = w.unminimize();
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

            // ホットキー(グローバルショートカット)機能。個々のキー割り当ての
            // 登録・解除はフロントエンド(main.js)側からJS API
            // (window.__TAURI__.globalShortcut.register/unregister)を直接呼んで
            // 動的に行うので、ここではプラグイン自体を有効化するだけでよい
            // (アプリ起動時点で固定のショートカットを登録する必要はない)。
            app.handle()
                .plugin(tauri_plugin_global_shortcut::Builder::new().build())?;

            let app_data_dir = app.path().app_data_dir()?;
            let config_path = app_data_dir.join("viewer-config.json");
            let store = ConfigStore::load(config_path);

            app.manage(store);
            app.manage(ManualPickState {
                cancelled: AtomicBool::new(false),
            });

            let handle = app.handle().clone();
            // ウィンドウは常に1つだけ作る。設定パネルとリアクションパネルの
            // どちらを最初に見せるかはmain.js側がcfg_get('connectInfo')の結果で
            // 判断する(Rust側でウィンドウを作り分けない)。
            create_main_window(&handle)?;

            // タスクトレイに常駐アイコンを出す。メインウィンドウは常時表示だが、
            // それでもアプリが起動中であることを分かりやすく示し、「最小化」
            // (ユーザーが明示的に望んだ時だけウィンドウを隠す唯一の手段)・
            // 接続設定のやり直し・終了ができるようにしておく。
            // 「配信者を切り替える/追加する」= 配信者一覧・設定パネル(mainウィンドウ)を
            // 前面に出すだけの項目。以前はここでウィンドウ内のパネルを「設定」に
            // 切り替える(reset-requestedイベント)処理もしていたが、複数配信者
            // 同時接続対応により、mainウィンドウは常に配信者一覧・設定専用になった
            // (個々の配信者のリアクション表示は別ウィンドウ"bar-*"が担当する)ため、
            // 単に表示・前面化するだけでよくなった。
            let reset_item =
                MenuItem::with_id(&handle, "reset", "配信者一覧を開く", true, None::<&str>)?;
            let minimize_item =
                MenuItem::with_id(&handle, "minimize", "配信者一覧を隠す", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(&handle, "quit", "終了(すべてのリアクション表示を閉じる)", true, None::<&str>)?;

            let tray_menu = Menu::with_items(&handle, &[&minimize_item, &reset_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                // 重要: tray-iconクレートは既定でmenu_on_left_click=trueになっており、
                // 左クリックでもOS標準のメニューが自動的に開いてしまう。ここで
                // on_tray_icon_event(下記)により「左クリックでウィンドウを復元する」
                // 独自の処理も別途行っているため、両方が同時に発火すると、メニューが
                // 開いた直後にウィンドウがフォーカスを奪ってメニューを閉じてしまい、
                // 「一瞬何か出てきてすぐ閉じる」ように見えるバグの原因になっていた。
                // 左クリックでのメニュー表示自体を無効化し、右クリックでのメニュー
                // 表示・左クリックでの復元、それぞれ意図通り単独で動くようにする。
                .show_menu_on_left_click(false)
                .tooltip(
                    "ReaTap(起動中)\n左クリックで配信者一覧を表示します\n右クリックでメニューを表示します",
                )
                .on_menu_event(move |app, event| {
                    let id = event.id.as_ref();
                    match id {
                        "quit" => {
                            close_all_bar_windows(app);
                            app.exit(0);
                        }
                        "minimize" => {
                            // ユーザーが明示的に「隠す」を選んだ時だけmainウィンドウを
                            // 隠す。接続中の各配信者のバー("bar-*")はここでは一切
                            // 触らないため、一覧を隠してもリアクション表示は続行される。
                            if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                                let _ = w.hide();
                            }
                        }
                        "reset" => {
                            if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
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
                        // 左クリックで、隠れている(最小化中の)ウィンドウを表示・前面化する。
                        // タスクバーに収まる=最小化操作なので、元に戻す手段も明示的な
                        // ユーザー操作(この左クリック)で行う。
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window(MAIN_WINDOW_LABEL) {
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
            // (例: デスクトップ環境にトレイが無い等)アプリ本体は起動を続ける
            if let Err(e) = tray_builder.build(&handle) {
                log::warn!("tray icon creation failed (continuing without it): {e}");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_windows,
            is_auto_detect_available,
            detect_target_window,
            focus_target_window,
            is_window_foreground,
            list_candidate_windows,
            get_relay_address,
            cfg_get,
            cfg_set,
            position_bar_window,
            resize_window,
            get_screen_bounds,
            set_always_on_top,
            open_bar_window,
            close_bar_window,
            notify_profile_settings_changed,
            send_hotkey_reaction,
            start_manual_pick,
            cancel_manual_pick,
            updater::check_for_update,
            updater::download_and_apply_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
