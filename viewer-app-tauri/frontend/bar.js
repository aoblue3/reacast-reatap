'use strict';
/**
 * リアクションバー(1配信者分)専用のスクリプト。
 *
 * 複数配信者への同時接続に対応するため、以前main.js(メインウィンドウ)が
 * 単体で担っていた「アイコンだけを映像に重ねて表示する透明なオーバーレイ」の
 * 役割を、プロファイル(合言葉+配信者名)ごとに独立したウィンドウ
 * ("bar-{プロファイルid}"。lib.rsのcreate_bar_window参照)に切り出したもの。
 *
 * このウィンドウは常にこの1画面(アイコン列)だけを表示し、設定パネルは
 * 一切持たない。位置・表示するリアクション・ホットキー等の設定は、
 * メインウィンドウ(main.html/main.js)の配信者一覧から行い、このスクリプトは
 * 起動時と"profile-settings-changed"イベント受信時に設定を読み直すだけ。
 *
 * ウィンドウごとにJSの実行コンテキストが完全に分離されている(Tauri v2の
 * 仕様)ため、複数の配信者に同時接続してもグローバル変数が衝突する心配はない
 * (このファイル自身が「1配信者分」の状態をすべて自己完結して持つ)。
 */
const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;
const getCurrentWindow = window.__TAURI__.window.getCurrentWindow;

// 右クリックで素のWebView2/WebKitGTKの既定メニュー(再読み込み等)が出てしまうと、
// 枠なし・透明でユーザーからは「メニューが出た」と気付きにくいこの画面では、
// 誤って「再読み込み」等を選んでしまい接続状態や位置がリセットされて挙動が
// おかしくなる、というテスターからの報告があった。このウィンドウにはテキスト
// 入力など右クリックを使う要素が一切無いため、既定のコンテキストメニュー自体を
// 出さないようにする(メインウィンドウの合言葉入力欄などはコピペで右クリックを
// 使うため、そちらでは抑止していない。main.js参照)。
window.addEventListener('contextmenu', (e) => {
  e.preventDefault();
});

const barEl = document.getElementById('bar');
const reactionPanelInner = document.getElementById('reactionPanelInner');
const muteNoticeEl = document.getElementById('muteNotice');

const DEBOUNCE_MS = 500;
const POLL_INTERVAL_MS = 250;
const BAR_EXTRA_PADDING_PX = 6;
const REACTION_WINDOW_RIGHT_MAX_HEIGHT_PX = 2000;

let myLabel = '';
let profileId = '';
let profile = null; // { id, passphrase, broadcasterName, label, relayHostOverride?, relayPortOverride? }

let lastSentAt = 0;
let mutedUntil = 0;
let muteTimer = null;
let relayClient = null;
let pollTimer = null;

// プロファイル個別設定(既定値。init()で読み込んだ内容やprofile-settings-changed
// イベントで上書きされる)
let manualOverridePath = null;
let followTarget = true;
let alwaysOnTop = true;
let placementMode = 'right';
let marginPx = 60;
let rightRows = 10;
let hotkeyMap = {};
// 対象ウィンドウが見つかっていない間もバーを表示し続けるかどうか(既定OFF)。
// OFFの場合、対象が見つかるまでウィンドウそのものを隠す(setBarVisible参照)。
let manualShow = false;

// 現在のウィンドウ表示/非表示状態のキャッシュ(nullは未確定=初回)。同じ状態への
// show()/hide()呼び出しを避けるための記録(cfg_set連発と同じ理由で、無駄な
// IPC呼び出し自体を減らす)。
let currentVisibility = null;

// リアクションの表示/並び順は全プロファイル共通の設定(main.js参照)。
let reactionOrder = null;
let hiddenReactionIds = [];

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function getEffectiveOrder() {
  const allIds = window.EmojiSet.EMOJI_SET.map((e) => e.id);
  const saved = Array.isArray(reactionOrder) ? reactionOrder : [];
  const known = saved.filter((id) => window.EmojiSet.isValidEmojiId(id));
  const missing = allIds.filter((id) => !known.includes(id));
  return [...known, ...missing];
}

