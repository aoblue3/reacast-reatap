'use strict';
/**
 * 配信者一覧・設定画面(メインウィンドウ)のスクリプト。
 *
 * 以前はこのウィンドウ自身が「設定パネル/リアクションパネル」を動的に
 * 切り替える構成だったが、複数配信者への同時接続に対応するため、
 * リアクションを実際に表示するオーバーレイは配信者(プロファイル)ごとに
 * 独立したウィンドウ("bar-{プロファイルid}"。bar.html/bar.js)に切り出した。
 * このウィンドウは常に「配信者の追加・一覧・各種設定」専用の、枠あり・
 * 不透明な通常ウィンドウとして使う(以前set_overlay_modeで行っていた透明化の
 * 切り替えは廃止)。
 *
 * データモデル:
 *   - broadcasterProfiles (config): [{ id, passphrase, broadcasterName, label }]
 *     配信者(合言葉+任意の配信者名)のプロファイル一覧。
 *   - activeProfileIds (config): 現在「接続する」状態にしたプロファイルidの配列。
 *     アプリ起動時、この一覧に含まれるプロファイルのバーを自動的に開き直す
 *     (以前のconnectInfoによる自動再接続に相当)。
 *   - profile:{id}:xxx (config): プロファイルごとの個別設定(表示位置・余白・
 *     行数・手動選択パス・追従/最前面・ホットキー)。
 *   - reactionOrder / hiddenReactionIds (config): 表示するリアクション・並び順。
 *     全プロファイル共通(バーの見た目を配信者間で揃えるため)。
 *   - relayHostOverride / relayPortOverride (config): 中継サーバーの上書き設定。
 *     これも全プロファイル共通(同じ中継サーバーに全員つなぐ運用を想定)。
 */
const invoke = window.__TAURI__.core.invoke;

// このウィンドウには合言葉入力欄など右クリックでのコピペを使う要素があるが、
// 実際に使う場面はほぼ無く、素のWebView2/WebKitGTKの既定コンテキストメニュー
// (再読み込み等)が誤って選ばれて挙動がおかしくなる方が実害として大きいとの
// 判断で、こちらでも右クリックメニュー自体を無効化する(コピー/貼り付けは
// Ctrl+C/Ctrl+Vのショートカットキーで引き続き行える。bar.js側は元から無効化済み)。
window.addEventListener('contextmenu', (e) => {
  e.preventDefault();
});

const addProfileForm = document.getElementById('addProfileForm');
const passphraseInput = document.getElementById('passphraseInput');
const broadcasterNameInput = document.getElementById('broadcasterNameInput');
const displayLabelInput = document.getElementById('displayLabelInput');
const relayHostInput = document.getElementById('relayHostInput');
const relayPortInput = document.getElementById('relayPortInput');
const submitBtn = document.getElementById('submitBtn');
const errorMsg = document.getElementById('errorMsg');
const connStatus = document.getElementById('connStatus');
const profileListEl = document.getElementById('profileListEl');
const emptyProfilesMsg = document.getElementById('emptyProfilesMsg');
const defaultSettingsPanelEl = document.getElementById('defaultSettingsPanelEl');
const defaultSettingsDetails = document.getElementById('defaultSettingsDetails');
const reactionOrderListEl = document.getElementById('reactionOrderList');

// デフォルト設定ひな形の<details>が開かれた瞬間に一度だけ中身を構築する
// (renderProfileListImpl側は「開いている間だけ」再構築する省エネ仕様のため、
// 閉→開の遷移自体はここで拾って明示的に再描画をキックする必要がある)。
if (defaultSettingsDetails) {
  defaultSettingsDetails.addEventListener('toggle', () => {
    if (defaultSettingsDetails.open) renderProfileList();
  });
}

// 中継サーバーが要求する合言葉の形式(relay-server/server.jsのPASSPHRASE_REと
// 必ず同じにしておくこと)。
const PASSPHRASE_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{2,31}$/;
const PLACEMENT_OPTIONS = [
  ['right', '対象の右(縦並び・既定)'],
  ['comment', 'コメント欄の上(重ねて表示)'],
  ['bottom', '対象の下(重ねない)'],
  ['top', '対象の上(重ねない)'],
  ['overlap-bottom', '対象の下端に重ねて表示(旧仕様)'],
];
const STATUS_POLL_MS = 1000;

const confirmOverlay = document.getElementById('confirmOverlay');
const confirmMessageEl = document.getElementById('confirmMessage');
const confirmOkBtn = document.getElementById('confirmOkBtn');
const confirmCancelBtn = document.getElementById('confirmCancelBtn');
const updateStatusOverlay = document.getElementById('updateStatusOverlay');
const updateStatusMessageEl = document.getElementById('updateStatusMessage');

/** window.confirm()の代わりに使う、自前のページ内モーダル。
 *
 * 重要: window.confirm()はWebView2/WebKitGTK側のネイティブなモーダル
 * ダイアログとして実装されており、環境によっては(特にこのウィンドウの
 * ように.focused(false)・.always_on_top(true)で作っているウィンドウでは)
 * ダイアログが表に出てこず、それでいてJS実行そのものは(GTK/WebView2の
 * イベントループが式ネストして)ブロックされたままになる、という報告が
 * あった。見た目には「ボタンを押しても何も起きない」「その後ウィンドウ全体が
 * 固まって見える」ように映り、通常終了もできなくなる(タスクトレイの「終了」も
 * 効かなくなる)ことがある。同じ問題を避けるため、確認ダイアログは常に
 * この関数(=ページ内の普通のdiv要素)を使う。 */
function showConfirmDialog(message) {
  return new Promise((resolve) => {
    confirmMessageEl.textContent = message;
    confirmOverlay.classList.add('visible');
    let settled = false;
    const cleanup = () => {
      confirmOverlay.classList.remove('visible');
      confirmOkBtn.removeEventListener('click', onOk);
      confirmCancelBtn.removeEventListener('click', onCancel);
      confirmOverlay.removeEventListener('click', onOverlayClick);
      document.removeEventListener('keydown', onKeydown, true);
    };
    const finish = (result) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(result);
    };
    const onOk = () => finish(true);
    const onCancel = () => finish(false);
    const onOverlayClick = (e) => {
      if (e.target === confirmOverlay) finish(false);
    };
    const onKeydown = (e) => {
      if (e.code === 'Escape') finish(false);
      if (e.code === 'Enter') finish(true);
    };
    confirmOkBtn.addEventListener('click', onOk);
    confirmCancelBtn.addEventListener('click', onCancel);
    confirmOverlay.addEventListener('click', onOverlayClick);
    document.addEventListener('keydown', onKeydown, true);
  });
}

let profiles = [];
let activeProfileIds = [];
let reactionOrder = null;
let hiddenReactionIds = [];
let expandedProfileId = null;
let statusPollTimer = null;

// 「新規追加時の詳細設定ひな形(デフォルト値)」用の疑似プロファイルID。
// getProfileSetting/setProfileSettingは`profile:${id}:${key}`というconfigキーに
// 読み書きするだけの汎用実装なので、実在のプロファイル(genProfileId()が生成する
// "p"始まりのID)と衝突しないこの固定文字列を使い回せば、専用のストレージ機構を
// 新設せずに済む。activeProfileIds/profilesのどちらにも含まれないため、
// ホットキーがOS側に二重登録されたり(applyAllHotkeysはactiveProfileIdsのみ走査)、
// 一覧描画のループ(profilesのみ走査)に紛れ込んだりする心配もない。
const DEFAULT_SETTINGS_ID = '__default__';

