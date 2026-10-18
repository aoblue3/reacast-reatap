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
  if (el.hasAttribute('data-int')) return Number(el.value) | 0;
  if (el.type === 'number' || el.type === 'range') {
    const n = Number(el.value);
    if (!Number.isFinite(n)) return undefined;
    return el.hasAttribute('data-percent') ? n / 100 : n;
  }
  return el.value;
}

function writeField(el, value) {
  if (el.type === 'checkbox') el.checked = !!value;
  else if (el.type === 'number' || el.type === 'range') {
    const n = Number(value);
    el.value = Number.isFinite(n) ? String(el.hasAttribute('data-percent') ? Math.round(n * 1000) / 10 : n) : '';
  } else if (el.hasAttribute('data-keep-unknown') && value != null && value !== '' && !Array.from(el.options).some((o) => o.value === String(value))) {
    // 一覧をまだ取得していない声・出力先も、保存済みの値として表示しておく
    const o = document.createElement('option');
    o.value = String(value);
    o.textContent = `(保存済み: ${value})`;
    el.appendChild(o);
    el.value = String(value);
  } else el.value = value == null ? '' : String(value);
  if (el.type === 'range') updateRangeOutput(el);
}

function renderFields() {
  for (const el of fields) writeField(el, getPath(settings, el.dataset.key));
  renderHotkeys();
  renderRuleEditors();
  renderEngineBoxes();
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
  // 設定を読み込む前にプレビューの画面が読み込み終わることがある(その時は読み込み後に描く)
  if (!settings || !listPreview || !listPreview.contentWindow) return;
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

// ---- 読み上げソフト(tts.rs) ----
const TTS_DEFAULT_URL = {
  voicevox: 'http://127.0.0.1:50021',
  coeiroink: 'http://127.0.0.1:50032',
  coeiroinkv1: 'http://127.0.0.1:50031',
  aivisspeech: 'http://127.0.0.1:10101',
  sharevox: 'http://127.0.0.1:50025',
  voicevoxcompat: 'http://127.0.0.1:50021',
};
const COMMAND_PRESETS = {
  tamiyasu: { commandPath: 'C:\\tamiyasuex\\vrx.exe', commandArgs: '#Res#' },
  // /X:1=SofTalkの画面を出さない、/W:は最後に置く(それ以降が読み上げる文字)
  softalk: { commandPath: 'C:\\softalk\\softalkw.exe', commandArgs: '/X:1 /W:#Res#' },
};
let ttsListedFor = null; // 声の一覧を取得した接続先(取り直しを避ける)
let ttsDevicesListed = false;
let ttsExeCheckedFor = null; // exeの場所を探したソフト

function isHttpEngine(engine) {
  return Object.prototype.hasOwnProperty.call(TTS_DEFAULT_URL, engine);
}

function updateRangeOutput(el) {
  const out = el.parentElement.querySelector('output');
  if (out) out.textContent = Number(el.value).toFixed(2);
}

function renderEngineBoxes() {
  const engine = settings.speechEngine || 'command';
  const http = isHttpEngine(engine);
  for (const box of document.querySelectorAll('[data-engine]')) {
    const kind = box.dataset.engine;
    const show =
      kind === 'http' ? http
      : kind === 'voicevox-only' ? http && engine !== 'coeiroink'
      : kind === 'test' || kind === 'launch' ? engine !== 'command'
      : kind === engine;
    box.style.display = show ? '' : 'none';
  }
  if (http) {
    document.getElementById('ttsDefaultUrl').textContent = TTS_DEFAULT_URL[engine];
    // 開いた時・ソフトを切り替えた時は、黙って声の一覧を取ってみる(起動していなければそのまま)
    const key = `${engine} ${settings.ttsUrl || ''}`;
    if (ttsListedFor !== key) {
      ttsListedFor = key;
      fetchSpeakers(true);
    }
    if (!ttsDevicesListed) {
      ttsDevicesListed = true;
      fetchDevices();
    }
  }
  if (engine !== 'command' && ttsExeCheckedFor !== engine) {
    ttsExeCheckedFor = engine;
    findExe(true);
  }
}

/** 読み上げソフトのexeを探して、見つかった場所を表示する。指定欄が空なら覚えておく
 * (起動中のプロセスから見つかった場所は、ソフトを閉じた後の自動起動に使うため)。 */
async function findExe(quiet) {
  const msg = document.getElementById('ttsExeMsg');
  const found = await invoke('tts_find_exe', { settings }).catch(() => null);
  if (found) {
    if (!settings.ttsExePath || !quiet) {
      settings.ttsExePath = found;
      writeField(document.getElementById('ttsExePath'), found);
      scheduleSave();
    }
    msg.textContent = `見つかりました: ${found}`;
  } else if (!settings.ttsExePath) {
    msg.textContent = '見つかりませんでした。ソフトを起動してから「自動で探す」を押すか、「参照...」で場所を指定してください。';
  } else if (!quiet) {
    msg.textContent = '自動では見つかりませんでした(指定した場所を使います)。';
  }
}

function fillSpeakerSelects(list) {
  for (const sel of document.querySelectorAll('select[data-speaker-select]')) {
    sel.textContent = '';
    const none = document.createElement('option');
    none.value = '';
    none.textContent = sel.hasAttribute('data-allow-same') ? '上の「声」と同じ' : '(選んでください)';
    sel.appendChild(none);
    for (const sp of list) {
      const o = document.createElement('option');
      o.value = sp.id;
      o.textContent = sp.name;
      sel.appendChild(o);
    }
    writeField(sel, getPath(settings, sel.dataset.key));
  }
}

async function fetchSpeakers(quiet) {
  const msg = document.getElementById('ttsSpeakerMsg');
  if (!quiet) msg.textContent = '取得中…';
  try {
    const list = await invoke('tts_list_speakers', { settings });
    fillSpeakerSelects(list);
    // まだ声を選んでいなければ、最初の声にしておく
    if (!settings.ttsSpeaker && list.length) {
      settings.ttsSpeaker = list[0].id;
      fillSpeakerSelects(list);
      scheduleSave();
    }
    msg.textContent = `${list.length}種類の声があります。`;
  } catch (e) {
    fillSpeakerSelects([]);
    msg.textContent = quiet ? '読み上げソフトを起動してから「声の一覧を取得」を押してください。' : String(e);
  }
}

async function fetchDevices() {
  const sel = document.getElementById('ttsDevice');
  const names = await invoke('tts_list_devices').catch(() => []);
  sel.textContent = '';
  const def = document.createElement('option');
  def.value = '';
  def.textContent = 'Windowsの既定の出力先';
  sel.appendChild(def);
  for (const n of names) {
    const o = document.createElement('option');
    o.value = n;
    o.textContent = n;
    sel.appendChild(o);
  }
  writeField(sel, settings.ttsDevice);
}

document.getElementById('speechEngine').addEventListener('change', () => {
  // ソフトが変わると接続先も声の番号も別物になるので、選び直してもらう
  settings.ttsUrl = '';
  settings.ttsExePath = '';
  settings.ttsSpeaker = '';
  settings.ttsSpeakerYoutube = '';
  settings.ttsSpeakerTwitch = '';
  document.getElementById('ttsSpeakerMsg').textContent = '';
  document.getElementById('ttsTestMsg').textContent = '';
  renderFields();
  scheduleSave();
});
// 接続先を変えたら、入力が落ち着いてから一覧を取り直す
let ttsUrlTimer = null;
for (const el of document.querySelectorAll('input[data-key="ttsUrl"]')) {
  el.addEventListener('input', () => {
    clearTimeout(ttsUrlTimer);
    ttsUrlTimer = setTimeout(renderFields, 800);
  });
}
for (const el of document.querySelectorAll('input[type=range][data-key]')) {
  el.addEventListener('input', () => updateRangeOutput(el));
}
for (const btn of document.querySelectorAll('[data-reset]')) {
  btn.addEventListener('click', () => {
    const el = btn.parentElement.querySelector('input[type=range]');
    el.value = el.dataset.default;
    onFieldChange(el);
    updateRangeOutput(el);
  });
}
document.getElementById('ttsFetchSpeakers').addEventListener('click', () => fetchSpeakers(false));
document.getElementById('ttsFetchDevices').addEventListener('click', fetchDevices);
document.getElementById('ttsFindExe').addEventListener('click', () => findExe(false));
document.getElementById('ttsPickExe').addEventListener('click', async () => {
  const path = await invoke('comments_pick_exe').catch(() => null);
  if (!path) return;
  settings.ttsExePath = path;
  document.getElementById('ttsExeMsg').textContent = '';
  renderFields();
  scheduleSave();
});
for (const btn of document.querySelectorAll('[data-command-preset]')) {
  btn.addEventListener('click', () => {
    Object.assign(settings, COMMAND_PRESETS[btn.dataset.commandPreset]);
    renderFields();
    scheduleSave();
  });
}
document.getElementById('ttsTestBtn').addEventListener('click', async () => {
  const text = document.getElementById('ttsTestText').value.trim();
  const msg = document.getElementById('ttsTestMsg');
  if (!text) return;
  msg.textContent = '読み上げ中…';
  try {
    await invoke('tts_test', { settings, text });
    msg.textContent = '';
  } catch (e) {
    msg.textContent = String(e);
  }
});

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
  // ペルソナ5風: 黒地に白、赤のアクセント、斜めに傾いた札、太いゴシック
  p5: {
    headFontSize: 14, headBold: true, numberColor: '#000000', nameColor: '#000000', dateColor: '#bbbbbb',
    bodyColor: '#ffffff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 92, itemPadding: 8, itemRadius: 0,
    customCss: [
      "@import url('https://fonts.googleapis.com/css2?family=Dela+Gothic+One&display=swap');",
      ".item:not(.aa), .item:not(.aa) .head, .item:not(.aa) .body { font-family: 'Dela Gothic One', 'BIZ UDゴシック', sans-serif !important; }",
      '.item { transform: skewX(-10deg) rotate(-1.5deg); border-left: 6px solid #ff1f1f; box-shadow: 5px 5px 0 #ff1f1f; }',
      '.item:nth-child(even) { transform: skewX(-10deg) rotate(1.5deg); }',
      '.item .content { transform: skewX(10deg); }',
      '.head .no, .head .avatar { background: #ff1f1f; padding: 0 0.35em; transform: rotate(-4deg); display: inline-block; }',
      '.head .name { background: #ffffff; padding: 0 0.35em; transform: rotate(2deg); display: inline-block; }',
      '.body { letter-spacing: 0.04em; }',
    ].join('\n'),
  },
  // レトロ(ファミコン風): ドット文字、黒地に白い二重枠のメッセージウィンドウ
  retro: {
    headFontSize: 0, headBold: false, numberColor: '#f8d800', nameColor: '#7cf87c', dateColor: '#a8a8a8',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 100, itemPadding: 10, itemRadius: 4,
    customCss: [
      "@import url('https://fonts.googleapis.com/css2?family=DotGothic16&display=swap');",
      ".item:not(.aa), .item:not(.aa) .head, .item:not(.aa) .body { font-family: 'DotGothic16', monospace !important; -webkit-font-smoothing: none; }",
      '.item { border: 3px solid #ffffff; box-shadow: 0 0 0 3px #000000, inset 0 0 0 3px #000000; }',
      ".head .name::before { content: '▶'; margin-right: 0.2em; }",
      // 一番新しいレスの最後に、点滅する「▼」(新しい順でも古い順でも)
      "#list:not(.bottom) .item:first-child .body::after, #list.bottom .item:last-child .body::after { content: ' ▼'; animation: retro-blink 1s steps(1) infinite; }",
      '@keyframes retro-blink { 50% { opacity: 0; } }',
    ].join('\n'),
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

// ---- 話した字幕(音声認識はEdgeの窓で行う。src-tauri/src/voice.rs) ----
const voiceStatusEl = document.getElementById('voiceStatus');
function renderVoiceStatus(v) {
  if (!v || !v.state) return;
  voiceStatusEl.textContent = v.message || '';
  voiceStatusEl.style.color =
    v.state === 'listening' ? 'var(--c-7dff9a)' : v.state === 'error' || v.state === 'unsupported' ? 'var(--c-ff8a8a)' : '';
}
const voiceMuteBtn = document.getElementById('voiceMuteBtn');
function renderVoiceMuted(muted) {
  voiceMuteBtn.textContent = muted ? 'マイクを再開する' : 'マイクを止める';
  voiceMuteBtn.classList.toggle('secondary', !!muted);
}
voiceMuteBtn.addEventListener('click', () => invoke('voice_toggle_mute').then(renderVoiceMuted).catch(() => {}));
invoke('voice_status')
  .then((v) => {
    renderVoiceStatus(v);
    renderVoiceMuted(v && v.muted);
  })
  .catch(() => {});
listen('voice:status', (e) => renderVoiceStatus(e.payload));
// ホットキーで切り替えた時もボタンの表示を合わせる
listen('voice:muted', (e) => renderVoiceMuted(e.payload));
document.getElementById('voiceReopenBtn').addEventListener('click', async () => {
  // 入力途中の設定(言語など)も反映してから開き直す
  clearTimeout(saveTimer);
  await invoke('comments_save_settings', { settings }).catch(() => {});
  invoke('voice_reopen').catch(() => {});
});

// ---- コメントとリアクションの連動 ----
const reactionRulesMsgEl = document.getElementById('reactionRulesMsg');
document.getElementById('reactionRulesResetBtn').addEventListener('click', async () => {
  settings.reactionRules = await invoke('comments_default_reaction_rules');
  renderFields();
  scheduleSave();
  reactionRulesMsgEl.textContent = '既定のルールに戻しました';
});
// ---- 読み方・リアクション連動のルールを、正規表現を書かずに編集する画面 ----
// 保存する形はこれまでどおり「正規表現/読み方(またはリアクションID)」の1行1ルール。
// 「言葉(、区切りで複数)」と「当てはまり方」から正規表現を作り、逆に読み込む時は、
// その形で作れる正規表現だけを「言葉」に戻す(戻せないものは上級者向けの行として出す)。
const RULE_MODES = [
  ['contains', '含む'],
  ['exact', 'ちょうど同じ'],
  ['starts', 'で始まる'],
  ['ends', 'で終わる'],
  ['regex', '上級者向け(正規表現)'],
];
const REGEX_SPECIAL = '.*+?^${}()|[]\\/';

/** 正規表現の「|」で区切る(\| は区切りではない)。 */
function splitAlternatives(pattern) {
  const out = [];
  let cur = '';
  for (let i = 0; i < pattern.length; i++) {
    const c = pattern[i];
    if (c === '\\' && i + 1 < pattern.length) {
      cur += c + pattern[++i];
    } else if (c === '|') {
      out.push(cur);
      cur = '';
    } else {
      cur += c;
    }
  }
  out.push(cur);
  return out;
}

/** 記号を含まないただの文字列なら、その文字列(\. 等は元に戻す)。そうでなければnull。 */
function regexToLiteral(s) {
  let out = '';
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '\\') {
      const next = s[++i];
      if (next === undefined || !REGEX_SPECIAL.includes(next)) return null;
      out += next;
    } else if (REGEX_SPECIAL.includes(c)) {
      return null;
    } else {
      out += c;
    }
  }
  return out;
}

