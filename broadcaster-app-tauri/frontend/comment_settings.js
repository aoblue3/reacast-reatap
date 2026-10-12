'use strict';
/**
 * コメント読み上げ・字幕の設定ウィンドウ。
 *
 * 入力欄はdata-key属性で設定の項目名(ネストは"style.subtitle.fontSize"の
 * ようにドット区切り)と結び付けてあり、変更されたら少し待ってから設定
 * 全体をまとめてRust側(comments_save_settings)に保存する。保存すると
 * 見た目の設定はOBS・デスクトップ字幕の画面にもその場で反映される。
 */

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const C = window.CommentCommon;

let settings = null;
let saveTimer = null;

const fields = Array.from(document.querySelectorAll('[data-key]'));
const statusEl = document.getElementById('status');
const startBtn = document.getElementById('startBtn');
const stopBtn = document.getElementById('stopBtn');
const monitorSelect = document.getElementById('desktopMonitor');

function getPath(obj, path) {
  return path.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

function setPath(obj, path, value) {
  const keys = path.split('.');
  let o = obj;
  for (const k of keys.slice(0, -1)) {
    if (!o[k] || typeof o[k] !== 'object') o[k] = {};
    o = o[k];
  }
  o[keys[keys.length - 1]] = value;
}

function readField(el) {
  if (el.type === 'checkbox') return el.checked;
  if (el.type === 'number') {
    const n = Number(el.value);
    if (!Number.isFinite(n)) return undefined;
    return el.hasAttribute('data-percent') ? n / 100 : n;
  }
  return el.value;
}

function writeField(el, value) {
  if (el.type === 'checkbox') el.checked = !!value;
  else if (el.type === 'number') {
    const n = Number(value);
    el.value = Number.isFinite(n) ? String(el.hasAttribute('data-percent') ? Math.round(n * 1000) / 10 : n) : '';
  } else el.value = value == null ? '' : String(value);
}

function renderFields() {
  for (const el of fields) writeField(el, getPath(settings, el.dataset.key));
  renderHotkeys();
  monitorSelect.value = settings.desktopMonitorId || '';
  const r = settings.desktopRect;
  document.getElementById('desktopRectInfo').textContent = r
    ? `マウスで決めた位置: ${r.x}, ${r.y}(${r.width}×${r.height})`
    : '既定の位置(下の設定)';
  renderPreview();
}

/** 字幕のプレビュー。実際の字幕と同じく下端揃えで、文字揃え・位置の微調整・
 * 見出し(番号・名前・時刻)も反映する。 */
function renderPreview() {
  const s = C.mergeStyle(settings.style).subtitle;
  const box = document.getElementById('preview');
  const p = document.getElementById('previewText');
  p.style.fontFamily = C.cssFontFamily(s.fontFamily);
  p.style.fontSize = `${Math.min(80, Number(s.fontSize) || 30)}px`;
  p.style.fontWeight = s.bold ? 'bold' : 'normal';
  p.style.fontStyle = s.italic ? 'italic' : 'normal';
  p.style.color = s.color;
  p.style.textShadow = C.outlineShadow(s.outlineWidth, s.outlineColor);
  p.style.textAlign = s.align || 'center';
  p.style.writingMode = s.vertical ? 'vertical-rl' : '';
  p.style.transform = `translate(${Number(s.offsetX) || 0}px, ${Number(s.offsetY) || 0}px)`;
  box.style.justifyContent = s.align === 'left' ? 'flex-start' : s.align === 'right' ? 'flex-end' : 'center';
  box.style.alignItems = s.valign === 'bottom' ? 'flex-end' : 'flex-start';
  const sample = { no: 69, name: '名無し＠BBSさん', date: '2026/10/08(木) 21:00:00', body: 'プレビュー 123 ABC\n2行目のテキスト' };
  const head = C.resHeader(sample, { showResNumber: s.showResNumber, showName: s.showName, showDate: s.showTime });
  p.textContent = '';
  if (head) {
    const h = document.createElement('span');
    h.className = 'res-head';
    h.textContent = s.showResNumber ? head.replace(/^\d+/, `レス${sample.no}`) : head;
    p.appendChild(h);
  }
  p.appendChild(document.createTextNode(sample.body));
  renderListPreview();
}

// ---- 右のレス一覧のプレビュー(comments.html?previewを埋め込んで同じ描画をさせる) ----
const listPreview = document.getElementById('listPreview');
const LIST_SAMPLES = [
  { no: 67, name: '名無し＠BBSさん', date: '2026/10/08(木) 21:00:01.11 ID:abc', body: '水中にいるみたい', aa: false, icon: null },
  { no: 68, name: '名無し＠BBSさん', date: '2026/10/08(木) 21:00:05.22 ID:def', body: 'ちょっと長めのレスです。横幅を超えた時の折り返しや行間の確認用のテキストです。', aa: false, icon: null },
  { no: 69, name: '名無し＠BBSさん', date: '2026/10/08(木) 21:00:09.33 ID:ghi', body: 'おつかれ', aa: false, icon: null },
];
function renderListPreview() {
  if (!listPreview || !listPreview.contentWindow) return;
  listPreview.contentWindow.postMessage({ type: 'commentStyle', style: settings.style }, '*');
  listPreview.contentWindow.postMessage({ type: 'commentHistory', items: LIST_SAMPLES }, '*');
}
listPreview.addEventListener('load', renderListPreview);

document.getElementById('listTestBtn').addEventListener('click', () => {
  const text = document.getElementById('listTestText').value.trim();
  const name = document.getElementById('listTestName').value;
  if (text) invoke('comments_test_list', { text, name }).catch((e) => showStatusError(String(e)));
});

function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    invoke('comments_save_settings', { settings }).catch((e) => showStatusError(`設定の保存に失敗しました: ${e}`));
  }, 300);
}