// ホットキー捕捉(キー入力待ち)の状態。メインウィンドウは1つしか無いので、
// どのプロファイルのどのリアクションを捕捉中かをここに1組だけ持てば十分。
let hotkeyCapturing = null; // { profileId, emojiId } | null
let hotkeyCaptureTimeoutId = null;
const HOTKEY_CAPTURE_TIMEOUT_MS = 8000;

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function genProfileId() {
  return `p${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

/* ------------------------- リアクション表示の並び順(全プロファイル共通) ------------------------- */

function getEffectiveOrder() {
  const allIds = window.EmojiSet.EMOJI_SET.map((e) => e.id);
  const saved = Array.isArray(reactionOrder) ? reactionOrder : [];
  const known = saved.filter((id) => window.EmojiSet.isValidEmojiId(id));
  const missing = allIds.filter((id) => !known.includes(id));
  return [...known, ...missing];
}

async function persistReactionOrder() {
  await invoke('cfg_set', { key: 'reactionOrder', value: reactionOrder });
}
async function persistHiddenReactionIds() {
  await invoke('cfg_set', { key: 'hiddenReactionIds', value: hiddenReactionIds });
}

function renderReactionOrderList() {
  const order = getEffectiveOrder();
  reactionOrderListEl.innerHTML = '';
  order.forEach((id, index) => {
    const emoji = window.EmojiSet.EMOJI_BY_ID.get(id);
    if (!emoji) return;
    const hidden = hiddenReactionIds.includes(id);
    const row = document.createElement('div');
    row.className = 'ro-row' + (hidden ? ' ro-hidden' : '');

    const visLabel = document.createElement('label');
    visLabel.style.margin = '0';
    const visCheckbox = document.createElement('input');
    visCheckbox.type = 'checkbox';
    visCheckbox.checked = !hidden;
    visCheckbox.addEventListener('change', async () => {
      if (visCheckbox.checked) {
        hiddenReactionIds = hiddenReactionIds.filter((x) => x !== id);
      } else if (!hiddenReactionIds.includes(id)) {
        hiddenReactionIds.push(id);
      }
      await persistHiddenReactionIds();
      renderReactionOrderList();
    });
    visLabel.appendChild(visCheckbox);
    row.appendChild(visLabel);

    const emojiSpan = document.createElement('span');
    emojiSpan.className = 'ro-emoji';
    emojiSpan.textContent = emoji.char;
    row.appendChild(emojiSpan);

    const labelSpan = document.createElement('span');
    labelSpan.className = 'ro-label';
    labelSpan.textContent = emoji.label;
    row.appendChild(labelSpan);

    const upBtn = document.createElement('button');
    upBtn.className = 'secondary ro-updown';
    upBtn.textContent = '↑';
    upBtn.disabled = index === 0;
    upBtn.addEventListener('click', async () => {
      const arr = getEffectiveOrder();
      if (index <= 0) return;
      [arr[index - 1], arr[index]] = [arr[index], arr[index - 1]];
      reactionOrder = arr;
      await persistReactionOrder();
      renderReactionOrderList();
    });
    row.appendChild(upBtn);

    const downBtn = document.createElement('button');
    downBtn.className = 'secondary ro-updown';
    downBtn.textContent = '↓';
    downBtn.disabled = index === order.length - 1;
    downBtn.addEventListener('click', async () => {
      const arr = getEffectiveOrder();
      if (index >= arr.length - 1) return;
      [arr[index + 1], arr[index]] = [arr[index], arr[index + 1]];
      reactionOrder = arr;
      await persistReactionOrder();
      renderReactionOrderList();
    });
    row.appendChild(downBtn);

    reactionOrderListEl.appendChild(row);
  });
}

/* ------------------------- 接続確認・追加 ------------------------- */

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

function testConnection(connectInfo) {
  return new Promise((resolve) => {
    let settled = false;
    const client = new window.RelayClient({
      url: `ws://${connectInfo.relayHost}:${connectInfo.relayPort}`,
      hello: { type: 'join', passphrase: connectInfo.passphrase },
    });
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.close();
      resolve(result);
    };
    client.on('type:joined', () => finish({ ok: true }));
    client.on('type:error', (m) =>
      finish({ ok: false, message: m.message || m.code || '不明なエラーが返されました' })
    );
    client.on('error', () =>
      finish({ ok: false, message: '中継サーバーに接続できません(アドレスや起動状況を確認してください)' })
    );
    const timer = setTimeout(
      () => finish({ ok: false, message: 'タイムアウトしました(中継サーバーが応答しません)' }),
      6000
    );
    client.connect();
  });
}

async function persistProfiles() {
  await invoke('cfg_set', { key: 'broadcasterProfiles', value: profiles });
}
async function persistActiveProfileIds() {
  await invoke('cfg_set', { key: 'activeProfileIds', value: activeProfileIds });
}

// <form>にしたことで、合言葉欄でEnterキーを押すだけでも送信できるようにした
// (以前はsubmitBtnのclickしか拾っておらず、入力してすぐEnterを押すいつもの
// 操作では何も起きないように見える、という報告があったため)。
addProfileForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  errorMsg.textContent = '';
  connStatus.textContent = '';
  const passphrase = passphraseInput.value.trim();
  const broadcasterName = broadcasterNameInput.value.trim();
  const displayLabel = displayLabelInput.value.trim();

  if (!passphrase) {
    errorMsg.textContent = '合言葉を入力してください';
    return;
  }
  if (!PASSPHRASE_RE.test(passphrase)) {
    errorMsg.textContent = '合言葉の形式が正しくありません(配信者から聞いた文字列をそのまま入力してください)';
    return;
  }
  if (profiles.some((p) => p.passphrase === passphrase)) {
    errorMsg.textContent = 'その合言葉は既に登録済みです';
    return;
  }

  submitBtn.disabled = true;
  try {
    const hostOverride = relayHostInput.value.trim();
    const portOverrideRaw = relayPortInput.value.trim();
    await invoke('cfg_set', { key: 'relayHostOverride', value: hostOverride || null });
    const portOverride = portOverrideRaw ? parseInt(portOverrideRaw, 10) : null;
    await invoke('cfg_set', {
      key: 'relayPortOverride',
      value: Number.isFinite(portOverride) ? portOverride : null,
    });

    const relayAddress = await resolveRelayAddress();

    connStatus.style.color = '#8ac6ff';
    connStatus.textContent = '配信者への接続を確認しています...';

    const result = await testConnection({ ...relayAddress, passphrase });
    if (!result.ok) {
      connStatus.style.color = '#ff8a8a';
      connStatus.textContent = `接続を確認できませんでした: ${result.message}`;
      submitBtn.disabled = false;
      return;
    }

    connStatus.style.color = '#8effa0';
    connStatus.textContent = '配信者への接続を確認しました。一覧に追加しました。';

    const id = genProfileId();
    const profile = {
      id,
      passphrase,
      broadcasterName: broadcasterName || '',
      label: displayLabel || broadcasterName || passphrase,
    };
    profiles.push(profile);
    await persistProfiles();
    // デフォルト設定ひな形(表示位置・行数・追従・最前面・ホットキー等)を
    // 新規プロファイルへコピーする。個別に詳細設定を開いて一から設定する
    // 手間を省くための機能(ホットキーは他の接続中配信者と衝突しないものだけ
    // コピーされる。詳細はapplyDefaultSettingsToNewProfile()のコメント参照)。
    await applyDefaultSettingsToNewProfile(id);
    renderProfileList();

    passphraseInput.value = '';
    broadcasterNameInput.value = '';
    displayLabelInput.value = '';
    submitBtn.disabled = false;
    connStatus.textContent = '';

    // 追加したその場ですぐ接続する(「追加」と「接続する」を毎回別々に押す
    // 手間を省くため)。
    await connectProfile(id);
  } catch (err) {
    errorMsg.textContent = `接続に失敗しました: ${err.message || err}`;
    submitBtn.disabled = false;
  }
});

