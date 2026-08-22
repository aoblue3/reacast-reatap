'use strict';
/**
 * コントロールパネルのロジック(Tauri版)。
 * Electron版と違い、中継サーバーへの接続(RelayClient)はこのウィンドウのJS自身が
 * 直接持つ。受信したリアクションはRustコマンド経由でオーバーレイウィンドウへ転送する。
 *
 * 接続方式について: 以前は「中継サーバーのアドレス+部屋ID+視聴者トークン」を
 * 暗号化した長い接続コードを視聴者に貼り付けてもらう方式だったが、手入力しづらい
 * という要望を受けて、配信者が決めた短い「合言葉」を中継サーバーに登録し、
 * 視聴者はその合言葉だけを入力して参加する方式に変更した(中継サーバーの
 * アドレスは視聴者アプリのビルド時に埋め込む前提)。
 */
const invoke = window.__TAURI__.core.invoke;

const connStatusEl = document.getElementById('connStatus');
const viewerCountEl = document.getElementById('viewerCount');
const relayAddrEl = document.getElementById('relayAddr');
const passphraseInputEl = document.getElementById('passphraseInput');
const passphraseMsgEl = document.getElementById('passphraseMsg');
const copyBtn = document.getElementById('copyBtn');
const savePassphraseBtn = document.getElementById('savePassphraseBtn');
const testButtonsEl = document.getElementById('testButtons');
const monitorSelectEl = document.getElementById('monitorSelect');
const glyphSizeRangeEl = document.getElementById('glyphSizeRange');
const glyphSizeValueEl = document.getElementById('glyphSizeValue');
const glyphSizeResetBtn = document.getElementById('glyphSizeResetBtn');
const glyphOpacityRangeEl = document.getElementById('glyphOpacityRange');
const glyphOpacityValueEl = document.getElementById('glyphOpacityValue');
const glyphOpacityResetBtn = document.getElementById('glyphOpacityResetBtn');
const comboGrowthCheckboxEl = document.getElementById('comboGrowthCheckbox');
const reactionToggleListEl = document.getElementById('reactionToggleList');

// ネガティブなリアクション(荒らし対策で見たくない配信者もいるため)は、
// 初回は「受け付けるリアクション」から既定でOFFにしておく。一度でも
// disabledReactionIdsが保存されていれば(cfg_getが配列を返せば)そちらを
// 常に優先するので、ここで既定OFFにしても、あとでユーザーがONに変更すれば
// 次回以降もその選択が尊重される(下のIIFE内のロード処理を参照)。
const DEFAULT_DISABLED_REACTION_IDS = ['poop', 'tengu', 'angry', 'boo', 'provoke'];
const confirmOverlay = document.getElementById('confirmOverlay');
const confirmMessageEl = document.getElementById('confirmMessage');
const confirmOkBtn = document.getElementById('confirmOkBtn');
const confirmCancelBtn = document.getElementById('confirmCancelBtn');

// window.confirm()は使わない(main.js参照と同じ理由)。自前のオーバーレイで
// OK/キャンセルを待つPromiseベースの簡易ダイアログ。
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

// 単体exeのままの自動アップデート確認(updater.rs参照)。ビルド時に
// APP_VERSION/UPDATE_CHECK_REPOが埋め込まれていない場合(ローカルビルド等)は
// check_for_updateが常にnullを返すので、この関数は実質何もしない。
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

  try {
    await invoke('download_and_apply_update', {
      downloadUrl: info.download_url,
      sha256: info.sha256 || null,
    });
  } catch (err) {
    await showConfirmDialog(
      `更新に失敗しました: ${err.message || err}\n\nお手数ですがGitHubのReleasesページから手動でダウンロードしてください。`
    );
  }
}

// 中継サーバーが要求する合言葉の形式(relay-server/server.jsのPASSPHRASE_REと
// 必ず同じにしておくこと)。保存前にここで弾ければ、往復せずその場でエラーを示せる。
const PASSPHRASE_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{2,31}$/;

let relayClient = null;
let credentials = null;
// OFFにした(=受け付けない)リアクションのid一覧。テスターから「配信者側で
// いらないリアクションがあったら受け付けないOFF設定が欲しい」という要望が
// あったための機能。あくまで配信者のオーバーレイに表示しないだけで、視聴者
// 側には一切通知しない(視聴者アプリのボタン自体は押せるままにしておき、
// 「押しても何も起きない」という素朴な形にする。合言葉の共有以外に視聴者と
// 配信者の間で設定を同期する仕組みが無いため、まずはこの一番シンプルな形で
// 実装している)。
let disabledReactionIds = [];

