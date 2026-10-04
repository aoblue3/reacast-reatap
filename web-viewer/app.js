'use strict';
/**
 * ReaTap Web: exeのインストールを不要にした、ブラウザだけで完結する軽量版。
 *
 * Tauri版(viewer-app-tauri)のbar.js/main.jsと役割はほぼ同じだが、対象の
 * 配信ソフトウィンドウへの自動追従・フォーカス制御など、OSのネイティブAPIに
 * 依存する機能(pcwmp.rs経由の処理)は持たない。このページはブラウザの別タブ
 * /別ウィンドウとして開いたまま、自分でタップして使う運用を想定している。
 *
 * 通信部分(RelayClient)・絵文字データ(EmojiSet)はTauri版と全く同じファイルを
 * そのまま使っている(shared/参照。どちらも元々ブラウザ標準API/純粋なデータ
 * だけで書かれており、Tauriへの依存が無いため)。
 *
 * 設定(合言葉の履歴・リアクション並び順・表示/非表示・アイコンの大きさ)は
 * Tauri版のようなファイル保存(ConfigStore)の代わりに、このブラウザの
 * localStorageに保存する。他の端末やブラウザとは共有されない(端末ごとの
 * 設定になる)。
 */

const LS_PREFIX = 'reatapweb.';
const DEBOUNCE_MS = 500;
const PASSPHRASE_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{2,31}$/; // relay-server/server.jsのPASSPHRASE_REと同じ

function lsGet(key, fallback) {
  try {
    const raw = localStorage.getItem(LS_PREFIX + key);
    if (raw === null) return fallback;
    return JSON.parse(raw);
  } catch {
    return fallback;
  }
}
function lsSet(key, value) {
  try {
    localStorage.setItem(LS_PREFIX + key, JSON.stringify(value));
  } catch {
    // 無視(プライベートブラウジング等でlocalStorageが使えなくても、
    // 今回の接続中にタップしてリアクションを送ること自体はできる。
    // 設定や合言葉の履歴が次回に引き継がれないだけ)
  }
}

/* ------------------------- 要素参照 ------------------------- */
const connectScreen = document.getElementById('connectScreen');
const reactionScreen = document.getElementById('reactionScreen');
const passphraseForm = document.getElementById('passphraseForm');
const passphraseInput = document.getElementById('passphraseInput');
const errorMsgEl = document.getElementById('errorMsg');
const savedWrap = document.getElementById('savedWrap');
const savedListEl = document.getElementById('savedList');
const relayHostInput = document.getElementById('relayHostInput');
const relayPortInput = document.getElementById('relayPortInput');
const connLabelEl = document.getElementById('connLabel');
const gridEl = document.getElementById('grid');
const muteNoticeEl = document.getElementById('muteNotice');
const openSettingsBtn = document.getElementById('openSettingsBtn');
const closeSettingsBtn = document.getElementById('closeSettingsBtn');
const disconnectBtn = document.getElementById('disconnectBtn');
const settingsOverlay = document.getElementById('settingsOverlay');
const iconScaleRange = document.getElementById('iconScaleRange');
const reactionOrderListEl = document.getElementById('reactionOrderList');
const resetOrderBtn = document.getElementById('resetOrderBtn');

/* ------------------------- 状態 ------------------------- */
let relayClient = null;
let currentPassphrase = '';
let lastSentAt = 0;
let mutedUntil = 0;
let muteTimer = null;

let reactionOrder = lsGet('reactionOrder', null); // null = 未設定(EMOJI_SET通りの順)
let hiddenReactionIds = lsGet('hiddenReactionIds', []);
let iconScale = lsGet('iconScale', 1);

/* ------------------------- 並び順(Tauri版main.js/bar.jsと同じロジック) ------------------------- */

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

function persistReactionOrder() {
  lsSet('reactionOrder', reactionOrder);
}
function persistHiddenReactionIds() {
  lsSet('hiddenReactionIds', hiddenReactionIds);
}

let dragFromIndex = null;

function applyReactionReorder(fromIndex, targetIndex, insertAfter) {
  const arr = getEffectiveOrder();
  if (fromIndex < 0 || fromIndex >= arr.length) return;
  const [item] = arr.splice(fromIndex, 1);
  let insertIndex = targetIndex;
  if (fromIndex < targetIndex) insertIndex -= 1;
  if (insertAfter) insertIndex += 1;
  insertIndex = Math.max(0, Math.min(arr.length, insertIndex));
  arr.splice(insertIndex, 0, item);
  reactionOrder = arr;
  persistReactionOrder();
  renderReactionOrderList();
  renderGrid();
}