/* ------------------------- 接続する/切断する ------------------------- */

async function connectProfile(id) {
  await invoke('open_bar_window', { label: `bar-${id}` });
  if (!activeProfileIds.includes(id)) {
    activeProfileIds.push(id);
    await persistActiveProfileIds();
  }
  // ホットキーは「接続する」状態のプロファイル分だけをOS側に登録する設計
  // (applyAllHotkeys参照)なので、接続状態が変わるたびに登録し直す。
  await applyAllHotkeys();
  renderProfileList();
}

async function disconnectProfile(id) {
  await invoke('close_bar_window', { label: `bar-${id}` });
  activeProfileIds = activeProfileIds.filter((x) => x !== id);
  await persistActiveProfileIds();
  await applyAllHotkeys();
  renderProfileList();
}

/** 「対象が見つかるまでバーを隠す」既定動作の手動上書き。ONにすると、対象が
 * 見つかっていなくてもそのバーを常時表示する(自由な位置にドラッグして単独で
 * 使いたい場合向け)。バーが実際に開いていれば即座に反映する。 */
async function toggleManualShow(id) {
  const current = await getProfileSetting(id, 'manualShow', false);
  await setProfileSetting(id, 'manualShow', !current);
  await notifyIfActive(id);
  renderProfileList();
}

async function deleteProfile(id) {
  const ok = await showConfirmDialog('この配信者をリストから削除しますか?(接続中の場合は自動的に切断されます)');
  if (!ok) return;
  if (activeProfileIds.includes(id)) {
    await disconnectProfile(id);
  }
  profiles = profiles.filter((p) => p.id !== id);
  await persistProfiles();
  if (expandedProfileId === id) expandedProfileId = null;
  renderProfileList();
}

/* ------------------------- プロファイル個別設定 ------------------------- */

async function getProfileSetting(id, key, fallback) {
  const value = await invoke('cfg_get', { key: `profile:${id}:${key}` });
  return value === null || value === undefined ? fallback : value;
}
async function setProfileSetting(id, key, value) {
  await invoke('cfg_set', { key: `profile:${id}:${key}`, value });
}

/** このプロファイルのバーが今開いている(接続中)なら、設定変更をその場で
 * 反映させるためのイベントを送る。開いていなければ何もしない
 * (次に接続した時に新しい設定で開かれるため問題ない)。 */
async function notifyIfActive(id) {
  if (activeProfileIds.includes(id)) {
    await invoke('notify_profile_settings_changed', { label: `bar-${id}` });
  }
}

/** OS側のホットキーはプロセス全体で一意である必要があるため、プロファイルを
 * またいだ重複を検出する。自分自身(excludeProfileId+excludeEmojiId)は除く。 */
async function findGlobalHotkeyConflict(shortcut, excludeProfileId, excludeEmojiId) {
  for (const p of profiles) {
    const map = await getProfileSetting(p.id, 'hotkeys', {});
    for (const [emojiId, sc] of Object.entries(map || {})) {
      if (sc === shortcut && !(p.id === excludeProfileId && emojiId === excludeEmojiId)) {
        return { profile: p, emojiId };
      }
    }
  }
  return null;
}

/**
 * 現在「接続する」状態(activeProfileIds)にあるすべてのプロファイルの
 * ホットキーを、実際にOS側のグローバルショートカットとして一括で登録し直す。
 *
 * 重要: tauri-plugin-global-shortcutは登録済みショートカットをアプリ全体で
 * 1つのマップとして管理しており(プラグイン本体のソースコードで確認済み)、
 * ウィンドウ単位の分離は無い。そのため、もし各バーウィンドウ(bar.js)が
 * 自分の分だけのつもりでgs.unregisterAll()を呼んでしまうと、他プロファイルの
 * バーが登録した分まで巻き添えで消えてしまう。この問題を避けるため、
 * ホットキーの実際のOS登録・解除はこのメインウィンドウ(main.js)からのみ、
 * 常に「全部消してから全部登録し直す」形で一括して行う(以前の単一配信者接続
 * 時代の実装と同じ考え方を、複数プロファイル分に拡張したもの)。
 *
 * 押された時に呼ぶハンドラでは、実際のリアクション送信(連打防止のdebounceや
 * WebSocket送信)はそのプロファイルのバー自身に任せる(バーごとに独立した
 * relayClientを持っているため)。ここではsend_hotkey_reactionコマンド経由で
 * 対象のバーウィンドウに"hotkey-reaction"イベントを転送するだけでよい。
 */
async function applyAllHotkeys() {
  const gs = window.__TAURI__.globalShortcut;
  if (!gs) return; // Windows以外の開発環境などでプラグインが無い場合は何もしない
  try {
    await gs.unregisterAll();
  } catch {
    // 失敗しても続行(登録し直しで上書きされることを期待する)
  }
  for (const profileId of activeProfileIds) {
    const map = await getProfileSetting(profileId, 'hotkeys', {});
    for (const [emojiId, shortcut] of Object.entries(map || {})) {
      try {
        await gs.register(shortcut, (event) => {
          if (event.state === 'Pressed') {
            invoke('send_hotkey_reaction', { label: `bar-${profileId}`, emojiId });
          }
        });
      } catch (err) {
        console.error(`ホットキー "${shortcut}" (${emojiId}, ${profileId}) の登録に失敗しました`, err);
      }
    }
  }
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

function buildShortcutFromEvent(e) {
  const modifierOnlyCodes = [
    'ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight',
    'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight',
  ];
  if (modifierOnlyCodes.includes(e.code)) return null;
  const parts = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Super');
  parts.push(e.code);
  return parts.join('+');
}

function hotkeyCaptureKeydownListener(e) {
  if (!hotkeyCapturing) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.code === 'Escape') {
    endHotkeyCapture();
    return;
  }
  const shortcut = buildShortcutFromEvent(e);
  if (!shortcut) return;
  const { profileId, emojiId } = hotkeyCapturing;
  (async () => {
    const conflict = await findGlobalHotkeyConflict(shortcut, profileId, emojiId);
    const errEl = document.getElementById(`hkError-${profileId}`);
    if (conflict) {
      const conflictEmoji = window.EmojiSet.EMOJI_BY_ID.get(conflict.emojiId);
      if (errEl) {
        errEl.textContent = `「${formatHotkeyLabel(shortcut)}」は既に配信者「${conflict.profile.label}」の「${conflictEmoji ? conflictEmoji.label : conflict.emojiId}」に割り当てられています`;
      }
      return;
    }
    // OS側がこのキーの組み合わせを実際に受け付けるかを、試しに登録して
    // すぐ解除することで確認する(他のアプリで既に使われている等の理由で
    // 拒否されることがある)。捕捉開始時(startHotkeyCapture)に既存の
    // ホットキーはすべて解除済みなので、この試験登録は必ずクリーンな状態に
    // 対して行われる。
    const gs = window.__TAURI__.globalShortcut;
    if (gs) {
      try {
        await gs.register(shortcut, () => {});
        await gs.unregister(shortcut);
      } catch (err) {
        if (errEl) {
          errEl.textContent = `「${formatHotkeyLabel(shortcut)}」は登録できませんでした(他のアプリで使用中の可能性があります)`;
        }
        console.error(err);
        return;
      }
    }
    const map = await getProfileSetting(profileId, 'hotkeys', {});
    map[emojiId] = shortcut;
    await setProfileSetting(profileId, 'hotkeys', map);
    if (errEl) errEl.textContent = '';
    await notifyIfActive(profileId);
    await endHotkeyCapture();
  })();
}