function onFieldChange(el) {
  const v = readField(el);
  if (v === undefined) return;
  setPath(settings, el.dataset.key, v);
  renderPreview();
  scheduleSave();
}

for (const el of fields) {
  el.addEventListener(el.type === 'checkbox' || el.tagName === 'SELECT' ? 'change' : 'input', () => onFieldChange(el));
}

// ---- タブ ----
document.getElementById('tabs').addEventListener('click', (e) => {
  const btn = e.target.closest('button[data-tab]');
  if (!btn) return;
  for (const b of document.querySelectorAll('#tabs button')) b.classList.toggle('active', b === btn);
  for (const s of document.querySelectorAll('main section')) s.classList.toggle('active', s.dataset.tab === btn.dataset.tab);
});

// ---- 字幕位置の微調整 ----
for (const btn of document.querySelectorAll('[data-nudge]')) {
  btn.addEventListener('click', () => {
    const sub = C.mergeStyle(settings.style).subtitle;
    let x = Number(sub.offsetX) || 0;
    let y = Number(sub.offsetY) || 0;
    if (btn.dataset.nudge === 'reset') {
      x = 0;
      y = 0;
    } else {
      const [dx, dy] = btn.dataset.nudge.split(',').map(Number);
      x += dx;
      y += dy;
    }
    setPath(settings, 'style.subtitle.offsetX', x);
    setPath(settings, 'style.subtitle.offsetY', y);
    renderFields();
    scheduleSave();
  });
}

// ---- コピー ----
for (const btn of document.querySelectorAll('[data-copy]')) {
  btn.addEventListener('click', async () => {
    const text = document.getElementById(btn.dataset.copy).textContent;
    try {
      await navigator.clipboard.writeText(text);
      btn.textContent = 'コピーしました';
      setTimeout(() => (btn.textContent = 'コピー'), 1500);
    } catch {
      // 無視(手で選択してコピーできる)
    }
  });
}

// ---- 参照ボタン ----
document.getElementById('pickExeBtn').addEventListener('click', async () => {
  const path = await invoke('comments_pick_exe').catch(() => null);
  if (!path) return;
  settings.commandPath = path;
  renderFields();
  scheduleSave();
});
for (const btn of document.querySelectorAll('[data-pick-folder]')) {
  btn.addEventListener('click', async () => {
    const path = await invoke('comments_pick_folder').catch(() => null);
    if (!path) return;
    settings[btn.dataset.pickFolder] = path;
    renderFields();
    scheduleSave();
  });
}