function reorderReactionByPosition(fromIndex, newPosition1Based) {
  const arr = getEffectiveOrder();
  const clamped = Math.max(1, Math.min(arr.length, Math.round(newPosition1Based) || 1));
  applyReactionReorder(fromIndex, clamped - 1, false);
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
    row.dataset.index = String(index);

    const handle = document.createElement('span');
    handle.className = 'ro-handle';
    handle.textContent = '⠿';
    handle.title = 'ドラッグして並び替え';
    handle.draggable = true;
    handle.addEventListener('dragstart', (e) => {
      dragFromIndex = index;
      row.classList.add('ro-dragging');
      e.dataTransfer.effectAllowed = 'move';
      try {
        e.dataTransfer.setData('text/plain', String(index));
      } catch {
        // 無視(dragFromIndexで代替できる)
      }
    });
    handle.addEventListener('dragend', () => {
      dragFromIndex = null;
      row.classList.remove('ro-dragging');
      reactionOrderListEl
        .querySelectorAll('.ro-row.ro-drop-before, .ro-row.ro-drop-after')
        .forEach((el) => el.classList.remove('ro-drop-before', 'ro-drop-after'));
    });
    row.appendChild(handle);

    row.addEventListener('dragover', (e) => {
      if (dragFromIndex === null) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      const rect = row.getBoundingClientRect();
      const insertAfter = e.clientY - rect.top > rect.height / 2;
      row.classList.toggle('ro-drop-before', !insertAfter);
      row.classList.toggle('ro-drop-after', insertAfter);
    });
    row.addEventListener('dragleave', () => {
      row.classList.remove('ro-drop-before', 'ro-drop-after');
    });
    row.addEventListener('drop', (e) => {
      e.preventDefault();
      if (dragFromIndex === null) return;
      const rect = row.getBoundingClientRect();
      const insertAfter = e.clientY - rect.top > rect.height / 2;
      const fromIndex = dragFromIndex;
      dragFromIndex = null;
      row.classList.remove('ro-drop-before', 'ro-drop-after');
      applyReactionReorder(fromIndex, index, insertAfter);
    });

    const visLabel = document.createElement('label');
    visLabel.style.margin = '0';
    visLabel.style.display = 'flex';
    visLabel.style.alignItems = 'center';
    const visCheckbox = document.createElement('input');
    visCheckbox.type = 'checkbox';
    visCheckbox.checked = !hidden;
    visCheckbox.addEventListener('change', () => {
      if (visCheckbox.checked) {
        hiddenReactionIds = hiddenReactionIds.filter((x) => x !== id);
      } else if (!hiddenReactionIds.includes(id)) {
        hiddenReactionIds.push(id);
      }
      persistHiddenReactionIds();
      renderReactionOrderList();
      renderGrid();
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

    const posInput = document.createElement('input');
    posInput.type = 'number';
    posInput.className = 'ro-pos-input';
    posInput.min = '1';
    posInput.max = String(order.length);
    posInput.value = String(index + 1);
    posInput.title = 'この番号の位置に挿入します';
    const applyPosInput = () => {
      const newPos = Number(posInput.value);
      if (!Number.isFinite(newPos) || Math.round(newPos) === index + 1) {
        posInput.value = String(index + 1);
        return;
      }
      reorderReactionByPosition(index, newPos);
    };
    posInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') posInput.blur();
    });
    posInput.addEventListener('blur', applyPosInput);
    row.appendChild(posInput);

    const upBtn = document.createElement('button');
    upBtn.className = 'secondary ro-updown';
    upBtn.textContent = '↑';
    upBtn.disabled = index === 0;
    upBtn.addEventListener('click', () => {
      const arr = getEffectiveOrder();
      if (index <= 0) return;
      [arr[index - 1], arr[index]] = [arr[index], arr[index - 1]];
      reactionOrder = arr;
      persistReactionOrder();
      renderReactionOrderList();
      renderGrid();
    });
    row.appendChild(upBtn);

    const downBtn = document.createElement('button');
    downBtn.className = 'secondary ro-updown';
    downBtn.textContent = '↓';
    downBtn.disabled = index === order.length - 1;
    downBtn.addEventListener('click', () => {
      const arr = getEffectiveOrder();
      if (index >= arr.length - 1) return;
      [arr[index + 1], arr[index]] = [arr[index], arr[index + 1]];
      reactionOrder = arr;
      persistReactionOrder();
      renderReactionOrderList();
      renderGrid();
    });
    row.appendChild(downBtn);

    reactionOrderListEl.appendChild(row);
  });
}