/** ホットキーの捕捉(キー入力待ち)を開始する。
 *
 * 重要: 捕捉開始時に、今現在OS側へ登録済みの全ホットキー(すべての接続中
 * プロファイル分)を一旦すべて解除する。これをしないと、「既に別のリアクション
 * (別の配信者のものも含む)に割り当て済みのキー」を捕捉しようと押した瞬間、
 * そのキー入力はOS側のグローバルショートカットのグラブに奪われてしまい、
 * この画面のkeydownリスナーまで届かず反応しなくなることがある。 */
async function startHotkeyCapture(profileId, emojiId) {
  if (!hotkeyCapturing) {
    const gs = window.__TAURI__.globalShortcut;
    if (gs) {
      try { await gs.unregisterAll(); } catch { /* 無視 */ }
    }
    document.addEventListener('keydown', hotkeyCaptureKeydownListener, true);
  }
  hotkeyCapturing = { profileId, emojiId };
  clearTimeout(hotkeyCaptureTimeoutId);
  hotkeyCaptureTimeoutId = setTimeout(() => {
    if (!hotkeyCapturing || hotkeyCapturing.profileId !== profileId || hotkeyCapturing.emojiId !== emojiId) {
      return;
    }
    const errEl = document.getElementById(`hkError-${profileId}`);
    if (errEl) errEl.textContent = 'キー入力を検出できませんでした。別のキーの組み合わせでもう一度お試しください';
    endHotkeyCapture();
  }, HOTKEY_CAPTURE_TIMEOUT_MS);
  renderProfileList();
}

/** 捕捉を終了し(確定・Escでの取消・タイムアウトのいずれの場合も)、
 * その時点の全プロファイルのホットキーでOS側の登録を復元する。 */
async function endHotkeyCapture() {
  document.removeEventListener('keydown', hotkeyCaptureKeydownListener, true);
  clearTimeout(hotkeyCaptureTimeoutId);
  hotkeyCapturing = null;
  await applyAllHotkeys();
  renderProfileList();
}

async function clearHotkey(profileId, emojiId) {
  const map = await getProfileSetting(profileId, 'hotkeys', {});
  delete map[emojiId];
  await setProfileSetting(profileId, 'hotkeys', map);
  await notifyIfActive(profileId);
  await applyAllHotkeys();
  renderProfileList();
}

/** プロファイルカードの「詳細設定」内、ホットキー一覧部分を描画する。 */
async function renderProfileHotkeyList(container, profileId) {
  container.innerHTML = '';
  const hotkeyMap = await getProfileSetting(profileId, 'hotkeys', {});
  const order = getEffectiveOrder().filter((id) => !hiddenReactionIds.includes(id));
  for (const emojiId of order) {
    const emoji = window.EmojiSet.EMOJI_BY_ID.get(emojiId);
    if (!emoji) continue;
    const row = document.createElement('div');
    row.className = 'ps-hk-row';

    const emojiSpan = document.createElement('span');
    emojiSpan.className = 'ps-hk-emoji';
    emojiSpan.textContent = emoji.char;
    row.appendChild(emojiSpan);

    const labelSpan = document.createElement('span');
    labelSpan.className = 'ps-hk-label';
    labelSpan.textContent = emoji.label;
    row.appendChild(labelSpan);

    const isCapturing =
      hotkeyCapturing && hotkeyCapturing.profileId === profileId && hotkeyCapturing.emojiId === emojiId;
    const currentHotkey = hotkeyMap[emojiId];
    const currentSpan = document.createElement('span');
    currentSpan.className = 'ps-hk-current' + (currentHotkey ? '' : ' unset') + (isCapturing ? ' capturing' : '');
    currentSpan.textContent = isCapturing
      ? 'キーを押してください(Esc)'
      : currentHotkey
      ? formatHotkeyLabel(currentHotkey)
      : '未設定';
    row.appendChild(currentSpan);

    const setBtn = document.createElement('button');
    setBtn.className = 'secondary';
    setBtn.textContent = isCapturing ? '取消' : '設定';
    setBtn.addEventListener('click', () => {
      if (isCapturing) {
        endHotkeyCapture();
      } else {
        startHotkeyCapture(profileId, emojiId);
      }
    });
    row.appendChild(setBtn);

    const clearBtn = document.createElement('button');
    clearBtn.className = 'secondary';
    clearBtn.textContent = '解除';
    clearBtn.disabled = !currentHotkey;
    clearBtn.addEventListener('click', () => clearHotkey(profileId, emojiId));
    row.appendChild(clearBtn);

    container.appendChild(row);
  }
}