/** 正規表現を{mode, words}に戻す。言葉の形で表せなければnull。 */
function patternToWords(pattern) {
  if (!pattern || pattern.includes('¥')) return null;
  let mode = null;
  const words = [];
  for (const alt of splitAlternatives(pattern)) {
    let body = alt;
    const start = body.startsWith('^');
    if (start) body = body.slice(1);
    const end = body.endsWith('$') && !body.endsWith('\\$');
    if (end) body = body.slice(0, -1);
    const word = regexToLiteral(body);
    if (!word || word.includes('、')) return null;
    const m = start && end ? 'exact' : start ? 'starts' : end ? 'ends' : 'contains';
    if (mode && mode !== m) return null;
    mode = m;
    words.push(word);
  }
  return { mode, words };
}

function wordsToPattern(mode, words) {
  const lits = words.map(escapeRegExp);
  const wrap = { contains: (l) => l, exact: (l) => `^${l}$`, starts: (l) => `^${l}`, ends: (l) => `${l}$` }[mode];
  return lits.map(wrap).join('|');
}

function parseRules(text) {
  return String(text || '')
    .split('\n')
    .filter((l) => l.trim() !== '')
    .map((line) => {
      const i = line.lastIndexOf('/');
      const pattern = i === -1 ? line : line.slice(0, i);
      const target = i === -1 ? '' : line.slice(i + 1);
      const w = patternToWords(pattern);
      return w ? { mode: w.mode, words: w.words.join('、'), target } : { mode: 'regex', words: pattern, target };
    });
}

