//! pcwmp(またはPCRPlayerなど互換の視聴ソフト)ウィンドウの自動検出。
//!
//! Electron版(node-window-manager)では実行ファイルパスの末尾一致でしか
//! 判定できなかったが、Tauri版はWindowsのAPIを直接呼べるので、
//! 本来やりたかった「Win32のウィンドウクラス名(`TpcwmpMain`)による判定」を
//! 実装できる。ただし念のため、クラス名判定が外れた場合の保険として
//! 実行ファイル名での判定も残す(どちらか一致すれば検出成功とする)。
//!
//! 実際に使われているソフトはpcwmp単体とは限らない(実機テストで、pcwmp.exeでは
//! なく`PCRPlayer64.exe`を使っているケースが確認された)ため、既知の実行ファイル名を
//! 複数リストで持ち、そのどれかと完全一致(パス内のファイル名部分のみ、大文字小文字
//! 区別なし)すれば検出成功とする。ends_with方式だと`superpcwmp.exe`のような無関係な
//! ファイルまで誤検出してしまうため、ファイル名の完全一致に変更している。
//!
//! Windows以外(このリポジトリの開発・検証はLinux上で行っている)では
//! 常に「何も見つからない」スタブとして振る舞う。

use serde::Serialize;

pub const PCWMP_CLASS_NAME: &str = "TpcwmpMain";

/// 既知の対象ソフトの実行ファイル名(小文字)。新しい互換ソフトが見つかったら
/// ここに追加する。
pub const KNOWN_TARGET_EXE_NAMES: &[&str] =
    &["pcwmp.exe", "pcrplayer.exe", "pcrplayer64.exe"];

#[derive(Clone, Debug, Serialize)]
pub struct WindowInfo {
    pub title: String,
    pub class_name: String,
    pub path: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// 生のHWND値(ポインタ→整数変換)。Windows以外や、まだ実ウィンドウに
    /// 紐付いていないテストデータでは0。
    ///
    /// オーナーウィンドウ設定(set_window_owner)に使うためだけの値であり、
    /// フロントエンドに渡っても特に意味は無いが、実害も無いのでそのまま
    /// シリアライズしておく。
    pub raw_handle: isize,
}

/// 実行ファイルパスのファイル名部分が、既知の対象ソフトのいずれかと完全一致するか
/// (大文字小文字は区別しない)。
fn is_known_target_exe(path: &str) -> bool {
    let lower = path.to_lowercase();
    let file_name = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
    KNOWN_TARGET_EXE_NAMES.contains(&file_name)
}

/// ウィンドウ一覧からpcwmp(または互換ソフト)のウィンドウを探す(純粋なロジックなので
/// 単体テスト可能)。クラス名が一致するか、実行ファイル名が既知のものと一致するかの
/// どちらかで判定する。
///
/// 重要: 一致するウィンドウが複数見つかった場合、単純に最初の1つ(.find())を
/// 返すのではなく、その中で最も面積が大きいものを選ぶ。実機で「レスの上に
/// カーソルを合わせた時に出るスレッドのポップアップ」や「右クリックメニュー」に
/// リアクション表示が誤って追従してしまう不具合が見つかったが、原因はこれらの
/// ポップアップ・メニューがpcwmp/PCRPlayer本体と**同じプロセス(同じ実行ファイル
/// パス)から作られる別の小さなトップレベルウィンドウ**であるため、実行ファイル名
/// 一致の判定に(意図せず)引っかかってしまうこと。EnumWindowsはZ-order(手前に
/// あるものが先)で列挙されるため、ポップアップ表示中はそちらが本体より先に
/// 列挙されてしまい、.find()だと本体ではなくポップアップの位置・サイズを
/// 拾ってしまっていた。ポップアップ・メニューの類は本体の映像ウィンドウよりも
/// 明らかに小さいはずなので、「一致した中で一番大きいもの」を選ぶことで、
/// クラス名の詳細(ポップアップ側の正確なクラス名)を個別に把握しなくても
/// 頑健に本体だけを選べるようにしている。
pub fn find_pcwmp_window(windows: &[WindowInfo]) -> Option<&WindowInfo> {
    windows
        .iter()
        .filter(|w| w.class_name.eq_ignore_ascii_case(PCWMP_CLASS_NAME) || is_known_target_exe(&w.path))
        .max_by_key(|w| (w.width as u64) * (w.height as u64))
}