/** プロファイルカードの「詳細設定」パネルを構築する。 */
async function buildProfileSettingsPanel(profile) {
  const id = profile.id;
  const panel = document.createElement('div');
  panel.className = 'profile-settings';

  const placementMode = await getProfileSetting(id, 'placementMode', 'right');
  const marginPx = await getProfileSetting(id, 'commentMargin', 60);
  const rightRows = await getProfileSetting(id, 'placementRightRows', 10);
  const followTarget = await getProfileSetting(id, 'followTarget', true);
  const alwaysOnTop = await getProfileSetting(id, 'alwaysOnTop', true);
  const manualOverridePath = await getProfileSetting(id, 'manualOverridePath', null);
  const broadcasterName = profile.broadcasterName || '';

  // 配信者名の編集
  const bnRow = document.createElement('div');
  bnRow.className = 'field-row';
  const bnLabel = document.createElement('label');
  bnLabel.textContent = '配信者名(絞り込み用・任意)';
  bnRow.appendChild(bnLabel);
  const bnInput = document.createElement('input');
  bnInput.type = 'text';
  bnInput.value = broadcasterName;
  bnInput.placeholder = '例: あお(空欄なら絞り込みなし)';
  bnInput.maxLength = 64;
  bnInput.spellcheck = false;
  bnInput.addEventListener('change', async () => {
    profile.broadcasterName = bnInput.value.trim();
    await persistProfiles();
    await notifyIfActive(id);
    renderProfileList();
  });
  bnRow.appendChild(bnInput);

  // 合言葉だけで接続した配信者は配信者名が空欄のままのことが多いが、後から
  // 手入力するのは面倒という要望があったため、今検出できているpcwmp/
  // PCRPlayerウィンドウのタイトルをそのまま入力欄にコピーするボタンを用意する。
  // タイトルには配信者名以外の文字が含まれることもあるため、自動判定はせず
  // そのままコピーするだけにとどめ、必要なら人間が編集して確定してもらう。
  const bnFillBtn = document.createElement('button');
  bnFillBtn.type = 'button';
  bnFillBtn.className = 'secondary';
  bnFillBtn.textContent = '検出中のウィンドウ名から入力';
  bnFillBtn.title = '今検出できているpcwmp/PCRPlayerウィンドウのタイトルをそのまま入力欄にコピーします(必要に応じて編集してください)';
  const bnFillDefaultText = bnFillBtn.textContent;
  bnFillBtn.addEventListener('click', async () => {
    const status = await invoke('cfg_get', { key: `profile:${id}:runtimeStatus` });
    if (!status || !status.found || !status.title) {
      bnFillBtn.textContent = '今は検出できていません';
      setTimeout(() => {
        bnFillBtn.textContent = bnFillDefaultText;
      }, 1500);
      return;
    }
    bnInput.value = status.title;
    profile.broadcasterName = status.title;
    await persistProfiles();
    await notifyIfActive(id);
    renderProfileList();
  });
  bnRow.appendChild(bnFillBtn);
  panel.appendChild(bnRow);

  // 表示名の編集
  const lblRow = document.createElement('div');
  lblRow.className = 'field-row';
  const lblLabel = document.createElement('label');
  lblLabel.textContent = '一覧での表示名';
  lblRow.appendChild(lblLabel);
  const lblInput = document.createElement('input');
  lblInput.type = 'text';
  lblInput.value = profile.label || '';
  lblInput.maxLength = 64;
  lblInput.spellcheck = false;
  lblInput.addEventListener('change', async () => {
    const v = lblInput.value.trim();
    profile.label = v || profile.broadcasterName || profile.passphrase;
    await persistProfiles();
    renderProfileList();
  });
  lblRow.appendChild(lblInput);
  panel.appendChild(lblRow);

  // 表示位置
  const placeLabel = document.createElement('label');
  placeLabel.textContent = '表示位置: ';
  const placeSelect = document.createElement('select');
  for (const [value, text] of PLACEMENT_OPTIONS) {
    const opt = document.createElement('option');
    opt.value = value;
    opt.textContent = text;
    if (value === placementMode) opt.selected = true;
    placeSelect.appendChild(opt);
  }
  placeLabel.appendChild(placeSelect);
  panel.appendChild(placeLabel);

  const marginRow = document.createElement('label');
  marginRow.className = 'ps-marginrow' + (placementMode === 'comment' ? ' visible' : '');
  marginRow.textContent = '下からの余白: ';
  const marginInputEl = document.createElement('input');
  marginInputEl.type = 'number';
  marginInputEl.min = '0';
  marginInputEl.max = '500';
  marginInputEl.step = '5';
  marginInputEl.value = String(marginPx);
  marginRow.appendChild(marginInputEl);
  marginRow.appendChild(document.createTextNode('px'));
  panel.appendChild(marginRow);

  const rightRowsRow = document.createElement('label');
  rightRowsRow.className = 'ps-rightrowsrow' + (placementMode === 'right' ? ' visible' : '');
  rightRowsRow.textContent = '縦に並べる行数: ';
  const rightRowsInputEl = document.createElement('input');
  rightRowsInputEl.type = 'number';
  rightRowsInputEl.min = '1';
  rightRowsInputEl.max = '40';
  rightRowsInputEl.step = '1';
  rightRowsInputEl.value = String(rightRows);
  rightRowsRow.appendChild(rightRowsInputEl);
  rightRowsRow.appendChild(document.createTextNode('行'));
  panel.appendChild(rightRowsRow);

  placeSelect.addEventListener('change', async () => {
    await setProfileSetting(id, 'placementMode', placeSelect.value);
    marginRow.classList.toggle('visible', placeSelect.value === 'comment');
    rightRowsRow.classList.toggle('visible', placeSelect.value === 'right');
    await notifyIfActive(id);
  });
  marginInputEl.addEventListener('change', async () => {
    const parsed = parseInt(marginInputEl.value, 10);
    const v = Number.isFinite(parsed) ? clamp(parsed, 0, 500) : 60;
    marginInputEl.value = String(v);
    await setProfileSetting(id, 'commentMargin', v);
    await notifyIfActive(id);
  });
  rightRowsInputEl.addEventListener('change', async () => {
    const parsed = parseInt(rightRowsInputEl.value, 10);
    const v = Number.isFinite(parsed) ? clamp(parsed, 1, 40) : 10;
    rightRowsInputEl.value = String(v);
    await setProfileSetting(id, 'placementRightRows', v);
    await notifyIfActive(id);
  });

  // 追従・最前面
  const followLabel = document.createElement('label');
  const followCheckbox = document.createElement('input');
  followCheckbox.type = 'checkbox';
  followCheckbox.checked = followTarget;
  followCheckbox.addEventListener('change', async () => {
    await setProfileSetting(id, 'followTarget', followCheckbox.checked);
    await notifyIfActive(id);
  });
  followLabel.appendChild(followCheckbox);
  followLabel.appendChild(document.createTextNode('対象ウィンドウに自動で追従する'));
  panel.appendChild(followLabel);

  const topLabel = document.createElement('label');
  const topCheckbox = document.createElement('input');
  topCheckbox.type = 'checkbox';
  topCheckbox.checked = alwaysOnTop;
  topCheckbox.addEventListener('change', async () => {
    await setProfileSetting(id, 'alwaysOnTop', topCheckbox.checked);
    await notifyIfActive(id);
  });
  topLabel.appendChild(topCheckbox);
  topLabel.appendChild(document.createTextNode('常に最前面に表示する'));
  panel.appendChild(topLabel);

  // 対象ウィンドウの手動指定
  const manualHeading = document.createElement('div');
  manualHeading.style.fontSize = '12px';
  manualHeading.style.color = '#ccc';
  manualHeading.textContent = manualOverridePath
    ? `手動選択中の対象: ${manualOverridePath}`
    : '対象ウィンドウ: 自動検出' + (broadcasterName ? `(配信者名「${broadcasterName}」で絞り込み)` : '');
  panel.appendChild(manualHeading);

  const manualBtnRow = document.createElement('div');
  const refreshListBtn = document.createElement('button');
  refreshListBtn.className = 'secondary';
  refreshListBtn.textContent = '候補一覧から選ぶ';
  const pickBtn = document.createElement('button');
  pickBtn.className = 'secondary';
  pickBtn.textContent = '対象ウィンドウを手動選択';
  const clearManualBtn = document.createElement('button');
  clearManualBtn.className = 'secondary';
  clearManualBtn.textContent = '自動検出に戻す';
  clearManualBtn.disabled = !manualOverridePath;
  manualBtnRow.appendChild(refreshListBtn);
  manualBtnRow.appendChild(pickBtn);
  manualBtnRow.appendChild(clearManualBtn);
  panel.appendChild(manualBtnRow);

  const listStatus = document.createElement('div');
  listStatus.className = 'ps-status-line';
  panel.appendChild(listStatus);
  const windowListEl = document.createElement('div');
  windowListEl.className = 'ps-window-list';
  panel.appendChild(windowListEl);

  clearManualBtn.addEventListener('click', async () => {
    await setProfileSetting(id, 'manualOverridePath', null);
    await notifyIfActive(id);
    renderProfileList();
  });

  refreshListBtn.addEventListener('click', async () => {
    listStatus.textContent = '取得中...';
    windowListEl.innerHTML = '';
    let windows;
    try {
      windows = await invoke('list_candidate_windows');
    } catch (err) {
      listStatus.textContent = `取得に失敗しました: ${err.message || err}`;
      return;
    }
    if (!windows || windows.length === 0) {
      listStatus.textContent = 'ウィンドウが見つかりませんでした(この機能はWindows上でのみ利用できます)';
      return;
    }
    listStatus.textContent = `${windows.length}件見つかりました。pcwmpのウィンドウを選んでください`;
    for (const w of windows) {
      const item = document.createElement('button');
      item.className = 'ps-window-item';
      item.innerHTML = `${w.title}<span class="path">${w.path || '(パス不明)'}</span>`;
      item.addEventListener('click', async () => {
        await setProfileSetting(id, 'manualOverridePath', w.path);
        await notifyIfActive(id);
        listStatus.textContent = `選択しました: ${w.title}`;
        renderProfileList();
      });
      windowListEl.appendChild(item);
    }
  });

  pickBtn.addEventListener('click', async () => {
    listStatus.textContent = '対象のウィンドウをクリックしてください... (このウィンドウは一時的に隠れます)';
    const result = await invoke('start_manual_pick', { windowLabel: 'main' });
    if (result.ok) {
      listStatus.textContent = `選択しました: ${result.title}`;
      await setProfileSetting(id, 'manualOverridePath', result.path);
      await notifyIfActive(id);
      renderProfileList();
    } else {
      listStatus.textContent = `選択できませんでした: ${result.message}`;
    }
  });

  // ホットキー一覧
  const hkHeading = document.createElement('h2');
  hkHeading.style.marginTop = '4px';
  hkHeading.textContent = 'この配信者のホットキー(グローバルショートカット)';
  panel.appendChild(hkHeading);
  const hkList = document.createElement('div');
  hkList.className = 'ps-hotkey-list';
  panel.appendChild(hkList);
  const hkError = document.createElement('div');
  hkError.className = 'ps-hk-error';
  hkError.id = `hkError-${id}`;
  panel.appendChild(hkError);
  await renderProfileHotkeyList(hkList, id);

  return panel;
}