function serializeRules(rows) {
  return rows
    .map((r) => {
      if (r.mode === 'regex') return r.words.trim() ? `${r.words}/${r.target}` : null;
      const words = r.words.split(/[、,，]/).map((w) => w.trim()).filter(Boolean);
      return words.length ? `${wordsToPattern(r.mode, words)}/${r.target}` : null;
    })
    .filter((l) => l !== null)
    .join('\n');
}

function reactionSelect(value) {
  const sel = document.createElement('select');
  for (const e of window.EmojiSet.EMOJI_SET) {
    const o = document.createElement('option');
    o.value = e.id;
    o.textContent = e.char.length > 4 ? e.label : `${e.char} ${e.label}`;
    sel.appendChild(o);
  }
  if (value && !window.EmojiSet.isValidEmojiId(value)) {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = `${value}(見つかりません)`;
    sel.appendChild(o);
  }
  sel.value = value || window.EmojiSet.EMOJI_SET[0].id;
  return sel;
}

/** 画面の行から設定の文字列を作って保存する。 */
function commitRuleEditor(editor) {
  const rows = [...editor.querySelectorAll('.rule-row')].map((row) => ({
    mode: row.querySelector('.rule-mode').value,
    words: row.querySelector('.rule-words').value,
    target: row.querySelector('.rule-target').value.trim(),
  }));
  const text = serializeRules(rows);
  editor.dataset.last = text;
  const ta = document.querySelector(`textarea[data-key="${editor.dataset.rulesFor}"]`);
  ta.value = text;
  onFieldChange(ta);
}