/* ------------------------- アイコンの大きさ ------------------------- */

function applyIconScale() {
  const s = Number.isFinite(iconScale) && iconScale > 0 ? Math.min(1.8, Math.max(0.7, iconScale)) : 1;
  document.documentElement.style.setProperty('--icon-scale', String(s));
  iconScaleRange.value = String(s);
}

iconScaleRange.addEventListener('input', () => {
  iconScale = Number(iconScaleRange.value) || 1;
  document.documentElement.style.setProperty('--icon-scale', String(iconScale));
});
iconScaleRange.addEventListener('change', () => {
  lsSet('iconScale', iconScale);
});

resetOrderBtn.addEventListener('click', () => {
  reactionOrder = null;
  hiddenReactionIds = [];
  persistReactionOrder();
  persistHiddenReactionIds();
  renderReactionOrderList();
  renderGrid();
});

/* ------------------------- リアクションボタンの描画 ------------------------- */

function onClickEmoji(emojiId) {
  const now = Date.now();
  if (now < mutedUntil) return;
  if (now - lastSentAt < DEBOUNCE_MS) return;
  lastSentAt = now;
  if (relayClient) relayClient.send({ type: 'reaction', emoji: emojiId });
  // Web版はブラウザのタブとして独立して開く運用のため、Tauri版のような
  // 「配信ソフト側へのフォーカス戻し」処理は存在しない(そもそも他の
  // ネイティブウィンドウにフォーカスを移す手段がブラウザには無いため)。
}

function renderGrid() {
  const emojiSet = getVisibleOrderedEmojiSet();
  gridEl.innerHTML = '';
  for (const emoji of emojiSet) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'emoji-btn';
    btn.dataset.emojiId = emoji.id;
    if (emoji.rainbow) {
      btn.classList.add('emoji-btn-textreaction');
      const textSpan = document.createElement('span');
      textSpan.className = 'rainbow-text';
      textSpan.textContent = emoji.char;
      btn.appendChild(textSpan);
    } else {
      btn.textContent = emoji.char;
      if (emoji.color) {
        btn.classList.add('emoji-btn-textreaction');
        btn.style.color = emoji.color;
        btn.style.textShadow = `0 0 6px ${emoji.color}, 0 1px 2px rgba(0,0,0,0.75)`;
      }
    }
    btn.title = emoji.label;
    btn.addEventListener('click', () => onClickEmoji(emoji.id));
    gridEl.appendChild(btn);
  }
}

/* ------------------------- ミュート表示 ------------------------- */