/** 「新規追加時の詳細設定ひな形(デフォルト値)」パネルを構築する。
 * buildProfileSettingsPanel()とほぼ同じ作りだが、実在のプロファイルではない
 * (配信者名・表示名を持たない、対象ウィンドウの自動検出/手動指定という概念も
 * 存在しない)ため、その2項目は省いてある。ここで編集した値は
 * applyDefaultSettingsToNewProfile()経由で、新しく配信者を追加した瞬間に
 * その配信者のプロファイル設定としてコピーされる。 */
async function buildDefaultSettingsPanel() {
  const id = DEFAULT_SETTINGS_ID;
  const panel = document.createElement('div');
  panel.className = 'profile-settings';

  const placementMode = await getProfileSetting(id, 'placementMode', 'right');
  const marginPx = await getProfileSetting(id, 'commentMargin', 60);
  const rightRows = await getProfileSetting(id, 'placementRightRows', 10);
  const followTarget = await getProfileSetting(id, 'followTarget', true);
  const alwaysOnTop = await getProfileSetting(id, 'alwaysOnTop', true);

  // 表示位置
  const placeLabel = document.createElement('label');
  placeLabel.textContent = '表示位置: ';
  const placeSelect = document.createElement('select');
  for (const [value, text] of PLACEMENT_OPTIONS) {
    const opt = document.createElement('option');
    opt.value = value;
    opt.textContent = text;
    if (value === placementMode) opt.selected = true;
    placeSelect.appendChild(opt);
  }
  placeLabel.appendChild(placeSelect);
  panel.appendChild(placeLabel);

  const marginRow = document.createElement('label');
  marginRow.className = 'ps-marginrow' + (placementMode === 'comment' ? ' visible' : '');
  marginRow.textContent = '下からの余白: ';
  const marginInputEl = document.createElement('input');
  marginInputEl.type = 'number';
  marginInputEl.min = '0';
  marginInputEl.max = '500';
  marginInputEl.step = '5';
  marginInputEl.value = String(marginPx);
  marginRow.appendChild(marginInputEl);
  marginRow.appendChild(document.createTextNode('px'));
  panel.appendChild(marginRow);

  const rightRowsRow = document.createElement('label');
  rightRowsRow.className = 'ps-rightrowsrow' + (placementMode === 'right' ? ' visible' : '');
  rightRowsRow.textContent = '縦に並べる行数: ';
  const rightRowsInputEl = document.createElement('input');
  rightRowsInputEl.type = 'number';
  rightRowsInputEl.min = '1';
  rightRowsInputEl.max = '40';
  rightRowsInputEl.step = '1';
  rightRowsInputEl.value = String(rightRows);
  rightRowsRow.appendChild(rightRowsInputEl);
  rightRowsRow.appendChild(document.createTextNode('行'));
  panel.appendChild(rightRowsRow);

  placeSelect.addEventListener('change', async () => {
    await setProfileSetting(id, 'placementMode', placeSelect.value);
    marginRow.classList.toggle('visible', placeSelect.value === 'comment');
    rightRowsRow.classList.toggle('visible', placeSelect.value === 'right');
  });
  marginInputEl.addEventListener('change', async () => {
    const parsed = parseInt(marginInputEl.value, 10);
    const v = Number.isFinite(parsed) ? clamp(parsed, 0, 500) : 60;
    marginInputEl.value = String(v);
    await setProfileSetting(id, 'commentMargin', v);
  });
  rightRowsInputEl.addEventListener('change', async () => {
    const parsed = parseInt(rightRowsInputEl.value, 10);
    const v = Number.isFinite(parsed) ? clamp(parsed, 1, 40) : 10;
    rightRowsInputEl.value = String(v);
    await setProfileSetting(id, 'placementRightRows', v);
  });

  // 追従・最前面
  const followLabel = document.createElement('label');
  const followCheckbox = document.createElement('input');
  followCheckbox.type = 'checkbox';
  followCheckbox.checked = followTarget;
  followCheckbox.addEventListener('change', async () => {
    await setProfileSetting(id, 'followTarget', followCheckbox.checked);
  });
  followLabel.appendChild(followCheckbox);
  followLabel.appendChild(document.createTextNode('対象ウィンドウに自動で追従する'));
  panel.appendChild(followLabel);

  const topLabel = document.createElement('label');
  const topCheckbox = document.createElement('input');
  topCheckbox.type = 'checkbox';
  topCheckbox.checked = alwaysOnTop;
  topCheckbox.addEventListener('change', async () => {
    await setProfileSetting(id, 'alwaysOnTop', topCheckbox.checked);
  });
  topLabel.appendChild(topCheckbox);
  topLabel.appendChild(document.createTextNode('常に最前面に表示する'));
  panel.appendChild(topLabel);

  // ホットキー一覧(ひな形)
  const hkHeading = document.createElement('h2');
  hkHeading.style.marginTop = '4px';
  hkHeading.textContent = 'ホットキーのひな形(新規追加時にコピーされます)';
  panel.appendChild(hkHeading);
  const hkHint = document.createElement('p');
  hkHint.className = 'field-hint';
  hkHint.style.margin = '0 0 4px';
  hkHint.textContent = 'ここで設定したキーは、新しく配信者を追加した時点で他の接続中の配信者と重複していないものだけが自動的にコピーされます(重複するものは未設定のままになるので、追加後に個別に設定してください)。';
  panel.appendChild(hkHint);
  const hkList = document.createElement('div');
  hkList.className = 'ps-hotkey-list';
  panel.appendChild(hkList);
  const hkError = document.createElement('div');
  hkError.className = 'ps-hk-error';
  hkError.id = `hkError-${id}`;
  panel.appendChild(hkError);
  await renderProfileHotkeyList(hkList, id);

  return panel;
}