// ---- 右のレス一覧のプリセット ----
// デザインに関わる項目だけを置き換える(フォント・件数・表示順などはそのまま)
const LIST_PRESETS = {
  default: {},
  yellowLine: {
    headFontSize: 14, headBold: true, numberColor: '#ff6060', nameColor: '#60c0ff', dateColor: '#bbbbbb',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 2, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 50, itemPadding: 6, itemRadius: 6,
    customCss: '.item { border-left: 4px solid #ffd23f; }',
  },
  bubble: {
    headFontSize: 13, headBold: true, numberColor: '#e05a00', nameColor: '#555555', dateColor: '#888888',
    bodyColor: '#222222', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#ffffff', itemBgOpacity: 90, itemPadding: 8, itemRadius: 14,
    customCss: '.item { box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35); }',
  },
  neon: {
    headFontSize: 14, headBold: true, numberColor: '#3cf2ff', nameColor: '#ff4fd8', dateColor: '#9a9ab8',
    bodyColor: '#ffffff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#120a24', itemBgOpacity: 60, itemPadding: 6, itemRadius: 4,
    customCss: '.body { text-shadow: 0 0 6px #3cf2ff, 0 0 12px #3cf2ff; }\n.item { border: 1px solid rgba(60, 242, 255, 0.4); }',
  },
};
const PRESET_KEYS = [
  'headFontSize', 'headBold', 'numberColor', 'nameColor', 'dateColor', 'bodyColor', 'bodyBold',
  'outlineWidth', 'outlineColor', 'itemBgColor', 'itemBgOpacity', 'itemPadding', 'itemRadius', 'customCss',
];
document.getElementById('applyPresetBtn').addEventListener('click', () => {
  const preset = LIST_PRESETS[document.getElementById('listPreset').value] || {};
  const defaults = C.DEFAULT_STYLE.list;
  for (const k of PRESET_KEYS) settings.style.list[k] = k in preset ? preset[k] : defaults[k];
  renderFields();
  scheduleSave();
});

// ---- デスクトップ字幕の位置調整 ----
const adjustBtn = document.getElementById('adjustDesktopBtn');
let adjusting = false;
function renderAdjust() {
  adjustBtn.textContent = adjusting ? '位置の調整を終了(保存)' : 'マウスで位置を調整する';
}
adjustBtn.addEventListener('click', async () => {
  if (!settings.desktopEnabled && !adjusting) {
    showStatusError('「デスクトップに字幕を表示する」をONにしてください');
    return;
  }
  try {
    await invoke('comments_desktop_adjust', { on: !adjusting });
  } catch (e) {
    showStatusError(String(e));
  }
});
document.getElementById('resetDesktopBtn').addEventListener('click', () => {
  invoke('comments_desktop_reset').catch((e) => showStatusError(String(e)));
});

// ---- 次スレの>>1のテンプレート編集(右側に建てた時の見え方をすぐ出す) ----
const templateEl = document.querySelector('[data-key="createBodyTemplate"]');
let draftTimer = null;

/** 入力途中の設定でタイトルと>>1を作って表示する(実際には建てない)。 */
async function renderDraft() {
  const text = templateEl.value;
  // 2ちゃんねる互換の掲示板の本文はShift_JISで数えるので、全角は2バイトとして数える
  const bytes = Array.from(text).reduce((n, c) => n + (c.charCodeAt(0) < 0x80 ? 1 : 2), 0);
  document.getElementById('templateCount').textContent =
    `${text.split('\n').length}行 / ${Array.from(text).length}文字(約${bytes}バイト)`;
  const d = await invoke('comments_preview_draft', { settings }).catch((e) => ({ error: String(e) }));
  document.getElementById('draftNote').textContent = d.sample
    ? '建てた時の見え方(取得していないので「配信スレ★539」の次として表示)'
    : '建てた時の見え方';
  document.getElementById('draftTitle').textContent = d.error ? '' : d.title;
  document.getElementById('threadPreview').textContent = d.error || d.body;
}

function scheduleDraft() {
  clearTimeout(draftTimer);
  draftTimer = setTimeout(renderDraft, 200);
}

for (const key of ['createBodyTemplate', 'createTitleTemplate']) {
  document.querySelector(`[data-key="${key}"]`).addEventListener('input', scheduleDraft);
}
document.querySelector('nav button[data-tab="create"]').addEventListener('click', renderDraft);

