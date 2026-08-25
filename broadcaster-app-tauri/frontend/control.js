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
const listen = window.__TAURI__.event.listen;

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
const noComboGrowthListEl = document.getElementById('noComboGrowthList');
const cooldownRangeEl = document.getElementById('cooldownRange');
const cooldownValueEl = document.getElementById('cooldownValue');
const cooldownResetBtn = document.getElementById('cooldownResetBtn');
const displayRateRangeEl = document.getElementById('displayRateRange');
const displayRateValueEl = document.getElementById('displayRateValue');
const displayRateResetBtn = document.getElementById('displayRateResetBtn');
const hideLocalOverlayCheckboxEl = document.getElementById('hideLocalOverlayCheckbox');
const regionEnabledCheckboxEl = document.getElementById('regionEnabledCheckbox');
const regionFieldsEl = document.getElementById('regionFields');
const regionXRangeEl = document.getElementById('regionXRange');
const regionXValueEl = document.getElementById('regionXValue');
const regionYRangeEl = document.getElementById('regionYRange');
const regionYValueEl = document.getElementById('regionYValue');
const regionWidthRangeEl = document.getElementById('regionWidthRange');
const regionWidthValueEl = document.getElementById('regionWidthValue');
const regionHeightRangeEl = document.getElementById('regionHeightRange');
const regionHeightValueEl = document.getElementById('regionHeightValue');
const regionResetBtn = document.getElementById('regionResetBtn');
const regionPickBtn = document.getElementById('regionPickBtn');

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
const updateStatusOverlay = document.getElementById('updateStatusOverlay');
const updateStatusMessageEl = document.getElementById('updateStatusMessage');

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

  // OKを押した後、ダウンロードが終わる(=アプリが一旦終了する)までの間、
  // 画面が固まったように見えて不安になる、という指摘があったため、
  // せめて「今ダウンロード中である」ことだけは分かるようにしておく
  // (進捗%までは出せないが、無反応に見えるよりはよい)。
  updateStatusMessageEl.textContent =
    '新しいバージョンをダウンロード中です。しばらくお待ちください…\n(数十秒〜数分かかることがあります。完了すると自動的にアプリが再起動します)';
  updateStatusOverlay.classList.add('visible');

  try {
    await invoke('download_and_apply_update', {
      downloadUrl: info.download_url,
      sha256: info.sha256 || null,
    });
    // 成功時はRust側でapp.exit(0)が呼ばれてこのプロセスごと終了するため、
    // 通常ここには到達しない(到達したらそれ自体が想定外なので、念のため
    // オーバーレイは表示したままにしておく)。
  } catch (err) {
    updateStatusOverlay.classList.remove('visible');
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

// 「連打しても大きくしない」に個別指定されているリアクションIDの一覧
// (既定は空配列=全リアクションが対象=従来通り全て大きくなる)。
// comboGrowthCheckboxEl(全体ON/OFF)とは別軸の設定。
let noComboGrowthIds = [];

// クールタイム(秒。既定0.5秒=以前のReaTap側クリック間隔と同じ最低値、
// 最大5.0秒)。同じリアクションが短い間隔で連続して届いた場合、この秒数
// より短い間隔になる分は表示しない(間引く)。表示率(displayRatePercent、
// 下記)が「割合で間引く」のに対し、こちらは「間隔で間引く」形の別の
// 賑やかさ調整機能。テスト送信ボタン(loadEmojiTestButtons)はこちらも
// 対象外(動作確認は常に確実に表示したいため)。
let cooldownSec = 0.5;
const lastShownAtByEmoji = new Map();

// 表示率(既定100=間引きなし)。視聴者からのリアクションが多すぎて画面が
// 賑やかすぎる、という要望への対応。実際に受信したリアクション1件ごとに
// この確率で表示するかどうかを抽選する、単純な間引き(サンプリング)方式。
// 100%なら従来通り全件表示、1%ならおおよそ100件に1件だけ表示される。
// テスト送信ボタン(loadEmojiTestButtons)はこの間引きの対象外(動作確認は
// 常に確実に表示したいため)。
let displayRatePercent = 100;

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
    btn.title = emoji.label;
    if (emoji.rainbow) {
      // 神ｗ専用の「ゲーミングカラー」。bar.js/overlay.jsと同じく、ボタン自体の
      // 背景と文字のグラデーションが競合しないよう内側のspanに分離する。
      const textSpan = document.createElement('span');
      textSpan.className = 'rainbow-text';
      textSpan.textContent = emoji.char;
      btn.appendChild(textSpan);
    } else {
      btn.textContent = emoji.char;
      // 「草」のような、絵文字ではなく普通の文字として色指定が要る項目用
      if (emoji.color) {
        btn.style.color = emoji.color;
        btn.style.fontWeight = '900';
      }
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

function isComboGrowthAllowed(emojiId) {
  return !noComboGrowthIds.includes(emojiId);
}

async function persistNoComboGrowthIds() {
  await invoke('set_overlay_no_combo_growth_ids', { ids: noComboGrowthIds });
}

/** 1個分の「大きくしない」チップ要素を作る(チェック済み=大きくなる、が既定)。
 * buildReactionChipとほぼ同じ構造だが、対象の配列・保存先コマンドが別。 */
function buildNoComboGrowthChip(emoji) {
  const allowed = isComboGrowthAllowed(emoji.id);
  const chip = document.createElement('label');
  chip.className = 'rt-chip' + (allowed ? '' : ' rt-off');

  const checkbox = document.createElement('input');
  checkbox.type = 'checkbox';
  checkbox.checked = allowed;
  checkbox.addEventListener('change', async () => {
    if (checkbox.checked) {
      noComboGrowthIds = noComboGrowthIds.filter((id) => id !== emoji.id);
    } else if (!noComboGrowthIds.includes(emoji.id)) {
      noComboGrowthIds.push(emoji.id);
    }
    await persistNoComboGrowthIds();
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

/** 「連打で大きくするリアクション」のチップ一覧を描画する(既定は全てチェック
 * 済み=全て大きくなる)。受け付けるリアクション一覧と違い、ネガティブ
 * リアクション扱いでの区分けは不要なため、全件を1つのグループにまとめる。 */
function renderNoComboGrowthList() {
  const emojiSet = window.EmojiSet.EMOJI_SET;
  noComboGrowthListEl.innerHTML = '';
  const group = document.createElement('div');
  group.className = 'rt-group';
  for (const emoji of emojiSet) {
    group.appendChild(buildNoComboGrowthChip(emoji));
  }
  noComboGrowthListEl.appendChild(group);
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

/** 「連打で大きくするリアクション」チップ一覧の初期値を復元する。 */
async function loadNoComboGrowth() {
  const s = await invoke('get_overlay_settings');
  noComboGrowthIds = Array.isArray(s.noComboGrowthIds) ? s.noComboGrowthIds : [];
}

/** 「クールタイム」欄の初期値を、保存されている設定(無ければ既定0.5秒)から
 * 復元する。glyphScale/glyphOpacityと同じ構造(保存→Rustコマンド経由で
 * Tauriオーバーレイ・OBS側の設定には影響しない。displayRatePercentと同じく
 * このコントロールパネル自身の間引き判定にしか使わないため、専用の
 * Rustコマンドは持たず、cfg_get/cfg_setだけで完結させている)。 */
async function loadCooldown() {
  const saved = await invoke('cfg_get', { key: 'overlayCooldownSec' });
  const sec =
    typeof saved === 'number' && Number.isFinite(saved) ? Math.min(5, Math.max(0.5, saved)) : 0.5;
  cooldownSec = sec;
  cooldownRangeEl.value = String(sec);
  cooldownValueEl.textContent = `${sec.toFixed(1)}秒`;
}

let cooldownDebounceTimer = null;
cooldownRangeEl.addEventListener('input', () => {
  const sec = Number(cooldownRangeEl.value);
  cooldownValueEl.textContent = `${sec.toFixed(1)}秒`;
  cooldownSec = sec;
  clearTimeout(cooldownDebounceTimer);
  cooldownDebounceTimer = setTimeout(() => {
    invoke('cfg_set', { key: 'overlayCooldownSec', value: sec });
  }, 120);
});

cooldownResetBtn.addEventListener('click', () => {
  cooldownSec = 0.5;
  cooldownRangeEl.value = '0.5';
  cooldownValueEl.textContent = '0.5秒';
  invoke('cfg_set', { key: 'overlayCooldownSec', value: 0.5 });
});

/** 「表示率」欄の初期値を、保存されている設定(無ければ既定100%)から復元する。
 * glyphScale/glyphOpacityと違い、Tauriオーバーレイ・OBS側に反映する値ではなく
 * このコントロールパネル自身(relayClient受信時の間引き判定)でしか使わない
 * ため、専用のRustコマンドは持たず、汎用のcfg_get/cfg_setだけで完結させている。 */
async function loadDisplayRate() {
  const saved = await invoke('cfg_get', { key: 'overlayDisplayRate' });
  const percent =
    typeof saved === 'number' && Number.isFinite(saved) ? Math.min(100, Math.max(1, Math.round(saved))) : 100;
  displayRatePercent = percent;
  displayRateRangeEl.value = String(percent);
  displayRateValueEl.textContent = `${percent}%`;
}

let displayRateDebounceTimer = null;
displayRateRangeEl.addEventListener('input', () => {
  const percent = Number(displayRateRangeEl.value);
  displayRateValueEl.textContent = `${percent}%`;
  displayRatePercent = percent;
  clearTimeout(displayRateDebounceTimer);
  displayRateDebounceTimer = setTimeout(() => {
    invoke('cfg_set', { key: 'overlayDisplayRate', value: percent });
  }, 120);
});

displayRateResetBtn.addEventListener('click', () => {
  displayRatePercent = 100;
  displayRateRangeEl.value = '100';
  displayRateValueEl.textContent = '100%';
  invoke('cfg_set', { key: 'overlayDisplayRate', value: 100 });
});

/** 「自分の画面には表示しない(OBS経由の配信画面にのみ表示)」チェックボックスの
 * 初期値を復元する。emit_overlay_reaction(Rust側)がこの設定を見て、Tauri本体の
 * オーバーレイウィンドウへの反映だけを省略する(OBSブリッジへの配信は常に行う)。 */
async function loadHideLocalOverlay() {
  const saved = await invoke('cfg_get', { key: 'hideLocalOverlay' });
  hideLocalOverlayCheckboxEl.checked = saved === true;
}

hideLocalOverlayCheckboxEl.addEventListener('change', () => {
  invoke('cfg_set', { key: 'hideLocalOverlay', value: hideLocalOverlayCheckboxEl.checked });
});

/** 「モニターの一部の範囲だけに表示する」欄の初期値を復元する。座標・サイズは
 * 選んだモニターに対する割合(0〜100%)で保持している(解像度が変わっても
 * 破綻しないようにするため)。 */
async function loadRegionSettings() {
  const [enabled, x, y, width, height] = await Promise.all([
    invoke('cfg_get', { key: 'overlayRegionEnabled' }),
    invoke('cfg_get', { key: 'overlayRegionX' }),
    invoke('cfg_get', { key: 'overlayRegionY' }),
    invoke('cfg_get', { key: 'overlayRegionWidth' }),
    invoke('cfg_get', { key: 'overlayRegionHeight' }),
  ]);
  const xPercent = typeof x === 'number' ? Math.round(x * 100) : 0;
  const yPercent = typeof y === 'number' ? Math.round(y * 100) : 0;
  const widthPercent = typeof width === 'number' ? Math.round(width * 100) : 100;
  const heightPercent = typeof height === 'number' ? Math.round(height * 100) : 100;

  regionEnabledCheckboxEl.checked = enabled === true;
  regionFieldsEl.classList.toggle('disabled', enabled !== true);
  regionXRangeEl.value = String(xPercent);
  regionXValueEl.textContent = `${xPercent}%`;
  regionYRangeEl.value = String(yPercent);
  regionYValueEl.textContent = `${yPercent}%`;
  regionWidthRangeEl.value = String(widthPercent);
  regionWidthValueEl.textContent = `${widthPercent}%`;
  regionHeightRangeEl.value = String(heightPercent);
  regionHeightValueEl.textContent = `${heightPercent}%`;
}

/** 今の4つのスライダーの値を、Rust側(set_overlay_region)へまとめて送る。
 * Rust側で0〜95%(位置)・5〜100%(サイズ)へのクランプと、位置+サイズが
 * モニターをはみ出さないよう調整も行うので、ここでは単純に値を渡すだけでよい。 */
function applyRegionSettings() {
  invoke('set_overlay_region', {
    enabled: regionEnabledCheckboxEl.checked,
    x: Number(regionXRangeEl.value) / 100,
    y: Number(regionYRangeEl.value) / 100,
    width: Number(regionWidthRangeEl.value) / 100,
    height: Number(regionHeightRangeEl.value) / 100,
  });
}

regionEnabledCheckboxEl.addEventListener('change', () => {
  regionFieldsEl.classList.toggle('disabled', !regionEnabledCheckboxEl.checked);
  applyRegionSettings();
});

let regionDebounceTimer = null;
function onRegionRangeInput(valueEl, rangeEl, suffix) {
  valueEl.textContent = `${rangeEl.value}${suffix}`;
  clearTimeout(regionDebounceTimer);
  regionDebounceTimer = setTimeout(applyRegionSettings, 150);
}
regionXRangeEl.addEventListener('input', () => onRegionRangeInput(regionXValueEl, regionXRangeEl, '%'));
regionYRangeEl.addEventListener('input', () => onRegionRangeInput(regionYValueEl, regionYRangeEl, '%'));
regionWidthRangeEl.addEventListener('input', () => onRegionRangeInput(regionWidthValueEl, regionWidthRangeEl, '%'));
regionHeightRangeEl.addEventListener('input', () => onRegionRangeInput(regionHeightValueEl, regionHeightRangeEl, '%'));

regionResetBtn.addEventListener('click', () => {
  regionXRangeEl.value = '0';
  regionXValueEl.textContent = '0%';
  regionYRangeEl.value = '0';
  regionYValueEl.textContent = '0%';
  regionWidthRangeEl.value = '100';
  regionWidthValueEl.textContent = '100%';
  regionHeightRangeEl.value = '100';
  regionHeightValueEl.textContent = '100%';
  regionEnabledCheckboxEl.checked = false;
  regionFieldsEl.classList.add('disabled');
  applyRegionSettings();
});

/** 「範囲指定(ドラッグで選ぶ)」ボタン。専用ウィンドウ(region_picker.js)を開く
 * だけで、実際の保存・オーバーレイへの反映はそちら側がset_overlay_regionを
 * 直接呼んで完結させる。この画面は下のlisten('overlay-region-picked', ...)で
 * 確定した値を受け取り、スライダー表示だけを同期する。 */
regionPickBtn.addEventListener('click', () => {
  invoke('open_region_picker').catch(() => {
    // 無視(モニター情報が取得できない等の稀なケース。スライダーでの
    // 手動指定は引き続き使えるため、致命的ではない)
  });
});

/** region_picker.jsがドラッグでの範囲確定後にブロードキャストしてくる
 * イベント。値は既にRust側(set_overlay_region)へ保存・反映済みなので、
 * ここではスライダーの見た目をその値に合わせるだけでよい(loadRegionSettings
 * と同じ表示ロジックを使い回す)。 */
listen('overlay-region-picked', (event) => {
  const { x, y, width, height } = event.payload || {};
  const xPercent = typeof x === 'number' ? Math.round(x * 100) : 0;
  const yPercent = typeof y === 'number' ? Math.round(y * 100) : 0;
  const widthPercent = typeof width === 'number' ? Math.round(width * 100) : 100;
  const heightPercent = typeof height === 'number' ? Math.round(height * 100) : 100;

  regionEnabledCheckboxEl.checked = true;
  regionFieldsEl.classList.remove('disabled');
  regionXRangeEl.value = String(xPercent);
  regionXValueEl.textContent = `${xPercent}%`;
  regionYRangeEl.value = String(yPercent);
  regionYValueEl.textContent = `${yPercent}%`;
  regionWidthRangeEl.value = String(widthPercent);
  regionWidthValueEl.textContent = `${widthPercent}%`;
  regionHeightRangeEl.value = String(heightPercent);
  regionHeightValueEl.textContent = `${heightPercent}%`;
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
    // 表示率による間引き。100%未満の場合、受信したリアクション1件ごとに
    // その確率でだけ実際に表示する(視聴者への通知や連打防止判定は中継
    // サーバー側で完結しているので、ここで間引いても視聴者側には一切
    // 影響しない。あくまで配信画面の見た目を賑やかにしすぎない目的の機能)。
    if (displayRatePercent < 100 && Math.random() * 100 >= displayRatePercent) return;
    // クールタイムによる間引き。表示率(確率)とは別軸で、「同じリアクションが
    // これ以上短い間隔で連続表示されないようにする」形の間引き。テスト送信
    // ボタン(loadEmojiTestButtons)経由のemit_overlay_reaction呼び出しはこの
    // ハンドラを通らないため、既定通りクールタイムの対象外になる。
    const now = Date.now();
    const lastShownAt = lastShownAtByEmoji.get(m.emoji) || 0;
    if (now - lastShownAt < cooldownSec * 1000) return;
    lastShownAtByEmoji.set(m.emoji, now);
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
  await loadRegionSettings();
  await loadGlyphSize();
  await loadGlyphOpacity();
  await loadDisplayRate();
  await loadComboGrowth();
  await loadNoComboGrowth();
  renderNoComboGrowthList();
  await loadCooldown();
  await loadHideLocalOverlay();
  await connectRelay(credentials, relayAddress, passphraseInputEl.value);
  checkForUpdateOnStartup();
})();