function getVisibleOrderedEmojiSet() {
  return getEffectiveOrder()
    .filter((id) => !hiddenReactionIds.includes(id))
    .map((id) => window.EmojiSet.EMOJI_BY_ID.get(id))
    .filter(Boolean);
}

function formatHotkeyLabel(shortcut) {
  if (!shortcut) return '';
  return shortcut
    .split('+')
    .map((tok) => {
      if (tok === 'ArrowUp') return '↑';
      if (tok === 'ArrowDown') return '↓';
      if (tok === 'ArrowLeft') return '←';
      if (tok === 'ArrowRight') return '→';
      return tok.replace(/^Key/, '').replace(/^Digit/, '');
    })
    .join('+');
}

// data-tauri-drag-region属性(bar.html)によるウィンドウのドラッグ移動が、
// 以前ここでresizable(false)を指定していた際にはWindows上で効かなくなる
// (=表示位置を変更できなくなる)不具合が実際にあった(lib.rsのcreate_bar_window
// 参照。今はresizable(false)自体を外して直してある)。念のための保険として、
// data-tauri-drag-regionだけに頼らず、JS側からも明示的にウィンドウの移動を
// 開始する(Tauriの公開APIであるstartDragging()は、data-tauri-drag-region自身も
// 内部で使っている同じ仕組み)。ボタン(.emoji-btn)の上でのクリックは対象外にする
// (リアクション送信の邪魔をしないため)。
document.getElementById('reactionPanel').addEventListener('mousedown', (e) => {
  if (e.button !== 0) return;
  if (e.target.closest('.emoji-btn')) return;
  getCurrentWindow()
    .startDragging()
    .catch(() => {
      // 無視(data-tauri-drag-region側が既に処理できていれば問題ない)
    });
});

// 「アイコンの大きさ」「アイコンの背景の濃さ」(main.jsの「リアクションバーの
// 見た目」設定・全プロファイル共通)を、CSSカスタムプロパティ経由で反映する。
// bar.html側の.emoji-btnがこれらの変数を参照している。
function applyBarAppearance(scale, bgAlpha) {
  const s = Number.isFinite(scale) && scale > 0 ? Math.min(2, Math.max(0.5, scale)) : 1;
  const a = Number.isFinite(bgAlpha) ? Math.min(1, Math.max(0, bgAlpha)) : 0.55;
  document.documentElement.style.setProperty('--icon-scale', String(s));
  document.documentElement.style.setProperty('--icon-bg-alpha', String(a));
}

// 直近の検出で見つかった対象(pcwmp/PCRPlayer)のHWND。ボタンクリック後に
// フォーカスを戻すために使う(onClickEmoji参照)。0は「対象なし」。
let lastTargetRawHandle = 0;

function onClickEmoji(emojiId) {
  const now = Date.now();
  if (now < mutedUntil) return;
  if (now - lastSentAt < DEBOUNCE_MS) return;
  lastSentAt = now;
  if (relayClient) relayClient.send({ type: 'reaction', emoji: emojiId });
  // ボタンをクリックすると、このバー自身はfocused(false)で作っていても
  // クリックそのものでOSからキーボードフォーカスを奪ってしまい、pcwmp/
  // PCRPlayerのコメント入力欄からフォーカスが外れてしまう、というテスター
  // 報告への対応。直近に検出できていた対象があれば、クリック直後にそちらへ
  // フォーカスを戻す(対象が無い/まだ検出前の場合は何もしない)。
  if (lastTargetRawHandle) {
    invoke('focus_target_window', { rawHandle: lastTargetRawHandle }).catch(() => {
      // 無視(フォーカスの戻し忘れ程度で、リアクション自体は既に送信済み)
    });
  }
}

function renderButtons() {
  const emojiSet = getVisibleOrderedEmojiSet();
  barEl.innerHTML = '';
  for (const emoji of emojiSet) {
    const btn = document.createElement('button');
    btn.className = 'emoji-btn';
    btn.textContent = emoji.char;
    btn.dataset.emojiId = emoji.id;
    btn.addEventListener('click', () => onClickEmoji(emoji.id));
    const hotkey = hotkeyMap[emoji.id];
    btn.title = hotkey ? `${emoji.label}(${formatHotkeyLabel(hotkey)})` : emoji.label;
    if (hotkey) {
      const badge = document.createElement('span');
      badge.className = 'hk-badge';
      badge.textContent = formatHotkeyLabel(hotkey);
      btn.appendChild(badge);
    }
    barEl.appendChild(btn);
  }
}