for (const btn of document.querySelectorAll('[data-insert]')) {
  btn.addEventListener('click', () => {
    // カーソルの位置に差し込む
    const { selectionStart: a, selectionEnd: b, value } = templateEl;
    templateEl.value = value.slice(0, a) + btn.dataset.insert + value.slice(b);
    const pos = a + btn.dataset.insert.length;
    templateEl.focus();
    templateEl.setSelectionRange(pos, pos);
    onFieldChange(templateEl);
    scheduleDraft();
  });
}

document.getElementById('importFirstPostBtn').addEventListener('click', async () => {
  try {
    const body = await invoke('comments_first_post_template');
    templateEl.value = body;
    onFieldChange(templateEl);
    renderDraft();
  } catch (e) {
    document.getElementById('threadPreview').textContent = String(e);
  }
});

// ---- YouTube・Twitchの接続確認(取得は開始しない) ----
const checkResultEl = document.getElementById('checkResult');
function showCheck(lines, ok) {
  checkResultEl.style.display = '';
  checkResultEl.style.color = ok ? 'var(--c-7dff9a)' : 'var(--c-ff8a8a)';
  checkResultEl.textContent = lines.filter(Boolean).join('\n');
}
document.getElementById('checkYoutubeBtn').addEventListener('click', async (e) => {
  const target = (settings.youtubeTarget || '').trim();
  if (!target) return showCheck(['YouTubeの配信URLかチャンネルを入力してください'], false);
  e.target.disabled = true;
  showCheck(['YouTubeに接続しています…'], true);
  try {
    const r = await invoke('comments_check_youtube', { target });
    showCheck(
      [
        (r.ok ? '○ ' : '× ') + r.message,
        r.title ? `配信: ${r.title}` : '',
        r.videoId ? `動画ID: ${r.videoId}` : '',
        r.samples.length ? '直近のコメント:' : '',
        ...r.samples.map((s) => '  ' + s),
      ],
      r.ok
    );
  } catch (err) {
    showCheck([String(err)], false);
  } finally {
    e.target.disabled = false;
  }
});
document.getElementById('checkTwitchBtn').addEventListener('click', async (e) => {
  const channel = (settings.twitchChannel || '').trim();
  if (!channel) return showCheck(['Twitchのチャンネル名を入力してください'], false);
  e.target.disabled = true;
  showCheck(['Twitchに接続しています(8秒ほどコメントを待ちます)…'], true);
  try {
    const r = await invoke('comments_check_twitch', { channel });
    showCheck([(r.ok ? '○ ' : '× ') + r.message, r.samples.length ? '受信したコメント:' : '', ...r.samples.map((s) => '  ' + s)], r.ok);
  } catch (err) {
    showCheck([String(err)], false);
  } finally {
    e.target.disabled = false;
  }
});

// ---- スレッドを手動で選ぶ ----
const threadPicker = document.getElementById('threadPicker');
document.getElementById('listThreadsBtn').addEventListener('click', async (e) => {
  const url = document.getElementById('threadUrl').value.trim();
  if (!url) return showStatusError('掲示板のURLを入力してください');
  e.target.disabled = true;
  threadPicker.style.display = '';
  threadPicker.textContent = '読み込んでいます…';
  try {
    const { threads } = await invoke('comments_list_threads', { url });
    threadPicker.textContent = '';
    for (const t of threads) {
      const row = document.createElement('div');
      row.className = 'post';
      const head = document.createElement('div');
      head.className = 'post-head';
      const title = document.createElement('span');
      title.textContent = `${t.title}(${t.count})${t.current ? ' ← 読み込み中' : ''}`;
      if (t.count >= 1000) title.style.color = 'var(--c-8d909c)';
      const btn = document.createElement('button');
      btn.className = 'secondary';
      btn.textContent = 'このスレッドを読む';
      btn.disabled = t.current;
      btn.addEventListener('click', async () => {
        try {
          await invoke('comments_switch_thread', { url: t.url });
          threadPicker.style.display = 'none';
        } catch (err) {
          showStatusError(String(err));
        }
      });
      head.append(title, btn);
      row.appendChild(head);
      threadPicker.appendChild(row);
    }
    if (!threads.length) threadPicker.textContent = 'スレッドが見つかりませんでした';
  } catch (err) {
    threadPicker.textContent = String(err);
  } finally {
    e.target.disabled = false;
  }
});