/** 新しく追加したプロファイル(id)に、デフォルト設定ひな形の内容をコピーする。
 * 表示位置系の設定はそのままコピーする一方、ホットキーは接続中の他の配信者と
 * 衝突しないものだけを選んでコピーする(衝突するものまで機械的にコピーすると、
 * OS側のグローバルショートカット登録の際に一部が登録できず、原因が分かり
 * づらい形で無言失敗するため)。addProfileFormの送信ハンドラから、
 * persistProfiles()の後・connectProfile()の前に呼ばれる想定。 */
async function applyDefaultSettingsToNewProfile(id) {
  const placementMode = await getProfileSetting(DEFAULT_SETTINGS_ID, 'placementMode', 'right');
  const marginPx = await getProfileSetting(DEFAULT_SETTINGS_ID, 'commentMargin', 60);
  const rightRows = await getProfileSetting(DEFAULT_SETTINGS_ID, 'placementRightRows', 10);
  const followTarget = await getProfileSetting(DEFAULT_SETTINGS_ID, 'followTarget', true);
  const alwaysOnTop = await getProfileSetting(DEFAULT_SETTINGS_ID, 'alwaysOnTop', true);
  await setProfileSetting(id, 'placementMode', placementMode);
  await setProfileSetting(id, 'commentMargin', marginPx);
  await setProfileSetting(id, 'placementRightRows', rightRows);
  await setProfileSetting(id, 'followTarget', followTarget);
  await setProfileSetting(id, 'alwaysOnTop', alwaysOnTop);

  const templateHotkeys = await getProfileSetting(DEFAULT_SETTINGS_ID, 'hotkeys', {});
  const newHotkeys = {};
  for (const [emojiId, shortcut] of Object.entries(templateHotkeys || {})) {
    const conflict = await findGlobalHotkeyConflict(shortcut, id, emojiId);
    if (!conflict) {
      newHotkeys[emojiId] = shortcut;
    }
  }
  if (Object.keys(newHotkeys).length > 0) {
    await setProfileSetting(id, 'hotkeys', newHotkeys);
  }
}

/* ------------------------- 一覧描画 ------------------------- */

// renderProfileList()はプロファイルごとにいくつもcfg_get()を待つ(await)ため、
// 完了までに数十〜数百msかかることがある。その間に別の操作(詳細設定を開く・
// 接続する等)でもう一度renderProfileList()が呼ばれると、2つの呼び出しが
// 同時にprofileListEl.innerHTMLを触り合って一覧が壊れる(片方が空にした直後に
// もう片方が古い内容を書き戻す等)おそれがあったため、常に1つずつ順番に
// 実行されるようキューで直列化する。呼び出し側は従来通りrenderProfileList()を
// 呼ぶだけでよい(awaitしてもしなくても、必ずこの直列実行の恩恵を受ける)。
let profileListRenderQueue = Promise.resolve();
function renderProfileList() {
  profileListRenderQueue = profileListRenderQueue.then(renderProfileListImpl).catch((err) => {
    console.error('配信者一覧の描画に失敗しました', err);
  });
  return profileListRenderQueue;
}

async function renderProfileListImpl() {
  // デフォルト設定ひな形パネルもここで一緒に再描画する(ホットキー捕捉中の
  // 表示更新などがrenderProfileList()経由で走るため、プロファイル一覧と
  // ズレないようこの直列キューに乗せておく)。<details>要素自体には触れず
  // 中身のコンテナだけを差し替えるので、開閉状態は再描画のたびに保たれる。
  // 開いていない間は(cfg_get連発の)無駄な再構築をしないよう省略する。
  if (defaultSettingsPanelEl && defaultSettingsDetails && defaultSettingsDetails.open) {
    defaultSettingsPanelEl.innerHTML = '';
    defaultSettingsPanelEl.appendChild(await buildDefaultSettingsPanel());
  }

  emptyProfilesMsg.style.display = profiles.length === 0 ? 'block' : 'none';
  profileListEl.innerHTML = '';

  for (const profile of profiles) {
    const isActive = activeProfileIds.includes(profile.id);
    const card = document.createElement('div');
    card.className = 'profile-card' + (isActive ? ' connected' : '');

    const head = document.createElement('div');
    head.className = 'profile-head';

    const nameSpan = document.createElement('span');
    nameSpan.className = 'profile-name';
    nameSpan.textContent = profile.label || profile.passphrase;
    head.appendChild(nameSpan);

    const passSpan = document.createElement('span');
    passSpan.className = 'profile-passphrase';
    passSpan.textContent = profile.passphrase;
    head.appendChild(passSpan);

    const bnBadge = document.createElement('span');
    bnBadge.className = 'profile-broadcaster-badge' + (profile.broadcasterName ? '' : ' unset');
    bnBadge.textContent = profile.broadcasterName ? `配信者名: ${profile.broadcasterName}` : '配信者名: 指定なし';
    head.appendChild(bnBadge);

    if (isActive) {
      // 「接続する」を押しても見た目に変化が分かりにくい(枠の色だけでは
      // 気づきにくい)という報告があったため、はっきり文字でも状態を示す。
      const connectedBadge = document.createElement('span');
      connectedBadge.className = 'profile-connected-badge';
      connectedBadge.textContent = '接続中';
      head.appendChild(connectedBadge);
    }

    card.appendChild(head);

    const statusLine = document.createElement('div');
    statusLine.className = 'profile-status';
    statusLine.id = `status-${profile.id}`;
    statusLine.textContent = isActive ? '状態を確認しています...' : '';
    card.appendChild(statusLine);

    const btnRow = document.createElement('div');
    btnRow.className = 'profile-buttons';

    const connectBtn = document.createElement('button');
    connectBtn.textContent = isActive ? '切断する' : '接続する';
    connectBtn.className = isActive ? 'secondary' : '';
    connectBtn.addEventListener('click', () => {
      if (isActive) {
        disconnectProfile(profile.id);
      } else {
        connectProfile(profile.id);
      }
    });
    btnRow.appendChild(connectBtn);

    const expandBtn = document.createElement('button');
    expandBtn.className = 'secondary';
    expandBtn.textContent = expandedProfileId === profile.id ? '詳細設定を閉じる' : '詳細設定';
    expandBtn.addEventListener('click', () => {
      expandedProfileId = expandedProfileId === profile.id ? null : profile.id;
      renderProfileList();
    });
    btnRow.appendChild(expandBtn);

    // 「対象が見つかるまでバーを隠す」既定動作の手動上書きボタン。ONの間は
    // 対象ウィンドウが見つかっていなくてもバーを表示し続ける(bar.js参照)。
    const manualShowValue = await getProfileSetting(profile.id, 'manualShow', false);
    const showToggleBtn = document.createElement('button');
    showToggleBtn.className = 'secondary';
    showToggleBtn.textContent = manualShowValue ? 'ボタン非表示' : 'ボタン表示';
    showToggleBtn.title = manualShowValue
      ? '対象が見つかっていなくてもバーを表示し続けています。押すと既定動作(見つかるまで隠す)に戻します'
      : '既定では対象が見つかるまでバーは隠れています。押すと対象が無くても常時表示します';
    showToggleBtn.addEventListener('click', () => toggleManualShow(profile.id));
    btnRow.appendChild(showToggleBtn);

    const deleteBtn = document.createElement('button');
    deleteBtn.className = 'danger';
    deleteBtn.textContent = '削除';
    deleteBtn.addEventListener('click', () => deleteProfile(profile.id));
    btnRow.appendChild(deleteBtn);

    card.appendChild(btnRow);

    if (expandedProfileId === profile.id) {
      const settingsPanel = await buildProfileSettingsPanel(profile);
      card.appendChild(settingsPanel);
    }

    profileListEl.appendChild(card);
  }
}