/// PeerCastを多重起動している(複数のpcwmp/PCRPlayerが同時に開いている)環境向けの
/// 絞り込み。「配信者名」(name_filter)が指定されていれば、候補ウィンドウのうち
/// タイトルにその文字列を含むものだけに絞ってから、従来通り最大面積のものを選ぶ
/// (PCRPlayerはウィンドウタイトルに配信者名が表示されるため、これで意図した
/// プレイヤーだけに絞り込める)。
///
/// 重要: name_filterを指定した場合、一致するウィンドウが1つも無ければ
/// 絞り込み無しにフォールバックせず、必ずNoneを返す。「違う名前の配信を
/// 開いてもそちらには飛ばない」という要望のための仕様で、name_filter付きの
/// 呼び出しでは「見つからない」の方が「別の配信者に誤爆する」より安全という
/// 判断による。name_filterが空文字/Noneの場合は、従来通り絞り込み無し
/// (find_pcwmp_windowと同じ)で動作する。
pub fn find_pcwmp_window_by_name<'a>(
    windows: &'a [WindowInfo],
    name_filter: Option<&str>,
) -> Option<&'a WindowInfo> {
    let trimmed = name_filter.map(str::trim).filter(|s| !s.is_empty());
    let Some(name) = trimmed else {
        return find_pcwmp_window(windows);
    };
    let name_lower = name.to_lowercase();
    windows
        .iter()
        .filter(|w| w.class_name.eq_ignore_ascii_case(PCWMP_CLASS_NAME) || is_known_target_exe(&w.path))
        .filter(|w| w.title.to_lowercase().contains(&name_lower))
        .max_by_key(|w| (w.width as u64) * (w.height as u64))
}

/// 手動選択で記録した実行ファイルパスを優先しつつ、対象ウィンドウを探す。
/// こちらも同じ理由で、パスが一致する候補が複数ある場合は最大面積のものを選ぶ。
/// override_pathが無い場合のみname_filterによる絞り込みが効く(手動選択は
/// 常に最優先)。
pub fn find_target_window<'a>(
    windows: &'a [WindowInfo],
    override_path: Option<&str>,
    name_filter: Option<&str>,
) -> Option<&'a WindowInfo> {
    if let Some(op) = override_path {
        let best = windows
            .iter()
            .filter(|w| w.path.eq_ignore_ascii_case(op))
            .max_by_key(|w| (w.width as u64) * (w.height as u64));
        if let Some(best) = best {
            return Some(best);
        }
    }
    find_pcwmp_window_by_name(windows, name_filter)
}

/// 「候補一覧から選ぶ」機能用の絞り込み(純粋なロジックなので単体テスト可能)。
/// タイトルが無いヘルパーウィンドウ等と、自分自身(呼び出し元アプリ)のウィンドウを除外する。
/// クラス名や実行ファイルパスの命名規則に依存せず、実際に動いているウィンドウの一覧から
/// 人間が目で見て選べるようにするための、自動検出がうまくいかない場合の代替手段。
pub fn filter_candidate_windows(
    windows: Vec<WindowInfo>,
    self_exe_path: Option<&str>,
) -> Vec<WindowInfo> {
    windows
        .into_iter()
        .filter(|w| !w.title.trim().is_empty())
        .filter(|w| match self_exe_path {
            Some(sp) => !w.path.eq_ignore_ascii_case(sp),
            None => true,
        })
        .collect()
}