// ---- 表示するモニター ----
monitorSelect.addEventListener('change', () => {
  settings.desktopMonitorId = monitorSelect.value || null;
  scheduleSave();
});

async function loadMonitors() {
  const monitors = await invoke('list_monitors').catch(() => []);
  monitorSelect.textContent = '';
  const def = document.createElement('option');
  def.value = '';
  def.textContent = 'メインモニター';
  monitorSelect.appendChild(def);
  for (const m of monitors) {
    const o = document.createElement('option');
    o.value = m.id;
    o.textContent = m.label;
    monitorSelect.appendChild(o);
  }
}

// ---- 開始・停止・状態表示 ----
function showStatusError(text) {
  statusEl.textContent = '';
  const span = document.createElement('span');
  span.className = 'ng';
  span.textContent = text;
  statusEl.appendChild(span);
}

function renderStatus(s) {
  startBtn.textContent = s.running ? '再開始' : '開始';
  stopBtn.disabled = !s.running;
  statusEl.textContent = '';
  const add = (text, cls) => {
    const span = document.createElement('span');
    if (cls) span.className = cls;
    span.textContent = text;
    statusEl.appendChild(span);
  };
  pauseBtn.textContent = s.paused ? '再開' : '一時停止';
  if (!s.running) {
    add('停止中');
    if (s.paused) add(' / 読み上げ一時停止中', 'warn');
    return;
  }
  if (s.paused) add('読み上げ一時停止中 ', 'warn');
  add('取得中', 'ok');
  if (s.threadUrl) {
    if (s.threadTitle) add(` 「${s.threadTitle}」`);
    add(` レス${s.lastNo}`);
  }
  if (s.queueLen > 0) add(` / 順番待ち${s.queueLen}件`);
  if (s.warning) add(` ${s.warning}`, 'warn');
  if (s.error) add(` ${s.error}`, 'ng');
  for (const src of s.sources || []) {
    statusEl.appendChild(document.createElement('br'));
    add(`${src.label}: `);
    add(src.message, src.state === 'ok' ? 'ok' : src.state === 'error' ? 'ng' : 'warn');
  }
}

// ---- コメントとリアクションの連動 ----
const reactionRulesMsgEl = document.getElementById('reactionRulesMsg');
document.getElementById('reactionRulesResetBtn').addEventListener('click', async () => {
  settings.reactionRules = await invoke('comments_default_reaction_rules');
  renderFields();
  scheduleSave();
  reactionRulesMsgEl.textContent = '既定のルールに戻しました';
});
// 使えるリアクションの一覧。押すと「/ID」をルールの最後に付け足す(正規表現はその前に書く)
for (const e of window.EmojiSet.EMOJI_SET) {
  const b = document.createElement('button');
  b.className = 'secondary';
  b.textContent = `${e.char.length > 4 ? e.label : e.char} ${e.id}`;
  b.title = e.label;
  b.addEventListener('click', () => {
    const ta = document.querySelector('[data-key="reactionRules"]');
    const cur = ta.value.replace(/\n+$/, '');
    ta.value = `${cur}${cur ? '\n' : ''}/${e.id}`;
    onFieldChange(ta);
    ta.focus();
    // カーソルを「/ID」の前に置いて、そのまま言葉を打てるようにする
    const pos = ta.value.length - e.id.length - 1;
    ta.setSelectionRange(pos, pos);
  });
  document.getElementById('reactionIdList').appendChild(b);
}

// ---- 読み上げの一時停止・スキップ(ボタンとホットキー。ホットキーはRust側で登録) ----
const pauseBtn = document.getElementById('pauseBtn');
pauseBtn.addEventListener('click', () => invoke('comments_toggle_pause').catch(() => {}));
document.getElementById('skipBtn').addEventListener('click', () => invoke('comments_skip').catch(() => {}));

const hotkeyMsgEl = document.getElementById('hotkeyMsg');
const HOTKEY_HINT = hotkeyMsgEl.textContent;
let hotkeyCapturing = null; // 割り当て中の設定項目名("hotkeySkip" | "hotkeyPause")
let hotkeyCaptureTimer = null;