function buildRuleRow(editor, rule) {
  const isReaction = editor.dataset.target === 'reaction';
  const row = document.createElement('div');
  row.className = 'rule-row';
  const words = document.createElement('input');
  words.type = 'text';
  words.className = 'rule-words' + (rule.mode === 'regex' ? ' rule-regex' : '');
  words.value = rule.words;
  words.placeholder = rule.mode === 'regex' ? '正規表現' : '言葉(、で区切って複数)';
  const mode = document.createElement('select');
  mode.className = 'rule-mode';
  for (const [v, label] of RULE_MODES) {
    const o = document.createElement('option');
    o.value = v;
    o.textContent = label;
    mode.appendChild(o);
  }
  mode.value = rule.mode;
  let target;
  if (isReaction) {
    target = reactionSelect(rule.target);
  } else {
    target = document.createElement('input');
    target.type = 'text';
    target.value = rule.target;
    target.placeholder = '読み方(空欄なら読まない)';
  }
  target.classList.add('rule-target');
  const buttons = document.createElement('div');
  buttons.className = 'rule-buttons';
  const btn = (text, title, fn) => {
    const b = document.createElement('button');
    b.className = 'secondary';
    b.textContent = text;
    b.title = title;
    b.addEventListener('click', fn);
    buttons.appendChild(b);
  };
  btn('↑', '上へ(優先して当てはめる)', () => {
    if (row.previousElementSibling && row.previousElementSibling.classList.contains('rule-row')) {
      row.parentNode.insertBefore(row, row.previousElementSibling);
      commitRuleEditor(editor);
    }
  });
  btn('↓', '下へ', () => {
    if (row.nextElementSibling && row.nextElementSibling.classList.contains('rule-row')) {
      row.parentNode.insertBefore(row.nextElementSibling, row);
      commitRuleEditor(editor);
    }
  });
  btn('✕', 'このルールを消す', () => {
    row.remove();
    commitRuleEditor(editor);
  });
  words.addEventListener('input', () => commitRuleEditor(editor));
  target.addEventListener(isReaction ? 'change' : 'input', () => commitRuleEditor(editor));
  mode.addEventListener('change', () => {
    // 上級者向けとの切り替えでは、今の言葉を正規表現に(または正規表現を言葉に)直して見せる
    const prev = words.classList.contains('rule-regex') ? 'regex' : 'simple';
    if (prev === 'simple' && mode.value === 'regex') {
      const list = words.value.split(/[、,，]/).map((w) => w.trim()).filter(Boolean);
      const lastMode = row.dataset.mode || 'contains';
      words.value = list.length ? wordsToPattern(lastMode, list) : '';
    } else if (prev === 'regex' && mode.value !== 'regex') {
      const w = patternToWords(words.value);
      words.value = w ? w.words.join('、') : '';
    }
    words.classList.toggle('rule-regex', mode.value === 'regex');
    words.placeholder = mode.value === 'regex' ? '正規表現' : '言葉(、で区切って複数)';
    if (mode.value !== 'regex') row.dataset.mode = mode.value;
    commitRuleEditor(editor);
  });
  if (rule.mode !== 'regex') row.dataset.mode = rule.mode;
  row.append(words, mode, target, buttons);
  return row;
}