#[cfg(target_os = "windows")]
mod win {
    use super::WindowInfo;
    use std::sync::Mutex;
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, MAX_PATH, WPARAM};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowThreadProcessId, IsWindowVisible, SendMessageTimeoutW, SetForegroundWindow,
        SetWindowLongPtrW, GWLP_HWNDPARENT, GWL_EXSTYLE, SMTO_ABORTIFHUNG, WM_GETTEXT,
        WS_EX_NOACTIVATE,
    };

    /// ウィンドウのタイトルを安全に取得する。
    ///
    /// GetWindowTextWは、対象ウィンドウが別プロセス(別スレッド)所有の場合、
    /// 内部的にそのウィンドウへWM_GETTEXTメッセージを送って応答を待つ。
    /// もしそのプロセスが(応答なし状態などで)メッセージを処理できずにいると、
    /// GetWindowTextWはタイムアウト無く無期限にブロックしてしまい、結果として
    /// ウィンドウ一覧の取得(list_candidate_windows)全体が「取得中...」のまま
    /// 永久に返ってこなくなる。SendMessageTimeoutW + SMTO_ABORTIFHUNGを使うことで、
    /// 応答が無い/固まっているウィンドウは無視して(タイトル空扱いで)先に進めるようにする。
    fn get_window_text_safe(hwnd: HWND, timeout_ms: u32) -> String {
        let mut buf = [0u16; 512];
        let mut result: usize = 0;
        let ret = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETTEXT,
                WPARAM(buf.len()),
                LPARAM(buf.as_mut_ptr() as isize),
                SMTO_ABORTIFHUNG,
                timeout_ms,
                Some(&mut result as *mut usize),
            )
        };
        if ret.0 == 0 {
            // タイムアウトまたは失敗(固まっている等)。空文字として扱い、
            // このウィンドウ自体は候補から除外させる(filter_candidate_windowsで
            // 空タイトルは弾かれる)。
            return String::new();
        }
        let len = result.min(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn window_process_path(hwnd: HWND) -> String {
        let mut pid: u32 = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid == 0 {
            return String::new();
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        let Ok(handle) = handle else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            )
        };
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        if ok.is_err() {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..size as usize])
    }

    fn window_info(hwnd: HWND) -> Option<WindowInfo> {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return None;
        }
        let title = get_window_text_safe(hwnd, 200);

        let mut class_buf = [0u16; 256];
        let class_len = unsafe { GetClassNameW(hwnd, &mut class_buf) };
        let class_name = String::from_utf16_lossy(&class_buf[..class_len.max(0) as usize]);

        let mut rect = windows::Win32::Foundation::RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return None;
        }
        let width = (rect.right - rect.left).max(0) as u32;
        let height = (rect.bottom - rect.top).max(0) as u32;
        if width == 0 || height == 0 {
            return None;
        }

        let path = window_process_path(hwnd);

        Some(WindowInfo {
            title,
            class_name,
            path,
            x: rect.left,
            y: rect.top,
            width,
            height,
            raw_handle: hwnd.0 as isize,
        })
    }

    pub fn is_available() -> bool {
        true
    }

    pub fn list_windows() -> Vec<WindowInfo> {
        static RESULT: Mutex<Vec<WindowInfo>> = Mutex::new(Vec::new());
        RESULT.lock().unwrap().clear();

        unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
            if let Some(info) = window_info(hwnd) {
                RESULT.lock().unwrap().push(info);
            }
            BOOL(1)
        }

        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
                Some(enum_proc),
                LPARAM(0),
            );
        }

        let mut out = RESULT.lock().unwrap();
        std::mem::take(&mut out)
    }

    pub fn get_active_window() -> Option<WindowInfo> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return None;
        }
        window_info(hwnd)
    }

    /// 「オーナーウィンドウ」化(だめもとの実験機能)。
    ///
    /// `SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner)` を使い、自分のウィンドウの
    /// 「オーナー」を対象(pcwmp/PCRPlayer)のウィンドウに設定する。Windowsは
    /// 「オーナーを持つウィンドウは常にオーナーより手前(上のz-order)に表示される」
    /// という制約を構造的に保証するため、"topmostの奪い合い"のような競合(タイマーで
    /// 定期的にHWND_TOPMOSTを再設定し続けるが、相手も同じことをしていると負けることが
    /// ある)に頼らずに済む可能性がある。
    ///
    /// owner_hwnd_rawに0を渡すと、オーナー設定を解除する(通常のトップレベル
    /// ウィンドウに戻す)。
    ///
    /// 注意: これは対象が「見た目だけ全画面(ウィンドウが画面いっぱいのサイズ)」の
    /// 通常ウィンドウの場合にのみ効果が見込める。対象アプリが本当のDirectX排他
    /// フルスクリーンモードで描画している場合は、OS自体のウィンドウ管理(DWM)を
    /// 経由しないため、この方法を含むどんな通常のウィンドウ操作でも上に表示させる
    /// ことはできない。
    ///
    /// 呼び出し元(Tauri側)は、TauriのWindow::hwnd()で得たHWND(呼び出し元とは
    /// 別バージョンのwindowsクレートの型)をそのまま渡せないため、`.0`で生の
    /// ポインタ値をisizeとして取り出してから渡す想定。ここではその生の値から
    /// このクレート自身のwindows 0.62版のHWND型を再構築する。
    pub fn set_window_owner(our_hwnd_raw: isize, owner_hwnd_raw: isize) {
        let our_hwnd = HWND(our_hwnd_raw as *mut core::ffi::c_void);
        unsafe {
            // 戻り値(以前の値)は使わないが、エラーにはしない
            // (だめもとの実験機能なので、失敗しても他の動作を止めない)
            let _ = SetWindowLongPtrW(our_hwnd, GWLP_HWNDPARENT, owner_hwnd_raw);
        }
    }

    /// 指定ウィンドウを前面(フォアグラウンド)に戻す。リアクションバーの
    /// ボタンをクリックすると(バー自身は`focused(false)`で作っていても)
    /// クリックそのものでOSがバー側にキーボードフォーカスを渡してしまい、
    /// pcwmp/PCRPlayerのコメント入力欄からフォーカスが外れてしまう、という
    /// テスター報告への対応。呼び出し元(このアプリ)がクリックを受けて直前まで
    /// フォアグラウンドだったプロセスであるため、Windowsのフォアグラウンド
    /// 奪取制限(無関係なプロセスが勝手にSetForegroundWindowできない仕組み)には
    /// 引っかからずに戻せる想定。失敗しても実害は無い(単に元の挙動に戻るだけ)
    /// ため戻り値は無視する。
    pub fn set_foreground_window(hwnd_raw: isize) {
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }

    /// このウィンドウ(リアクションバー)がクリックされてもOS側のフォーカス
    /// (アクティブウィンドウ)を一切奪わないようにする。
    ///
    /// 経緯: 「絵文字ボタンを押すたびに配信ウィンドウが非アクティブになり、
    /// 連打すると枠の色がちらついて気持ち悪い」という報告への対応。以前は
    /// クリック直後にset_foreground_window()でフォーカスを配信ウィンドウへ
    /// 事後的に戻していたが、これだと「一瞬奪う→戻す」を毎クリックごとに
    /// 繰り返すことになり、連打時にその切り替え自体が視覚的なちらつきの
    /// 原因になっていた。
    ///
    /// Win32の拡張ウィンドウスタイルWS_EX_NOACTIVATEを立てておくと、この
    /// ウィンドウはクリックされても一切アクティブ化されなくなる(=そもそも
    /// フォーカスを奪う瞬間自体が発生しなくなる)。マウスクリック自体
    /// (WM_LBUTTONDOWN等)は引き続き正常にこのウィンドウへ届くため、ボタンの
    /// クリック処理(JS側のclickイベント)には影響しない。オンスクリーン
    /// キーボードや配信オーバーレイツール等、「常に前面にあるがクリックしても
    /// フォーカスを奪わないツールバー」的なウィンドウでよく使われる標準的な
    /// 手法。set_foreground_window()自体は保険として残してあるが、この
    /// スタイルが効いていればそもそも出番が無くなる想定。
    pub fn set_no_activate(hwnd_raw: isize) {
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, cur | (WS_EX_NOACTIVATE.0 as isize));
        }
    }

    /// 指定ウィンドウが「今まさにフォアグラウンド(アクティブ)かどうか」を返す。
    ///
    /// 経緯: 「連打時のちらつきは直ったが、pcwmp/PCRPlayerのコメント欄に
    /// フォーカスがある状態で絵文字を押すと、そのフォーカスが戻らなくなった」
    /// という指摘への対応。以前はクリックのたびに無条件でset_foreground_window
    /// を呼んで戻していたが、それ自体がちらつきの原因だったため撤去していた。
    /// 今回は「クリック時点で対象が本当にフォアグラウンドだった場合」だけ
    /// フォーカスを戻すことで、両立を図る。
    ///
    /// 判定のタイミングについて: このアプリのリアクションバー自身は
    /// set_no_activate()によりWS_EX_NOACTIVATEが立っており、クリックされても
    /// 自分自身はアクティブ化されない。そのため、クリックハンドラの実行時点
    /// (=bar.js側からこのコマンドが呼ばれる時点)でも、GetForegroundWindow()は
    /// クリック直前から変わらず「本当に手前にあったウィンドウ」を返し続ける
    /// はずで、狙い通り「対象が元々フォーカスされていたか」を判定できる。
    pub fn is_window_foreground(hwnd_raw: isize) -> bool {
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        let fg = unsafe { GetForegroundWindow() };
        !fg.0.is_null() && fg.0 == hwnd.0
    }

    /// 「ReaTapをタスクトレイから終了したはずなのに、後で配信画面を開いたら
    /// 前回のリアクションがまだ出ていた」という報告への対策。中継サーバー・
    /// クライアントのコード上はメッセージの保存・再送は一切しておらず、原因の
    /// 心当たりとして最も有力なのは「×ボタン/終了メニューでapp.exit(0)を
    /// 呼んでプロセスを強制終了させた際、WebView2が裏で使っている子プロセスの
    /// 後始末が間に合わず、タスクトレイからもタスクバーからも見えなくなった後も
    /// 実際にはプロセスが生き残ってしまう」パターン。create_main_windowで既に
    /// close_all_bar_windows()を先に呼ぶ対策はしてあるが、それでも取りこぼしが
    /// 起き得るため、Windows Job Object(親プロセスに全ての子プロセスを
    /// 紐付けておき、親のハンドルが閉じた瞬間に子も含めて丸ごとOSに強制終了
    /// させてもらう仕組み)を使って「完膚なきまでの終了」をOS側に保証させる。
    ///
    /// JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: このJobオブジェクトの最後のハンドルが
    /// 閉じられた時(=このプロセスの終了時)、Jobに所属する全プロセスを道連れに
    /// 終了させる。
    /// JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK: 万一どこかの子プロセスが明示的に
    /// 離脱(breakaway)を要求してきた場合はエラーにせず黙って許可する
    /// (更新時のヘルパープロセスがCREATE_BREAKAWAY_FROM_JOBで離脱するのに必要。
    /// updater.rsのspawn_apply_update_helper参照)。
    ///
    /// 失敗しても致命的ではない(=このハードニングが効かないだけで、従来通りの
    /// 動作にフォールバックする)ため、戻り値は無視して構わない設計にしている。
    pub fn harden_process_termination() {
        unsafe {
            let job = match CreateJobObjectW(None, PCWSTR::null()) {
                Ok(h) if !h.is_invalid() => h,
                _ => return,
            };

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;

            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok.is_err() {
                return;
            }

            let _ = AssignProcessToJobObject(job, GetCurrentProcess());

            // 作成したJobハンドルはプロセス終了までずっと生かしておく必要がある
            // (=ここでdropして閉じてしまうと、その時点でKILL_ON_JOB_CLOSEが
            // 発動してしまう)。プロセス寿命全体で1個だけ保持すればよいため、
            // 単純にリークさせる(OSがプロセス終了時にまとめて回収する)。
            std::mem::forget(job);
        }
    }

    // MAX_PATHは将来の拡張(短いバッファへのフォールバック等)用に残しておく
    #[allow(dead_code)]
    const _MAX_PATH_HINT: u32 = MAX_PATH;
}