/* ------------------------- 接続中プロファイルの状態表示ポーリング ------------------------- */

function startStatusPolling() {
  if (statusPollTimer) clearInterval(statusPollTimer);
  statusPollTimer = setInterval(async () => {
    for (const id of activeProfileIds) {
      const el = document.getElementById(`status-${id}`);
      if (!el) continue;
      const status = await invoke('cfg_get', { key: `profile:${id}:runtimeStatus` });
      if (status && status.found) {
        el.textContent = `検出中: ${status.title || '(タイトルなし)'}`;
        el.classList.remove('not-found');
      } else {
        el.textContent = '対象のウィンドウ(pcwmp/PCRPlayer等)が見つかりません。詳細設定の「候補一覧から選ぶ」等をお試しください。';
        el.classList.add('not-found');
      }
    }
  }, STATUS_POLL_MS);
}

/* ------------------------- 旧バージョン(単一配信者接続時代)の設定の引き継ぎ ------------------------- */

/** ReaTapが単一配信者接続だった頃のconnectInfo等が残っている場合、それを
 * 1件目のプロファイルとして自動的に引き継ぐ(既にbroadcasterProfilesが
 * 存在する場合は何もしない=1回だけ行われる)。 */
async function migrateLegacyConfigIfNeeded() {
  const existing = await invoke('cfg_get', { key: 'broadcasterProfiles' });
  if (Array.isArray(existing) && existing.length > 0) return;

  const legacyConnectInfo = await invoke('cfg_get', { key: 'connectInfo' });
  if (!legacyConnectInfo || typeof legacyConnectInfo.passphrase !== 'string') return;

  const id = genProfileId();
  const migrated = {
    id,
    passphrase: legacyConnectInfo.passphrase,
    broadcasterName: '',
    label: legacyConnectInfo.passphrase,
  };
  await invoke('cfg_set', { key: 'broadcasterProfiles', value: [migrated] });

  const legacyManual = await invoke('cfg_get', { key: 'manualOverridePath' });
  const legacyPlacement = await invoke('cfg_get', { key: 'placementMode' });
  const legacyMargin = await invoke('cfg_get', { key: 'commentMargin' });
  const legacyRightRows = await invoke('cfg_get', { key: 'placementRightRows' });
  const legacyHotkeys = await invoke('cfg_get', { key: 'hotkeys' });

  if (typeof legacyManual === 'string') {
    await invoke('cfg_set', { key: `profile:${id}:manualOverridePath`, value: legacyManual });
  }
  if (typeof legacyPlacement === 'string') {
    await invoke('cfg_set', { key: `profile:${id}:placementMode`, value: legacyPlacement });
  }
  if (typeof legacyMargin === 'number') {
    await invoke('cfg_set', { key: `profile:${id}:commentMargin`, value: legacyMargin });
  }
  if (typeof legacyRightRows === 'number') {
    await invoke('cfg_set', { key: `profile:${id}:placementRightRows`, value: legacyRightRows });
  }
  if (legacyHotkeys && typeof legacyHotkeys === 'object') {
    await invoke('cfg_set', { key: `profile:${id}:hotkeys`, value: legacyHotkeys });
  }
  // 以前は接続情報があれば起動時に自動でリアクション表示していたので、
  // 引き継いだプロファイルも自動的に「接続する」状態にしておく。
  await invoke('cfg_set', { key: 'activeProfileIds', value: [id] });
}

/* ------------------------- 初期化 ------------------------- */

async function init() {
  await migrateLegacyConfigIfNeeded();

  const savedProfiles = await invoke('cfg_get', { key: 'broadcasterProfiles' });
  profiles = Array.isArray(savedProfiles) ? savedProfiles : [];

  const savedActive = await invoke('cfg_get', { key: 'activeProfileIds' });
  activeProfileIds = Array.isArray(savedActive)
    ? savedActive.filter((id) => profiles.some((p) => p.id === id))
    : [];

  const savedOrder = await invoke('cfg_get', { key: 'reactionOrder' });
  reactionOrder = Array.isArray(savedOrder) ? savedOrder : null;
  const savedHidden = await invoke('cfg_get', { key: 'hiddenReactionIds' });
  hiddenReactionIds = Array.isArray(savedHidden) ? savedHidden : [];
  renderReactionOrderList();

  const hostOverride = await invoke('cfg_get', { key: 'relayHostOverride' });
  relayHostInput.value = typeof hostOverride === 'string' ? hostOverride : '';
  const portOverride = await invoke('cfg_get', { key: 'relayPortOverride' });
  relayPortInput.value = typeof portOverride === 'number' ? String(portOverride) : '';
  const builtRelayAddress = await invoke('get_relay_address');
  relayHostInput.placeholder = `(既定: ${builtRelayAddress.host})`;
  relayPortInput.placeholder = `(既定: ${builtRelayAddress.port})`;

  await renderProfileList();

  // 起動時、以前「接続する」にしていたプロファイルのバーを再度開く
  for (const id of activeProfileIds) {
    await invoke('open_bar_window', { label: `bar-${id}` });
  }
  await applyAllHotkeys();
  startStatusPolling();

  // アプリ本体の起動処理を待たせたくないので、awaitせず裏で確認する
  // (ネットワークが無い・GitHubに繋がらない環境でも起動自体は妨げない)。
  checkForUpdateOnStartup();
}

/* ------------------------- 自動アップデート確認 ------------------------- */

/** 起動時に一度だけ、GitHub Releases上に新しいバージョンが無いか確認する。
 * ビルド時にAPP_VERSION/UPDATE_CHECK_REPO(.github/workflows/build-release.yml
 * 参照)が設定されていない開発ビルドでは、Rust側(updater.rs)が常に
 * 「更新なし」を返すため、この関数は何もしない。 */
async function checkForUpdateOnStartup() {
  let info;
  try {
    info = await invoke('check_for_update');
  } catch (err) {
    console.error('アップデート確認に失敗しました', err);
    return;
  }
  if (!info) return;

  const notes = info.notes ? `\n\n更新内容:\n${info.notes}` : '';
  const ok = await showConfirmDialog(
    `新しいバージョン ${info.version} が利用可能です。今すぐ更新しますか?${notes}\n\n(更新中はこのアプリが一旦終了し、自動的に再起動します)`
  );
  if (!ok) return;

  // OKを押してからアプリが一旦終了する(=ダウンロード完了)までの間、
  // 画面が固まったように見えて不安になる、という指摘があったため、
  // せめて「今ダウンロード中である」ことだけは分かるようにしておく。
  updateStatusMessageEl.textContent =
    '新しいバージョンをダウンロード中です。しばらくお待ちください…\n(数十秒〜数分かかることがあります。完了すると自動的にアプリが再起動します)';
  updateStatusOverlay.classList.add('visible');

  try {
    await invoke('download_and_apply_update', {
      downloadUrl: info.download_url,
      sha256: info.sha256 || null,
    });
    // 成功時は、この行に到達する前にアプリ自体が終了・再起動されるのが
    // 正常な流れ(Rust側でapp.exit(0)している)。
  } catch (err) {
    updateStatusOverlay.classList.remove('visible');
    await showConfirmDialog(
      `更新に失敗しました: ${err.message || err}\n\nお手数ですがGitHubのReleasesページから手動でダウンロードしてください。`
    );
  }
}

init().catch((err) => {
  console.error('初期化に失敗しました', err);
});