function renderStatus({ relayConnected, viewerCount, relayHost, relayPort }) {
  connStatusEl.textContent = relayConnected ? '接続中' : '未接続';
  connStatusEl.className = 'status ' + (relayConnected ? 'ok' : 'ng');
  viewerCountEl.textContent = String(viewerCount);
  relayAddrEl.textContent = `${relayHost}:${relayPort}`;
}

function showPassphraseMsg(text, ok) {
  passphraseMsgEl.textContent = text;
  passphraseMsgEl.className = ok ? 'ok' : 'ng';
  passphraseInputEl.classList.toggle('invalid', !ok);
}

function loadEmojiTestButtons() {
  const emojiSet = window.EmojiSet.EMOJI_SET;
  testButtonsEl.innerHTML = '';
  for (const emoji of emojiSet) {
    const btn = document.createElement('button');
    btn.className = 'secondary';
    btn.textContent = emoji.char;
    btn.title = emoji.label;
    // 「草」のような、絵文字ではなく普通の文字として色指定が要る項目用
    if (emoji.color) {
      btn.style.color = emoji.color;
      btn.style.fontWeight = '900';
    }
    btn.addEventListener('click', () => {
      invoke('emit_overlay_reaction', { emoji: emoji.id, viewerId: 'test' });
    });
    testButtonsEl.appendChild(btn);
  }
}

function isReactionEnabled(emojiId) {
  return !disabledReactionIds.includes(emojiId);
}

async function persistDisabledReactionIds() {
  await invoke('cfg_set', { key: 'disabledReactionIds', value: disabledReactionIds });
}

/** 1個分のリアクションチップ要素を作る(クリックでON/OFF切り替え)。 */
function buildReactionChip(emoji) {
  const enabled = isReactionEnabled(emoji.id);
  const chip = document.createElement('label');
  chip.className = 'rt-chip' + (enabled ? '' : ' rt-off');

  const checkbox = document.createElement('input');
  checkbox.type = 'checkbox';
  checkbox.checked = enabled;
  checkbox.addEventListener('change', async () => {
    if (checkbox.checked) {
      disabledReactionIds = disabledReactionIds.filter((id) => id !== emoji.id);
    } else if (!disabledReactionIds.includes(emoji.id)) {
      disabledReactionIds.push(emoji.id);
    }
    await persistDisabledReactionIds();
    chip.classList.toggle('rt-off', !checkbox.checked);
  });
  chip.appendChild(checkbox);

  const emojiSpan = document.createElement('span');
  emojiSpan.className = 'rt-emoji';
  emojiSpan.textContent = emoji.char;
  chip.appendChild(emojiSpan);

  chip.appendChild(document.createTextNode(emoji.label));

  return chip;
}

/** 「受け付けるリアクション」のチップ一覧を描画する。通常リアクションを先に、
 * マイナスリアクション(既定でOFFにしているもの、DEFAULT_DISABLED_REACTION_IDS)
 * はその下に見出し付きでまとめて表示し、見分けやすくする。 */
function renderReactionToggleList() {
  const emojiSet = window.EmojiSet.EMOJI_SET;
  const negativeIds = new Set(DEFAULT_DISABLED_REACTION_IDS);
  const normalEmojis = emojiSet.filter((emoji) => !negativeIds.has(emoji.id));
  const negativeEmojis = emojiSet.filter((emoji) => negativeIds.has(emoji.id));

  reactionToggleListEl.innerHTML = '';

  const normalGroup = document.createElement('div');
  normalGroup.className = 'rt-group';
  for (const emoji of normalEmojis) {
    normalGroup.appendChild(buildReactionChip(emoji));
  }
  reactionToggleListEl.appendChild(normalGroup);

  if (negativeEmojis.length > 0) {
    const negativeHeading = document.createElement('div');
    negativeHeading.className = 'rt-negative-heading';
    negativeHeading.textContent = 'マイナスリアクション(既定では非表示):';
    reactionToggleListEl.appendChild(negativeHeading);

    const negativeGroup = document.createElement('div');
    negativeGroup.className = 'rt-group';
    for (const emoji of negativeEmojis) {
      negativeGroup.appendChild(buildReactionChip(emoji));
    }
    reactionToggleListEl.appendChild(negativeGroup);
  }
}

/** 「表示モニター」欄の一覧を作る。先頭に「メインモニターに従う(既定)」を
 * 用意しておき、それを選べばRust側は常に主モニターを自動追従する(overlayMonitorId
 * をnullにする)。それ以外は list_monitors が返した個々のモニターを選べる。 */