#[cfg(not(target_os = "windows"))]
mod win {
    use super::WindowInfo;

    pub fn is_available() -> bool {
        false
    }

    pub fn list_windows() -> Vec<WindowInfo> {
        Vec::new()
    }

    pub fn get_active_window() -> Option<WindowInfo> {
        None
    }

    #[allow(dead_code)]
    pub fn set_window_owner(_our_hwnd_raw: isize, _owner_hwnd_raw: isize) {
        // Windows以外では何もしない
    }

    #[allow(dead_code)]
    pub fn set_foreground_window(_hwnd_raw: isize) {
        // Windows以外では何もしない
    }

    #[allow(dead_code)]
    pub fn set_no_activate(_hwnd_raw: isize) {
        // Windows以外では何もしない
    }

    #[allow(dead_code)]
    pub fn is_window_foreground(_hwnd_raw: isize) -> bool {
        // Windows以外では判定できないため、常にfalse(=フォーカスを戻す動作は
        // 発生しない)を返す
        false
    }

    #[allow(dead_code)]
    pub fn harden_process_termination() {
        // Windows以外ではJob Objectの概念自体が無いため何もしない
    }
}

pub fn is_available() -> bool {
    win::is_available()
}

pub fn list_windows() -> Vec<WindowInfo> {
    win::list_windows()
}