function formatHotkeyLabel(shortcut) {
  if (!shortcut) return '';
  return shortcut
    .split('+')
    .map((tok) => tok.replace(/^Key/, '').replace(/^Digit/, ''))
    .join('+');
}

function renderHotkeys() {
  for (const el of document.querySelectorAll('[data-hotkey]')) {
    const key = el.dataset.hotkey;
    el.value = hotkeyCapturing === key ? 'キーを押してください(Escで取消)' : formatHotkeyLabel(settings[key]);
  }
}

/** ReaTapと同じ書き方("Ctrl+Alt+KeyS")。修飾キーだけの時はnull。 */
function shortcutFromEvent(e) {
  if (/^(Shift|Control|Alt|Meta)(Left|Right)$/.test(e.code)) return null;
  const parts = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Super');
  parts.push(e.code);
  return parts.join('+');
}

async function applyHotkeys() {
  try {
    await invoke('comments_apply_hotkeys');
    hotkeyMsgEl.textContent = HOTKEY_HINT;
    hotkeyMsgEl.style.color = '';
  } catch (err) {
    hotkeyMsgEl.textContent = String(err);
    hotkeyMsgEl.style.color = 'var(--c-ff8a8a)';
  }
}

async function endHotkeyCapture() {
  document.removeEventListener('keydown', onHotkeyCaptureKeydown, true);
  clearTimeout(hotkeyCaptureTimer);
  hotkeyCapturing = null;
  renderHotkeys();
  await applyHotkeys();
}

async function saveHotkey(key, shortcut) {
  settings[key] = shortcut;
  clearTimeout(saveTimer);
  await invoke('comments_save_settings', { settings }).catch((e) => showStatusError(`設定の保存に失敗しました: ${e}`));
}

function onHotkeyCaptureKeydown(e) {
  if (!hotkeyCapturing) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.code === 'Escape') {
    endHotkeyCapture();
    return;
  }
  const shortcut = shortcutFromEvent(e);
  if (!shortcut) return;
  const key = hotkeyCapturing;
  const other = key === 'hotkeySkip' ? 'hotkeyPause' : 'hotkeySkip';
  (async () => {
    if (settings[other] === shortcut) {
      hotkeyMsgEl.textContent = `「${formatHotkeyLabel(shortcut)}」は既にもう一方に割り当てられています`;
      hotkeyMsgEl.style.color = 'var(--c-ff8a8a)';
      return;
    }
    await saveHotkey(key, shortcut);
    await endHotkeyCapture();
  })();
}

for (const btn of document.querySelectorAll('[data-hotkey-set]')) {
  btn.addEventListener('click', async () => {
    // 割り当て済みのキーを押した時にOSに奪われないよう、一旦すべて外す
    await invoke('comments_suspend_hotkeys').catch(() => {});
    if (!hotkeyCapturing) document.addEventListener('keydown', onHotkeyCaptureKeydown, true);
    hotkeyCapturing = btn.dataset.hotkeySet;
    renderHotkeys();
    clearTimeout(hotkeyCaptureTimer);
    hotkeyCaptureTimer = setTimeout(endHotkeyCapture, 10000);
  });
}

for (const btn of document.querySelectorAll('[data-hotkey-clear]')) {
  btn.addEventListener('click', async () => {
    await saveHotkey(btn.dataset.hotkeyClear, '');
    renderHotkeys();
    await applyHotkeys();
  });
}

startBtn.addEventListener('click', async () => {
  const url = document.getElementById('threadUrl').value.trim();
  try {
    // 入力途中のYouTube・Twitchの設定も保存してから開始する
    clearTimeout(saveTimer);
    await invoke('comments_save_settings', { settings });
    await invoke('comments_start', { url });
    settings.threadUrl = url;
  } catch (e) {
    showStatusError(String(e));
  }
});

stopBtn.addEventListener('click', () => invoke('comments_stop'));

// ---- スレッドのレス(過去のレスを遡る) ----
const threadListEl = document.getElementById('threadList');