function applyMuteUI() {
  const buttons = barEl.querySelectorAll('.emoji-btn');
  const remainingMs = mutedUntil - Date.now();
  if (remainingMs <= 0) return;
  buttons.forEach((b) => b.classList.add('muted'));
  muteNoticeEl.classList.add('visible');
  updateMuteNoticeText();
  clearTimeout(muteTimer);
  muteTimer = setTimeout(() => {
    buttons.forEach((b) => b.classList.remove('muted'));
    muteNoticeEl.classList.remove('visible');
  }, remainingMs);
  const tick = setInterval(() => {
    if (Date.now() >= mutedUntil) {
      clearInterval(tick);
      return;
    }
    updateMuteNoticeText();
  }, 1000);
}

function updateMuteNoticeText() {
  const remainingSec = Math.max(0, Math.ceil((mutedUntil - Date.now()) / 1000));
  muteNoticeEl.textContent = `連打を検知しました。あと${remainingSec}秒送信できません`;
}

async function resolveRelayAddress() {
  const built = await invoke('get_relay_address');
  const hostOverride = await invoke('cfg_get', { key: 'relayHostOverride' });
  const portOverride = await invoke('cfg_get', { key: 'relayPortOverride' });
  return {
    relayHost: typeof hostOverride === 'string' && hostOverride ? hostOverride : built.host,
    relayPort:
      typeof portOverride === 'number' && Number.isFinite(portOverride) ? portOverride : built.port,
  };
}

function connectRelay(relayHost, relayPort) {
  if (relayClient) {
    relayClient.close();
    relayClient = null;
  }
  relayClient = new window.RelayClient({
    url: `ws://${relayHost}:${relayPort}`,
    hello: { type: 'join', passphrase: profile.passphrase },
  });
  relayClient.on('type:muted', (m) => {
    mutedUntil = m.untilMs;
    applyMuteUI();
  });
  relayClient.on('open', () => {
    document.body.style.opacity = '1';
  });
  relayClient.on('close', () => {
    document.body.style.opacity = '0.6';
  });
  relayClient.connect();
}

// 重要: このウィンドウ自身はOS側のグローバルショートカット登録
// (gs.register/unregisterAll)を一切行わない。tauri-plugin-global-shortcutは
// 登録済みショートカットをアプリ全体で1つのマップとして管理しているため、
// バーごとに自分のホットキーだけを登録・解除しているつもりでも、
// unregisterAll()は他プロファイルの分まで巻き添えで消してしまう
// (実機検証ではなく、プラグイン本体のソースコード確認で判明)。そのため、
// ホットキーの実際のOS登録はメインウィンドウ(main.js)に一本化してあり、
// メインウィンドウで押されたホットキーは"hotkey-reaction"イベント経由で
// このウィンドウに転送されてくる(lib.rsのsend_hotkey_reaction参照)。
// ここではそれを受けてonClickEmoji()を呼ぶだけでよい。
// hotkeyMap自体はボタン下のバッジ表示(どのキーが割り当てられているか)の
// ためだけに読み込む(reloadProfileSettings参照)。
listen('hotkey-reaction', (event) => {
  onClickEmoji(event.payload);
});

function computePlacement(target, screen, barSize) {
  const width = barSize.width;
  const height = barSize.height;
  let x, y;

  if (placementMode === 'right') {
    x = Math.round(target.x) + Math.round(target.width);
    y = Math.round(target.y);
  } else if (placementMode === 'top') {
    x = Math.round(target.x) + Math.round((target.width - width) / 2);
    y = Math.round(target.y) - height;
  } else if (placementMode === 'overlap-bottom') {
    x = Math.round(target.x) + Math.round((target.width - width) / 2);
    y = Math.round(target.y) + Math.round(target.height) - height;
  } else if (placementMode === 'bottom') {
    x = Math.round(target.x) + Math.round((target.width - width) / 2);
    y = Math.round(target.y) + Math.round(target.height);
  } else {
    // 'comment'(既定)
    x = Math.round(target.x) + Math.round((target.width - width) / 2);
    y = Math.round(target.y) + Math.round(target.height) - height - marginPx;
  }

  const w = Math.min(width, screen.width);
  const h = Math.min(height, screen.height);
  x = clamp(x, screen.x, screen.x + screen.width - w);
  y = clamp(y, screen.y, screen.y + screen.height - h);

  return { x, y, width: w, height: h };
}