pub fn get_active_window() -> Option<WindowInfo> {
    win::get_active_window()
}

/// 自分のウィンドウ(our_hwnd_raw)のオーナーをowner_hwnd_rawに設定する
/// (0を渡すと解除)。Windows以外では何もしない。
///
/// 呼び出し側(lib.rs)がWindows専用コード内でしか呼ばないため、Linux上での
/// ビルド(このリポジトリの開発・検証環境)では未使用警告が出るが、実害は無い。
#[allow(dead_code)]
pub fn set_window_owner(our_hwnd_raw: isize, owner_hwnd_raw: isize) {
    win::set_window_owner(our_hwnd_raw, owner_hwnd_raw);
}

/// 指定ウィンドウ(pcwmp/PCRPlayer側)をフォアグラウンドに戻す。
/// Windows以外では何もしない。
pub fn set_foreground_window(hwnd_raw: isize) {
    win::set_foreground_window(hwnd_raw);
}

/// 指定ウィンドウ(リアクションバー自身)がクリックされてもOS側のフォーカスを
/// 奪わないようにする(WS_EX_NOACTIVATE)。Windows以外では何もしない。
///
/// 呼び出し側(lib.rs)がWindows専用コード内でしか呼ばないため、Linux上での
/// ビルド(このリポジトリの開発・検証環境)では未使用警告が出るが、実害は無い
/// (set_window_owner等と同じパターン)。
#[allow(dead_code)]
pub fn set_no_activate(hwnd_raw: isize) {
    win::set_no_activate(hwnd_raw);
}