function buildPostEl(p) {
  const el = document.createElement('div');
  el.className = 'post' + (p.aa ? ' aa' : '');
  const head = document.createElement('div');
  head.className = 'post-head';
  const no = document.createElement('span');
  no.textContent = String(p.no);
  const name = document.createElement('span');
  name.textContent = p.name || '';
  const date = document.createElement('span');
  date.className = 'date';
  date.textContent = p.date || '';
  const btn = document.createElement('button');
  btn.className = 'secondary';
  btn.textContent = '読み上げ';
  btn.title = 'このレスをもう一度字幕に表示して読み上げます';
  btn.addEventListener('click', () => invoke('comments_replay', { no: p.no }).catch(() => {}));
  const reply = document.createElement('button');
  reply.className = 'secondary';
  reply.textContent = '返信';
  reply.title = '下の書き込み欄に「>>番号」を入れます';
  reply.addEventListener('click', () => insertReplyAnchor(p.no));
  head.append(no, name, date, btn, reply);
  const body = document.createElement('div');
  body.className = 'post-body';
  body.textContent = p.body || '';
  el.append(head, body);
  el.addEventListener('contextmenu', (e) => openPostMenu(e, p));
  return el;
}

// ---- レスの右クリックメニュー(NGワード・読み方に追加) ----
const postMenuEl = document.getElementById('postMenu');
const postMenuMsgEl = document.getElementById('postMenuMsg');
const readingDialogEl = document.getElementById('readingDialog');
const readingInputEl = document.getElementById('readingInput');
let readingWord = '';

function closePostMenu() {
  postMenuEl.classList.remove('visible');
}

/** 改行区切りの設定(NGワード等)に1行足す。既にあれば足さない。 */
function appendLine(key, line) {
  const cur = String(settings[key] || '').replace(/\n+$/, '');
  if (cur.split('\n').includes(line)) return false;
  settings[key] = cur ? `${cur}\n${line}` : line;
  renderFields();
  scheduleSave();
  return true;
}

function addNgWord(word) {
  const ok = appendLine('ngWords', word);
  postMenuMsgEl.textContent = ok ? `「${word}」をNGワードに追加しました` : `「${word}」は既にNGワードにあります`;
}

/** 読み方の単語部分は正規表現なので、記号はそのままの文字として扱うようにする。 */
function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
}

function openPostMenu(e, p) {
  e.preventDefault();
  const selected = String(window.getSelection() || '').trim().replace(/\s*\n\s*/g, ' ');
  const id = (String(p.date || '').match(/ID:([^\s]+)/) || [])[1];
  const items = [
    [selected ? `「${selected.slice(0, 20)}」をNGワードに追加` : '文字を選んでから右クリックするとNGワードに追加できます', selected ? () => addNgWord(selected) : null],
    [selected ? `「${selected.slice(0, 20)}」の読み方を登録…` : '文字を選んでから右クリックすると読み方を登録できます', selected ? () => openReadingDialog(selected) : null],
    [p.name ? `名前「${p.name.slice(0, 20)}」をNGワードに追加` : '名前がありません', p.name ? () => addNgWord(p.name) : null],
    [id ? `ID「${id}」をNGワードに追加` : 'IDがありません', id ? () => addNgWord(`ID:${id}`) : null],
  ];
  postMenuEl.textContent = '';
  for (const [label, action] of items) {
    const b = document.createElement('button');
    b.textContent = label;
    b.disabled = !action;
    if (action) {
      b.addEventListener('click', () => {
        closePostMenu();
        action();
      });
    }
    postMenuEl.appendChild(b);
  }
  postMenuEl.classList.add('visible');
  // 画面からはみ出さないように位置を決める
  const { offsetWidth: w, offsetHeight: h } = postMenuEl;
  postMenuEl.style.left = `${Math.min(e.clientX, window.innerWidth - w - 4)}px`;
  postMenuEl.style.top = `${Math.min(e.clientY, window.innerHeight - h - 4)}px`;
}

function openReadingDialog(word) {
  readingWord = word;
  document.getElementById('readingWord').textContent = word;
  readingInputEl.value = '';
  readingDialogEl.classList.add('visible');
  readingInputEl.focus();
}