async function loadMonitorSelect() {
  const monitors = await invoke('list_monitors');
  const savedId = await invoke('cfg_get', { key: 'overlayMonitorId' });
  monitorSelectEl.innerHTML = '';
  const autoOption = document.createElement('option');
  autoOption.value = '';
  autoOption.textContent = 'メインモニターに従う(既定)';
  monitorSelectEl.appendChild(autoOption);
  for (const m of monitors) {
    const opt = document.createElement('option');
    opt.value = m.id;
    opt.textContent = m.label;
    monitorSelectEl.appendChild(opt);
  }
  // 保存されていたモニターが今は接続されていない(一覧に無い)場合は
  // 「メインモニターに従う」表示にしておく(Rust側もその場合は自動的に
  // 主モニターへフォールバックする)
  const hasSavedOption = typeof savedId === 'string' && monitors.some((m) => m.id === savedId);
  monitorSelectEl.value = hasSavedOption ? savedId : '';
}

monitorSelectEl.addEventListener('change', async () => {
  const value = monitorSelectEl.value;
  await invoke('set_overlay_monitor', { monitorId: value || null });
});

/** 「絵文字の大きさ」欄の初期値を、保存されている設定(無ければ既定100%)から
 * 復元する。 */
async function loadGlyphSize() {
  const s = await invoke('get_overlay_settings');
  const percent = Math.round((s.glyphScale ?? 1) * 100);
  glyphSizeRangeEl.value = String(percent);
  glyphSizeValueEl.textContent = `${percent}%`;
}

// スライダーをドラッグしている間は表示中の%だけをすぐ更新し、実際にRust側へ
// 送って保存・反映するのは少し間を空けてから(デバウンス)にすることで、
// ドラッグ中に大量の保存処理が走らないようにしている。
let glyphSizeDebounceTimer = null;
glyphSizeRangeEl.addEventListener('input', () => {
  const percent = Number(glyphSizeRangeEl.value);
  glyphSizeValueEl.textContent = `${percent}%`;
  clearTimeout(glyphSizeDebounceTimer);
  glyphSizeDebounceTimer = setTimeout(() => {
    invoke('set_overlay_glyph_scale', { scale: percent / 100 });
  }, 120);
});

glyphSizeResetBtn.addEventListener('click', () => {
  glyphSizeRangeEl.value = '100';
  glyphSizeValueEl.textContent = '100%';
  invoke('set_overlay_glyph_scale', { scale: 1 });
});

/** 「スタンプの透明度」欄の初期値を、保存されている設定(無ければ既定100%)から
 * 復元する。loadGlyphSizeと同じ構造。 */
async function loadGlyphOpacity() {
  const s = await invoke('get_overlay_settings');
  const percent = Math.round((s.glyphOpacity ?? 1) * 100);
  glyphOpacityRangeEl.value = String(percent);
  glyphOpacityValueEl.textContent = `${percent}%`;
}

let glyphOpacityDebounceTimer = null;
glyphOpacityRangeEl.addEventListener('input', () => {
  const percent = Number(glyphOpacityRangeEl.value);
  glyphOpacityValueEl.textContent = `${percent}%`;
  clearTimeout(glyphOpacityDebounceTimer);
  glyphOpacityDebounceTimer = setTimeout(() => {
    invoke('set_overlay_glyph_opacity', { opacity: percent / 100 });
  }, 120);
});

glyphOpacityResetBtn.addEventListener('click', () => {
  glyphOpacityRangeEl.value = '100';
  glyphOpacityValueEl.textContent = '100%';
  invoke('set_overlay_glyph_opacity', { opacity: 1 });
});

/** 「連打すると大きくなる」チェックボックスの初期値を復元する。 */
async function loadComboGrowth() {
  const s = await invoke('get_overlay_settings');
  comboGrowthCheckboxEl.checked = s.comboGrowthEnabled !== false;
}

comboGrowthCheckboxEl.addEventListener('change', () => {
  invoke('set_overlay_combo_growth', { enabled: comboGrowthCheckboxEl.checked });
});

copyBtn.addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(passphraseInputEl.value);
  } catch {
    // クリップボードAPIが使えない環境向けのフォールバック: 入力欄を選択状態にする
    passphraseInputEl.select();
  }
  copyBtn.textContent = 'コピーしました';
  setTimeout(() => (copyBtn.textContent = 'コピー'), 1500);
});

/** 今の入力欄の値を、中継サーバーに合言葉として登録(変更)する。接続中なら
 * 既存の接続に乗せてそのまま送る(register済みの接続に対しても、中継サーバー側は
 * 合言葉の再登録として扱ってくれる)。まだ繋がっていなければ何もしない
 * (繋がった時点でconnectRelayのhelloが今の入力欄の値を拾って送ってくれる)。 */
function sendPassphraseUpdate(passphrase) {
  if (!relayClient || !credentials) return;
  relayClient.send({
    type: 'register',
    roomId: credentials.roomId,
    broadcasterToken: credentials.broadcasterToken,
    passphrase,
    protocolVersion: window.RelayClient.PROTOCOL_VERSION,
  });
}