/// 指定ウィンドウが今まさにフォアグラウンドかどうかを返す。Windows以外では
/// 常にfalse。
pub fn is_window_foreground(hwnd_raw: isize) -> bool {
    win::is_window_foreground(hwnd_raw)
}

/// 起動時に一度だけ呼ぶ。プロセス終了時にWebView2の子プロセス等を含めて
/// 確実に道連れ終了させるためのWindows Job Objectのハードニング設定。
/// Windows以外では何もしない。詳細はwin::harden_process_terminationのコメント参照。
///
/// 呼び出し側(lib.rs)が#[cfg(target_os = "windows")]の中でしか呼ばないため、
/// Linux上でのビルド(このリポジトリの開発・検証環境)では未使用警告が出るが、
/// 実害は無い(set_window_owner等と同じパターン)。
#[allow(dead_code)]
pub fn harden_process_termination() {
    win::harden_process_termination();
}

pub fn detect_target_window(override_path: Option<&str>, name_filter: Option<&str>) -> Option<WindowInfo> {
    let windows = list_windows();
    find_target_window(&windows, override_path, name_filter).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(title: &str, class_name: &str, path: &str) -> WindowInfo {
        WindowInfo {
            title: title.to_string(),
            class_name: class_name.to_string(),
            path: path.to_string(),
            x: 0,
            y: 0,
            width: 480,
            height: 360,
            raw_handle: 0,
        }
    }

    fn w_sized(title: &str, class_name: &str, path: &str, width: u32, height: u32) -> WindowInfo {
        WindowInfo {
            width,
            height,
            ..w(title, class_name, path)
        }
    }

    #[test]
    fn finds_by_class_name() {
        let windows = vec![
            w("エクスプローラー", "CabinetWClass", r"C:\Windows\explorer.exe"),
            w("ちゃんねる名", "TpcwmpMain", r"C:\Tools\pcwmp\weird-renamed.exe"),
        ];
        let found = find_pcwmp_window(&windows).expect("should find");
        assert_eq!(found.class_name, "TpcwmpMain");
    }

    #[test]
    fn finds_by_path_when_class_name_unknown() {
        let windows = vec![w("x", "SomeOtherClass", r"C:\Tools\pcwmp\pcwmp.exe")];
        assert!(find_pcwmp_window(&windows).is_some());
    }

    #[test]
    fn case_insensitive_path_match() {
        let windows = vec![w("x", "Other", r"C:\Tools\PCWMP\PCWMP.EXE")];
        assert!(find_pcwmp_window(&windows).is_some());
    }

    #[test]
    fn finds_pcrplayer64_by_exe_name() {
        // 実機テストで、pcwmp.exeではなくPCRPlayer64.exeが使われているケースが
        // 確認された(2ch/Monazilla系のPeerCast互換視聴ソフト)
        let windows = vec![w(
            "PCRPlayer",
            "SomeMfcClass",
            r"C:\Users\noobo\Desktop\PCRPlayer64.exe",
        )];
        let found = find_pcwmp_window(&windows).expect("should find PCRPlayer64.exe");
        assert_eq!(found.title, "PCRPlayer");
    }

    #[test]
    fn finds_pcrplayer_32bit_by_exe_name() {
        let windows = vec![w("PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer.exe")];
        assert!(find_pcwmp_window(&windows).is_some());
    }

    #[test]
    fn does_not_false_positive_on_substring_match() {
        // ends_with方式の名残りで「superpcwmp.exe」のような無関係なファイルまで
        // 誤検出しないことを確認する(ファイル名の完全一致のみ許可する)
        let windows = vec![w("怪しいソフト", "Other", r"C:\Tools\superpcwmp.exe")];
        assert!(find_pcwmp_window(&windows).is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let windows = vec![w("メモ帳", "Notepad", r"C:\Windows\notepad.exe")];
        assert!(find_pcwmp_window(&windows).is_none());
    }

    #[test]
    fn empty_list_is_safe() {
        assert!(find_pcwmp_window(&[]).is_none());
    }

    #[test]
    fn ignores_small_popup_from_same_process_and_picks_main_window() {
        // 実機で見つかった不具合の再現: レスにカーソルを合わせた時に出る
        // スレッドのポップアップや右クリックメニューは、PCRPlayer本体と同じ
        // exeパスを持つ別の小さなウィンドウとして列挙される。EnumWindowsは
        // Z-order順(手前にあるものが先)なので、ポップアップが本体より先に
        // 列挙されるケースを再現している。
        let windows = vec![
            w_sized(
                "スレッドプレビュー",
                "TPopupWindow",
                r"C:\Tools\PCRPlayer64.exe",
                220,
                140,
            ),
            w_sized(
                "PCRPlayer",
                "SomeMfcClass",
                r"C:\Tools\PCRPlayer64.exe",
                1280,
                720,
            ),
        ];
        let found = find_pcwmp_window(&windows).expect("should find main window");
        assert_eq!(found.title, "PCRPlayer");
        assert_eq!(found.width, 1280);
    }

    #[test]
    fn ignores_context_menu_from_same_process() {
        // 右クリックメニュー(標準の#32768クラス)も同じプロセスから出るため
        // 実行ファイル名一致で候補に入ってしまうが、本体より小さいので選ばれない。
        let windows = vec![
            w_sized("PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe", 1280, 720),
            w_sized("", "#32768", r"C:\Tools\PCRPlayer64.exe", 180, 260),
        ];
        let found = find_pcwmp_window(&windows).expect("should find main window");
        assert_eq!(found.class_name, "SomeMfcClass");
    }

    #[test]
    fn override_path_also_prefers_largest_match() {
        let windows = vec![
            w_sized("ポップアップ", "TPopupWindow", r"C:\Custom\myplayer.exe", 200, 150),
            w_sized("本体", "MainClass", r"C:\Custom\myplayer.exe", 1000, 600),
        ];
        let found = find_target_window(&windows, Some(r"C:\Custom\myplayer.exe"), None).unwrap();
        assert_eq!(found.title, "本体");
    }

    #[test]
    fn override_path_takes_priority() {
        let windows = vec![
            w("カスタムプレイヤー", "Other", r"C:\Custom\myplayer.exe"),
            w("x", "TpcwmpMain", r"C:\Tools\pcwmp\pcwmp.exe"),
        ];
        let found = find_target_window(&windows, Some(r"C:\Custom\myplayer.exe"), None).unwrap();
        assert_eq!(found.path, r"C:\Custom\myplayer.exe");
    }

    #[test]
    fn override_path_falls_back_when_not_found() {
        let windows = vec![w("x", "TpcwmpMain", r"C:\Tools\pcwmp\pcwmp.exe")];
        let found = find_target_window(&windows, Some(r"C:\Custom\myplayer.exe"), None).unwrap();
        assert_eq!(found.class_name, "TpcwmpMain");
    }

    #[test]
    fn filter_candidate_windows_excludes_empty_title() {
        let windows = vec![
            w("", "SomeHelperClass", r"C:\x\helper.exe"),
            w("メモ帳", "Notepad", r"C:\Windows\notepad.exe"),
        ];
        let result = filter_candidate_windows(windows, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "メモ帳");
    }

    #[test]
    fn filter_candidate_windows_excludes_self() {
        let windows = vec![
            w("視聴者アプリ - 初期設定", "TauriWebview", r"C:\Tools\ReactionViewer.exe"),
            w("メモ帳", "Notepad", r"C:\Windows\notepad.exe"),
        ];
        let result =
            filter_candidate_windows(windows, Some(r"C:\Tools\ReactionViewer.exe"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "メモ帳");
    }

    #[test]
    fn filter_candidate_windows_self_path_is_case_insensitive() {
        let windows = vec![w("自分", "TauriWebview", r"C:\Tools\ReactionViewer.EXE")];
        let result =
            filter_candidate_windows(windows, Some(r"c:\tools\reactionviewer.exe"));
        assert!(result.is_empty());
    }

    #[test]
    fn filter_candidate_windows_keeps_all_when_no_self_path() {
        let windows = vec![w("メモ帳", "Notepad", r"C:\Windows\notepad.exe")];
        let result = filter_candidate_windows(windows, None);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn name_filter_picks_only_matching_title_among_multiple_instances() {
        // PeerCastを多重起動している(複数のPCRPlayerが同時に開いている)ケースの
        // 再現。両方とも本体サイズだが、配信者名(タイトルに含まれる)で絞り込む。
        let windows = vec![
            w_sized("あお - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe", 1280, 720),
            w_sized("べつのひと - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe", 1280, 720),
        ];
        let found = find_pcwmp_window_by_name(&windows, Some("あお")).expect("should find あお's window");
        assert_eq!(found.title, "あお - PCRPlayer");
    }

    #[test]
    fn name_filter_is_case_insensitive_substring_match() {
        let windows = vec![w("Yuki-Live Broadcast - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe")];
        assert!(find_pcwmp_window_by_name(&windows, Some("yuki-live")).is_some());
    }

    #[test]
    fn name_filter_returns_none_instead_of_falling_back_when_no_match() {
        // 名前を指定した場合、一致するウィンドウが無ければ(誤って別の配信者に
        // 飛ぶくらいなら)Noneを返す。絞り込み無しのfind_pcwmp_windowへは
        // フォールバックしない。
        let windows = vec![w("べつのひと - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe")];
        assert!(find_pcwmp_window_by_name(&windows, Some("あお")).is_none());
    }

    #[test]
    fn name_filter_empty_or_none_behaves_like_unfiltered() {
        let windows = vec![w("だれか - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe")];
        assert!(find_pcwmp_window_by_name(&windows, None).is_some());
        assert!(find_pcwmp_window_by_name(&windows, Some("   ")).is_some());
    }

    #[test]
    fn find_target_window_applies_name_filter_when_no_override_path() {
        let windows = vec![
            w_sized("あお - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe", 1280, 720),
            w_sized("べつのひと - PCRPlayer", "SomeMfcClass", r"C:\Tools\PCRPlayer64.exe", 1280, 720),
        ];
        let found = find_target_window(&windows, None, Some("べつのひと")).expect("should find");
        assert_eq!(found.title, "べつのひと - PCRPlayer");
    }
}