function applyRightModeColumnLayout() {
  const reactionPanel = document.getElementById('reactionPanel');
  const isRight = placementMode === 'right';
  reactionPanel.classList.toggle('placement-right', isRight);
  if (!isRight) {
    barEl.style.gridTemplateRows = '';
    return;
  }
  const itemsPerColumn = Math.max(1, Math.round(rightRows));
  barEl.style.gridTemplateRows = `repeat(${itemsPerColumn}, auto)`;
}

function measureBarSize() {
  const rect = reactionPanelInner.getBoundingClientRect();
  return {
    width: Math.ceil(rect.width) + BAR_EXTRA_PADDING_PX,
    height: Math.ceil(rect.height) + BAR_EXTRA_PADDING_PX,
  };
}

async function positionOverWindow(target) {
  let screen;
  try {
    screen = await invoke('get_screen_bounds', { label: myLabel });
  } catch {
    screen = { x: 0, y: 0, width: 100000, height: 100000 };
  }
  applyRightModeColumnLayout();
  const barSize = measureBarSize();
  if (placementMode === 'right') {
    barSize.height = Math.min(barSize.height, REACTION_WINDOW_RIGHT_MAX_HEIGHT_PX);
  }
  const { x, y, width, height } = computePlacement(target, screen, barSize);
  await invoke('position_bar_window', { x, y, width, height, label: myLabel });
}

async function shrinkToIconSize() {
  applyRightModeColumnLayout();
  const barSize = measureBarSize();
  if (placementMode === 'right') {
    barSize.height = Math.min(barSize.height, REACTION_WINDOW_RIGHT_MAX_HEIGHT_PX);
  }
  try {
    await invoke('resize_window', { width: barSize.width, height: barSize.height, label: myLabel });
  } catch {
    // 無視(次のタイミングで再試行される)
  }
}

// reportStatus()の直近の書き込み内容(変化が無ければ書き込みを省略するための
// 記録用。lastReportedStatus参照)。
let lastReportedStatus = null;

/** 検出できているかどうかを、メインウィンドウの配信者一覧が見に行けるよう
 * 設定ストアに書いておく(プロファイルIDでスコープしたキー)。バー自体には
 * ステータス文字は一切表示しない(「リアクション表示中はアイコン以外何も
 * 描画しない」という以前からの方針を踏襲)。
 *
 * 重要: detectAndFollowOnce()はPOLL_INTERVAL_MS(250ms)おきに呼ばれるため、
 * 以前はこの関数も250msに1回、無条件にcfg_set(=設定ファイル全体をディスクに
 * 同期書き込み)していた。cfg_setはメインスレッドをブロックする処理だったため
 * (lib.rs参照。async化済み)、接続中の配信者が複数いると書き込みが積み重なり、
 * 詳細設定を開く・削除するといった他の操作やウィンドウを閉じる処理までもが
 * 長時間待たされる不具合の一因になっていた。async化した今でも、見つかって
 * いる/いないの状態が変わっていない限りは書き込む意味が無いため、実際に
 * 変化した時だけ書き込むようにして無駄なディスクI/Oそのものを減らしておく
 * (見た目の挙動は変わらない。found/titleが変わった瞬間だけ一覧側の表示が
 * 更新されれば十分なため)。 */
async function reportStatus(found, title) {
  const next = { found: !!found, title: title || '' };
  if (
    lastReportedStatus &&
    lastReportedStatus.found === next.found &&
    lastReportedStatus.title === next.title
  ) {
    return;
  }
  lastReportedStatus = next;
  try {
    await invoke('cfg_set', {
      key: `profile:${profileId}:runtimeStatus`,
      value: { ...next, updatedAtMs: Date.now() },
    });
  } catch {
    // 無視(表示上の付加情報に過ぎないため失敗しても致命的ではない)
  }
}