function renderRuleEditor(editor) {
  const key = editor.dataset.rulesFor;
  const text = String(settings[key] || '');
  // 入力中に保存→設定の再読み込みで描き直されて、入力欄のカーソルが飛ばないようにする
  if (editor.dataset.last === text && editor.childElementCount) return;
  editor.dataset.last = text;
  editor.textContent = '';
  const head = document.createElement('div');
  head.className = 'rule-head';
  for (const t of ['言葉', '当てはまり方', editor.dataset.target === 'reaction' ? 'リアクション' : '読み方', '']) {
    const s = document.createElement('span');
    s.textContent = t;
    head.appendChild(s);
  }
  editor.appendChild(head);
  for (const rule of parseRules(text)) editor.appendChild(buildRuleRow(editor, rule));
  const add = document.createElement('button');
  add.className = 'secondary';
  add.textContent = '＋ ルールを追加';
  add.addEventListener('click', () => {
    const row = buildRuleRow(editor, { mode: 'contains', words: '', target: '' });
    editor.insertBefore(row, add);
    row.querySelector('.rule-words').focus();
  });
  editor.appendChild(add);
}

function renderRuleEditors() {
  for (const editor of document.querySelectorAll('.rule-editor')) renderRuleEditor(editor);
}

for (const toggle of document.querySelectorAll('.rule-text-toggle')) {
  toggle.addEventListener('change', () => {
    const key = toggle.dataset.rulesFor;
    const editor = document.querySelector(`.rule-editor[data-rules-for="${key}"]`);
    document.querySelector(`textarea[data-key="${key}"]`).style.display = toggle.checked ? '' : 'none';
    editor.style.display = toggle.checked ? 'none' : '';
    if (!toggle.checked) {
      editor.dataset.last = '';
      renderRuleEditor(editor);
    }
  });
}