function submitReading() {
  const reading = readingInputEl.value.trim();
  readingDialogEl.classList.remove('visible');
  if (!reading) return;
  const ok = appendLine('readingRules', `${escapeRegExp(readingWord)}/${reading}`);
  postMenuMsgEl.textContent = ok ? `「${readingWord}」を「${reading}」と読むように登録しました` : '既に同じ読み方が登録されています';
}

document.getElementById('readingOk').addEventListener('click', submitReading);
document.getElementById('readingCancel').addEventListener('click', () => readingDialogEl.classList.remove('visible'));
readingInputEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') submitReading();
  if (e.key === 'Escape') readingDialogEl.classList.remove('visible');
});
document.addEventListener('click', (e) => {
  if (!postMenuEl.contains(e.target)) closePostMenu();
});
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') closePostMenu();
});
window.addEventListener('blur', closePostMenu);
threadListEl.addEventListener('scroll', closePostMenu);

function appendPosts(posts) {
  if (!posts.length) return;
  // 一番下を見ている時だけ、新しいレスに合わせて自動でスクロールする
  const atBottom = threadListEl.scrollHeight - threadListEl.scrollTop - threadListEl.clientHeight < 40;
  const empty = threadListEl.querySelector('.empty');
  if (empty) empty.remove();
  const frag = document.createDocumentFragment();
  for (const p of posts) frag.appendChild(buildPostEl(p));
  threadListEl.appendChild(frag);
  if (atBottom) threadListEl.scrollTop = threadListEl.scrollHeight;
}

// ---- 今読んでいるスレッドへの書き込み(名前・メール欄は設定として保存) ----
const postBodyEl = document.getElementById('postBody');
const postBtn = document.getElementById('postBtn');
const postMsgEl = document.getElementById('postMsg');

function insertReplyAnchor(no) {
  const anchor = `>>${no}\n`;
  const v = postBodyEl.value;
  postBodyEl.value = v && !v.endsWith('\n') ? `${v}\n${anchor}` : v + anchor;
  postBodyEl.focus();
  postBodyEl.setSelectionRange(postBodyEl.value.length, postBodyEl.value.length);
}

async function submitPost() {
  const body = postBodyEl.value;
  if (!body.trim() || postBtn.disabled) return;
  postBtn.disabled = true;
  postMsgEl.textContent = '書き込み中…';
  try {
    // 入力途中の名前・メール欄も反映してから書き込む
    clearTimeout(saveTimer);
    await invoke('comments_save_settings', { settings });
    await invoke('comments_post', { body });
    postBodyEl.value = '';
    postMsgEl.textContent = '書き込みました';
  } catch (err) {
    postMsgEl.textContent = String((err && err.message) || err);
  } finally {
    postBtn.disabled = false;
  }
}

postBtn.addEventListener('click', submitPost);
postBodyEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && e.ctrlKey) {
    e.preventDefault();
    submitPost();
  }
});

function resetPosts() {
  threadListEl.textContent = '';
  const empty = document.createElement('div');
  empty.className = 'empty';
  empty.textContent = '取得を開始すると、ここにスレッドのレスが表示されます';
  threadListEl.appendChild(empty);
}

document.getElementById('testBtn').addEventListener('click', () => {
  const text = document.getElementById('testText').value.trim();
  if (text) invoke('comments_test', { text });
});

(async () => {
  settings = await invoke('comments_get_settings');
  settings.style = C.mergeStyle(settings.style);
  await loadMonitors();
  renderFields();
  renderStatus(await invoke('comments_status'));
  listen('comments:status', (e) => renderStatus(e.payload));
  // マウスで位置を決めた時など、Rust側で設定が変わった時に表示し直す
  listen('comments:settings', (e) => {
    settings = e.payload;
    settings.style = C.mergeStyle(settings.style);
    renderFields();
  });
  adjusting = await invoke('comments_desktop_adjusting');
  renderAdjust();
  listen('subtitle:adjust', (e) => {
    adjusting = !!e.payload;
    renderAdjust();
  });
  const thread = await invoke('comments_get_thread');
  appendPosts(thread.posts || []);
  threadListEl.scrollTop = threadListEl.scrollHeight;
  listen('comments:posts', (e) => {
    if (e.payload.reset) resetPosts();
    appendPosts(e.payload.posts || []);
  });
})();