savePassphraseBtn.addEventListener('click', async () => {
  const value = passphraseInputEl.value.trim();
  if (!PASSPHRASE_RE.test(value)) {
    showPassphraseMsg(
      '合言葉は英数字で始まる3〜32文字(英数字・ハイフン・アンダースコアのみ)にしてください',
      false
    );
    return;
  }
  await invoke('cfg_set', { key: 'passphrase', value });
  showPassphraseMsg('中継サーバーに登録しています...', true);
  sendPassphraseUpdate(value);
});

async function connectRelay(creds, relayAddress, passphrase) {
  relayClient = new window.RelayClient({
    url: `ws://${relayAddress.host}:${relayAddress.port}`,
    hello: {
      type: 'register',
      roomId: creds.roomId,
      broadcasterToken: creds.broadcasterToken,
      passphrase,
    },
  });

  const state = { relayConnected: false, viewerCount: 0 };
  const push = () =>
    renderStatus({
      relayConnected: state.relayConnected,
      viewerCount: state.viewerCount,
      relayHost: relayAddress.host,
      relayPort: relayAddress.port,
    });

  relayClient.on('type:registered', () => {
    state.relayConnected = true;
    push();
  });
  relayClient.on('type:viewerCount', (m) => {
    state.viewerCount = m.count;
    push();
  });
  relayClient.on('type:passphrase_ok', (m) => {
    showPassphraseMsg(`「${m.passphrase}」でこの配信への参加を受け付けています`, true);
  });
  relayClient.on('type:error', (m) => {
    if (m.code === 'passphrase_taken' || m.code === 'invalid_passphrase') {
      showPassphraseMsg(m.message, false);
    } else if (m.code === 'outdated_app') {
      showPassphraseMsg(
        '中継サーバーとの通信に失敗しました(アプリのバージョンが古い可能性があります)。最新版に更新してください。',
        false
      );
    }
  });
  relayClient.on('type:reaction', (m) => {
    // OFFにしたリアクションは配信画面に一切表示しない(視聴者には何も
    // 通知しない。視聴者アプリのボタンは押せるがサイレントに無視される)。
    if (!isReactionEnabled(m.emoji)) return;
    invoke('emit_overlay_reaction', { emoji: m.emoji, viewerId: m.viewerId });
  });
  relayClient.on('close', () => {
    state.relayConnected = false;
    push();
  });
  relayClient.on('reconnecting', () => {
    state.relayConnected = false;
    push();
  });

  push();
  relayClient.connect();
}

(async () => {
  const [creds, relayAddress, savedPassphrase, savedDisabled, negativeDefaultsApplied] = await Promise.all([
    invoke('get_credentials'),
    invoke('get_relay_address'),
    invoke('cfg_get', { key: 'passphrase' }),
    invoke('cfg_get', { key: 'disabledReactionIds' }),
    invoke('cfg_get', { key: 'negativeReactionsDefaultApplied' }),
  ]);
  credentials = creds;
  passphraseInputEl.value = typeof savedPassphrase === 'string' ? savedPassphrase : '';
  disabledReactionIds = Array.isArray(savedDisabled) ? savedDisabled : [];
  // ネガティブリアクションの既定OFF化は「配列かどうか」では判定しない
  // (以前はそう判定していたが、この機能が無かった旧バージョンで既に
  // disabledReactionIdsが保存されていた場合に発動せず、ネガティブ項目が
  // 既定でONのまま表示されてしまう不具合があった)。専用のフラグを1回だけ
  // 見て、まだ適用していなければ、その時点でまだ触られていないネガティブ項目
  // だけをOFF側に追加する(ユーザーが既に行った他の設定は壊さない)。
  if (negativeDefaultsApplied !== true) {
    let changed = false;
    for (const id of DEFAULT_DISABLED_REACTION_IDS) {
      if (!disabledReactionIds.includes(id)) {
        disabledReactionIds.push(id);
        changed = true;
      }
    }
    await invoke('cfg_set', { key: 'negativeReactionsDefaultApplied', value: true });
    if (changed) {
      await persistDisabledReactionIds();
    }
  }
  renderStatus({
    relayConnected: false,
    viewerCount: 0,
    relayHost: relayAddress.host,
    relayPort: relayAddress.port,
  });
  loadEmojiTestButtons();
  renderReactionToggleList();
  await loadMonitorSelect();
  await loadGlyphSize();
  await loadGlyphOpacity();
  await loadComboGrowth();
  await connectRelay(credentials, relayAddress, passphraseInputEl.value);
  checkForUpdateOnStartup();
})();