function applyMuteUI() {
  const buttons = gridEl.querySelectorAll('.emoji-btn');
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

/* ------------------------- 合言葉の履歴(localStorage) ------------------------- */

function getSavedPassphrases() {
  const list = lsGet('savedPassphrases', []);
  return Array.isArray(list) ? list : [];
}

function touchSavedPassphrase(passphrase) {
  const normalized = passphrase.toLowerCase();
  let list = getSavedPassphrases().filter((p) => p.passphrase !== normalized);
  list.unshift({ passphrase: normalized, lastUsedAt: Date.now() });
  list = list.slice(0, 8); // 増えすぎないよう直近8件まで
  lsSet('savedPassphrases', list);
}

function removeSavedPassphrase(passphrase) {
  const list = getSavedPassphrases().filter((p) => p.passphrase !== passphrase);
  lsSet('savedPassphrases', list);
  renderSavedList();
}

function renderSavedList() {
  const list = getSavedPassphrases();
  savedWrap.style.display = list.length ? '' : 'none';
  savedListEl.innerHTML = '';
  for (const entry of list) {
    const row = document.createElement('div');
    row.className = 'saved-row';

    const label = document.createElement('span');
    label.className = 'saved-passphrase';
    label.textContent = entry.passphrase;
    row.appendChild(label);

    const connectBtn = document.createElement('button');
    connectBtn.type = 'button';
    connectBtn.textContent = '接続';
    connectBtn.addEventListener('click', () => startConnect(entry.passphrase));
    row.appendChild(connectBtn);

    const removeBtn = document.createElement('button');
    removeBtn.type = 'button';
    removeBtn.className = 'secondary';
    removeBtn.textContent = '削除';
    removeBtn.addEventListener('click', () => removeSavedPassphrase(entry.passphrase));
    row.appendChild(removeBtn);

    savedListEl.appendChild(row);
  }
}

/* ------------------------- 中継サーバーへの接続 ------------------------- */

function resolveRelayUrl() {
  const hostOverride = lsGet('relayHostOverride', '');
  const portOverride = lsGet('relayPortOverride', '');
  if (hostOverride) {
    const port = portOverride ? `:${portOverride}` : '';
    return `ws://${hostOverride}${port}`;
  }
  // 既定: このページを配信しているサーバーと同じホスト・同じポートに
  // WebSocketで接続する(中継サーバー自身がこのページも配信している場合、
  // これだけでアドレス設定が一切不要になる)。
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}`;
}

function showScreen(screenEl) {
  for (const s of document.querySelectorAll('.screen')) s.classList.remove('active');
  screenEl.classList.add('active');
}

function setErrorMsg(text) {
  errorMsgEl.textContent = text || '';
}

function startConnect(rawPassphrase) {
  const passphrase = (rawPassphrase || '').trim();
  if (!PASSPHRASE_RE.test(passphrase)) {
    setErrorMsg('合言葉は英数字で始まる3〜32文字(英数字・ハイフン・アンダースコアのみ)で入力してください');
    return;
  }
  setErrorMsg('');

  if (relayClient) {
    relayClient.close();
    relayClient = null;
  }

  currentPassphrase = passphrase;
  const url = resolveRelayUrl();
  relayClient = new window.RelayClient({ url, hello: { type: 'join', passphrase } });

  relayClient.on('type:joined', () => {
    touchSavedPassphrase(passphrase);
    connLabelEl.textContent = `接続中: ${passphrase.toLowerCase()}`;
    document.body.style.opacity = '1';
    renderGrid();
    showScreen(reactionScreen);
  });

  relayClient.on('type:error', (msg) => {
    if (msg.code === 'room_not_found' || msg.code === 'invalid_params' || msg.code === 'outdated_app') {
      // まだ「リアクション画面」に切り替わっていない(=接続確立前)場合のみ、
      // 接続画面側にエラーを出す。接続済み状態での一時的なエラーで画面ごと
      // 戻してしまうと、再接続中に毎回接続画面へ引き戻されてしまうため。
      if (!reactionScreen.classList.contains('active')) {
        setErrorMsg(msg.message || '接続に失敗しました');
        relayClient.close();
        relayClient = null;
      }
    }
  });

  relayClient.on('type:muted', (m) => {
    mutedUntil = m.untilMs;
    applyMuteUI();
  });

  relayClient.on('open', () => {
    document.body.style.opacity = '1';
  });
  relayClient.on('close', () => {
    // 接続画面にまだいる場合(=最初の接続確立前に切れた)は見た目を暗くする
    // 必要は無い。接続済み画面にいる間だけ「再接続中」の見た目にする。
    if (reactionScreen.classList.contains('active')) {
      document.body.style.opacity = '0.6';
    }
  });

  relayClient.connect();
}

function disconnect() {
  if (relayClient) {
    relayClient.close();
    relayClient = null;
  }
  document.body.style.opacity = '1';
  currentPassphrase = '';
  showScreen(connectScreen);
  renderSavedList();
}

/* ------------------------- イベント配線 ------------------------- */

passphraseForm.addEventListener('submit', (e) => {
  e.preventDefault();
  startConnect(passphraseInput.value);
});

disconnectBtn.addEventListener('click', disconnect);

openSettingsBtn.addEventListener('click', () => {
  renderReactionOrderList();
  settingsOverlay.classList.add('active');
});
closeSettingsBtn.addEventListener('click', () => {
  settingsOverlay.classList.remove('active');
});
settingsOverlay.addEventListener('click', (e) => {
  if (e.target === settingsOverlay) settingsOverlay.classList.remove('active');
});

relayHostInput.addEventListener('change', () => {
  lsSet('relayHostOverride', relayHostInput.value.trim());
});
relayPortInput.addEventListener('change', () => {
  lsSet('relayPortOverride', relayPortInput.value.trim());
});

/* ------------------------- 初期化 ------------------------- */

function init() {
  relayHostInput.value = lsGet('relayHostOverride', '');
  relayPortInput.value = lsGet('relayPortOverride', '');
  applyIconScale();
  renderSavedList();
  renderGrid();
}

init();