// ---- 読み上げの一時停止・スキップ(ボタンとホットキー。ホットキーはRust側で登録) ----
const pauseBtn = document.getElementById('pauseBtn');
pauseBtn.addEventListener('click', () => invoke('comments_toggle_pause').catch(() => {}));
document.getElementById('skipBtn').addEventListener('click', () => invoke('comments_skip').catch(() => {}));

// ホットキーの案内・エラーは、「読み上げ」タブと「話した字幕」タブの両方に出す
const hotkeyMsgEls = [...document.querySelectorAll('.hotkey-msg')];
for (const el of hotkeyMsgEls) el.dataset.hint = el.textContent;
function showHotkeyMsg(text) {
  for (const el of hotkeyMsgEls) {
    el.textContent = text || el.dataset.hint;
    el.style.color = text ? 'var(--c-ff8a8a)' : '';
  }
}
let hotkeyCapturing = null; // 割り当て中の設定項目名(HOTKEY_KEYSのどれか)
const HOTKEY_LABELS = {
  hotkeySkip: 'スキップ',
  hotkeyPause: '一時停止/再開',
  hotkeyVoiceMute: 'マイクを止める',
  hotkeyVoiceUnmute: 'マイクを再開',
};
const HOTKEY_KEYS = Object.keys(HOTKEY_LABELS);
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
    showHotkeyMsg('');
  } catch (err) {
    showHotkeyMsg(String(err));
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
  const used = HOTKEY_KEYS.filter((k) => k !== key).find((k) => settings[k] === shortcut);
  (async () => {
    if (used) {
      showHotkeyMsg(`「${formatHotkeyLabel(shortcut)}」は既に「${HOTKEY_LABELS[used]}」に割り当てられています`);
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