/** ウィンドウの表示/非表示を切り替える。同じ状態への呼び出しは省略する
 * (show()/hide()自体もTauriのIPC呼び出しであり、250msおきのポーリングから
 * 毎回無条件に呼ぶと無駄なため。reportStatus()と同じ考え方)。 */
async function setBarVisible(visible) {
  if (currentVisibility === visible) return;
  currentVisibility = visible;
  try {
    if (visible) {
      await getCurrentWindow().show();
    } else {
      await getCurrentWindow().hide();
    }
  } catch {
    // 無視(次のタイミングで状態が変わればまた呼ばれる)
  }
}

// 配信者名を指定せずに接続している場合、対象ウィンドウの絞り込みが緩い
// (系内のpcwmp/PCRPlayerクラスの窓を無条件に対象にする)ぶん、Windows側の
// 描画タイミング(DWMの再合成中など)によって、ある1回のポーリング(250ms)
// だけ一時的に検出に失敗することがある、というテスター報告があった
// (「ボタン表示」で常時表示に切り替えると症状が止まる=hide()が呼ばれなく
// なるだけで直る、という報告から、検出のチラつき自体が原因と判断した)。
// 単発の検出漏れのたびにウィンドウをhide()/show()し直すとバーがチカチカ
// 点滅して見えてしまうため、「見つかった」は即座に反映する一方、「見つから
// なくなった」は既定では数回連続で検出に失敗した場合だけ実際に隠すように
// し、単発のブレでは直前の表示状態を維持する(ヒステリシス)。
const HIDE_AFTER_CONSECUTIVE_MISSES = 3; // 250ms x 3 = 750ms連続で検出できない場合のみ隠す
let consecutiveMisses = 0;

async function detectAndFollowOnce() {
  const target = await invoke('detect_target_window', {
    overridePath: manualOverridePath,
    broadcasterName: profile.broadcasterName || null,
    windowLabel: myLabel,
  });
  if (target && target.width > 0) {
    consecutiveMisses = 0;
    lastTargetRawHandle = target.raw_handle || 0;
    await reportStatus(true, target.title);
    if (followTarget) {
      await positionOverWindow(target);
    }
    await setBarVisible(true);
  } else {
    consecutiveMisses += 1;
    if (consecutiveMisses >= HIDE_AFTER_CONSECUTIVE_MISSES) {
      // 対象を見失って久しい場合はraw_handleも捨てる。HWNDはOSに再利用される
      // ことがあるため、古い値を持ち続けると閉じた後に別の(無関係な)
      // ウィンドウへフォーカスを戻してしまう恐れがある(onClickEmoji参照)。
      lastTargetRawHandle = 0;
    }
    await reportStatus(false, '');
    // 対象がまだ見つかっていない間、以前は常にアイコンサイズへ縮めて表示し
    // 続けていたが、「対象が無い状態で画面のどこかにぽつんと表示され続けて
    // 邪魔」という報告を受け、既定では対象が見つかるまでウィンドウ自体を
    // 隠すようにした。一覧側の「表示する」トグル(main.js)でmanualShowを
    // ONにした場合のみ、対象なしでもアイコン列を表示し続ける
    // (自由な位置にドラッグして単独運用したい場合向け)。
    if (manualShow) {
      await shrinkToIconSize();
      await setBarVisible(true);
    } else if (consecutiveMisses >= HIDE_AFTER_CONSECUTIVE_MISSES) {
      await setBarVisible(false);
    }
  }
}

function startPolling() {
  if (pollTimer) clearInterval(pollTimer);
  pollTimer = setInterval(() => {
    detectAndFollowOnce();
  }, POLL_INTERVAL_MS);
}

/** メインウィンドウでこのプロファイルの設定が変更された時に呼ばれる。
 * 接続(合言葉・relayClient)自体はやり直さず、位置・表示・ホットキー関連の
 * 設定だけをconfigストアから読み直して即座に反映する。 */
async function reloadProfileSettings() {
  const [
    savedManualOverride,
    savedFollow,
    savedTop,
    savedPlacement,
    savedMargin,
    savedRightRows,
    savedHotkeys,
    savedOrder,
    savedHidden,
    savedManualShow,
    savedIconScale,
    savedIconBgAlpha,
  ] = await Promise.all([
    invoke('cfg_get', { key: `profile:${profileId}:manualOverridePath` }),
    invoke('cfg_get', { key: `profile:${profileId}:followTarget` }),
    invoke('cfg_get', { key: `profile:${profileId}:alwaysOnTop` }),
    invoke('cfg_get', { key: `profile:${profileId}:placementMode` }),
    invoke('cfg_get', { key: `profile:${profileId}:commentMargin` }),
    invoke('cfg_get', { key: `profile:${profileId}:placementRightRows` }),
    invoke('cfg_get', { key: `profile:${profileId}:hotkeys` }),
    invoke('cfg_get', { key: 'reactionOrder' }),
    invoke('cfg_get', { key: 'hiddenReactionIds' }),
    invoke('cfg_get', { key: `profile:${profileId}:manualShow` }),
    invoke('cfg_get', { key: 'barIconScale' }),
    invoke('cfg_get', { key: 'barIconBgAlpha' }),
  ]);

  manualOverridePath = typeof savedManualOverride === 'string' ? savedManualOverride : null;
  followTarget = typeof savedFollow === 'boolean' ? savedFollow : true;
  alwaysOnTop = typeof savedTop === 'boolean' ? savedTop : true;
  placementMode = typeof savedPlacement === 'string' ? savedPlacement : 'right';
  marginPx =
    typeof savedMargin === 'number' && Number.isFinite(savedMargin) ? clamp(savedMargin, 0, 500) : 60;
  rightRows =
    typeof savedRightRows === 'number' && Number.isFinite(savedRightRows)
      ? clamp(Math.round(savedRightRows), 1, 40)
      : 10;
  hotkeyMap = savedHotkeys && typeof savedHotkeys === 'object' ? savedHotkeys : {};
  reactionOrder = Array.isArray(savedOrder) ? savedOrder : null;
  hiddenReactionIds = Array.isArray(savedHidden) ? savedHidden : [];
  manualShow = savedManualShow === true;
  applyBarAppearance(
    typeof savedIconScale === 'number' ? savedIconScale : 1,
    typeof savedIconBgAlpha === 'number' ? savedIconBgAlpha : 0.55
  );

  // 配信者名・表示名がメインウィンドウ側で編集されている可能性があるため、
  // プロファイル本体の情報も読み直す(合言葉は接続済みの場合変更不可の扱いな
  // ので、passphraseだけは初回接続時の値のまま気にしなくてよい)。
  const profiles = (await invoke('cfg_get', { key: 'broadcasterProfiles' })) || [];
  const updated = Array.isArray(profiles) ? profiles.find((p) => p.id === profileId) : null;
  if (updated) profile = updated;

  await invoke('set_always_on_top', { enabled: alwaysOnTop, label: myLabel });
  renderButtons();
  await detectAndFollowOnce();
}

listen('profile-settings-changed', () => {
  reloadProfileSettings();
});

async function init() {
  myLabel = getCurrentWindow().label;
  profileId = myLabel.replace(/^bar-/, '');

  const profiles = (await invoke('cfg_get', { key: 'broadcasterProfiles' })) || [];
  profile = Array.isArray(profiles) ? profiles.find((p) => p.id === profileId) : null;

  if (!profile || !profile.passphrase) {
    // 何らかの理由でプロファイルが見つからない(一覧側で削除された直後の
    // 取りこぼし等)。何も表示せずウィンドウを閉じる。
    console.error('プロファイルが見つかりません:', profileId);
    try {
      await getCurrentWindow().close();
    } catch {
      // 無視
    }
    return;
  }

  await reloadProfileSettings();
  const relayAddress = await resolveRelayAddress();
  connectRelay(relayAddress.relayHost, relayAddress.relayPort);
  startPolling();
}

init();
