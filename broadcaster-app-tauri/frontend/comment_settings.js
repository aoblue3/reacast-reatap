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

// ---- 使う・使わない ----
// タブごとの「使う」の設定。OFFの時は左のメニューのタブとタブの中身をグレーにする
// (中身はそのまま変えられる)。NAV_USE_TABSのタブは、左のメニューにもチェックを出す
const TAB_USE = {
  fetch: 'useBbs', create: 'useBbs', speech: 'speechEnabled', chime: 'chimeEnabled', aa: 'aaEnabled',
  reactions: 'reactionLink', voice: 'voiceEnabled', nico: 'style.nico.enabled',
};
const NAV_USE_TABS = ['fetch', 'speech', 'chime', 'aa', 'reactions', 'voice', 'nico'];
// 取得元の「使う」と、そのURLの欄
const SOURCE_USE = { useBbs: ['threadUrl', 'subThreadUrl'], useYoutube: ['youtubeTarget'], useTwitch: ['twitchChannel'] };
for (const tab of NAV_USE_TABS) {
  const btn = document.querySelector(`#tabs button[data-tab="${tab}"]`);
  if (!btn) continue;
  const box = document.createElement('input');
  box.type = 'checkbox';
  box.className = 'nav-use';
  box.dataset.key = TAB_USE[tab];
  box.title = 'チェックを外すと、この機能を使いません';
  // チェックを押した時は、タブを切り替えない
  box.addEventListener('click', (e) => e.stopPropagation());
  btn.prepend(box);
}
for (const [tab, key] of Object.entries(TAB_USE)) {
  const section = document.querySelector(`main > section[data-tab="${tab}"]`);
  if (!section) continue;
  const note = document.createElement('div');
  note.className = 'tab-off-note';
  note.textContent = key === 'useBbs'
    ? '掲示板を使わない設定になっています(左のメニューのチェックか、「かんたん設定」の「掲示板」のチェックを入れると使います)'
    : 'この機能は使わない設定になっています(左のメニューのチェックか、下の「使う」の欄をONにすると使います)';
  section.prepend(note);
  // タブの中にある「使う」の欄は、グレーにしない
  const own = section.querySelector(`[data-key="${key}"]`);
  let row = own;
  while (row && row.parentElement !== section) row = row.parentElement;
  if (row) row.classList.add('use-master');
}

function isUsed(key) {
  return getPath(settings, key) !== false;
}

function renderUse() {
  if (!settings) return;
  for (const [tab, key] of Object.entries(TAB_USE)) {
    const on = key === 'useBbs' ? isUsed(key) : !!getPath(settings, key);
    const btn = document.querySelector(`#tabs button[data-tab="${tab}"]`);
    if (btn) btn.classList.toggle('use-off', !on);
    const section = document.querySelector(`main > section[data-tab="${tab}"]`);
    if (section) section.classList.toggle('tab-off', !on);
  }
  for (const [key, inputs] of Object.entries(SOURCE_USE)) {
    const on = isUsed(key);
    for (const k of inputs) {
      for (const el of document.querySelectorAll(`[data-key="${k}"]`)) {
        const row = el.closest('.field');
        (row || el).classList.toggle('use-off', !on);
      }
    }
    for (const box of document.querySelectorAll(`.use-check[data-key="${key}"]`)) box.closest('.field').classList.toggle('use-off', !on);
  }
}

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
  // ON/OFFを選択肢で選ぶ項目(value="true"/"false"。保存していない時はtrue扱い)
  if (el.hasAttribute('data-bool')) return el.value !== 'false';
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
  else if (el.hasAttribute('data-bool')) el.value = value === false ? 'false' : 'true';
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
  bindParamSliders();
  for (const el of fields) writeField(el, getPath(settings, el.dataset.key));
  renderHotkeys();
  renderRuleEditors();
  renderEngineBoxes();
  renderShowIf();
  renderUse();
  if (typeof renderFontPickers === 'function') renderFontPickers();
  renderPresetSelects();
  monitorSelect.value = settings.desktopMonitorId || '';
  for (const el of document.querySelectorAll('[data-desktop-rect]')) {
    const r = el.dataset.desktopRect === 'list' ? settings.desktopListRect : settings.desktopRect;
    el.textContent = r ? `マウスで決めた位置: ${r.x}, ${r.y}(${r.width}×${r.height})` : '既定の位置(下の「既定の位置」の設定)';
  }
  renderPreview();
}

/** 字幕のプレビュー(subtitle.html?previewを埋め込んで、本物と同じ描画・アニメーションで見せる)。
 * animate=trueならアニメーション付きで出し直す(「動きを見る」・プリセットを選んだ時)。 */
const subtitlePreview = document.getElementById('subtitlePreview');
const SUBTITLE_SAMPLE = { no: 69, name: '名無し＠BBSさん', date: '2026/10/08(木) 21:00:00', body: 'プレビュー 123 ABC\n2行目のテキスト', aa: false };
function renderPreview(animate) {
  renderListPreview();
  if (typeof renderAudienceNow === 'function') renderAudienceNow();
  if (!settings || !subtitlePreview || !subtitlePreview.contentWindow) return;
  const w = subtitlePreview.contentWindow;
  w.postMessage({ type: 'commentStyle', style: settings.style }, '*');
  w.postMessage({ type: 'subtitle', res: { ...SUBTITLE_SAMPLE }, durationMs: 600000, replay: animate !== true }, '*');
}
subtitlePreview.addEventListener('load', () => renderPreview(false));
document.getElementById('subtitleAnimTest').addEventListener('click', () => renderPreview(true));

// ---- コメント一覧のプレビュー(comments.html?previewを埋め込んで同じ描画をさせる) ----
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
  // 「コメント一覧の上にも出す」の見た目(取得中なら今の数字、そうでなければ見本)
  const live = audienceState && audienceState.running;
  listPreview.contentWindow.postMessage({ type: 'audience', state: live ? audienceState : AUDIENCE_SAMPLE }, '*');
}
listPreview.addEventListener('load', renderListPreview);

document.getElementById('listTestBtn').addEventListener('click', () => {
  const text = document.getElementById('listTestText').value.trim();
  const name = document.getElementById('listTestName').value;
  if (text) invoke('comments_test_list', { text, name }).catch((e) => showStatusError(String(e)));
});

// ---- 視聴者数・コメント数(comments/audience.rs。audience.html?previewを埋め込んで同じ描画をさせる) ----
let audienceState = null;
// 取得していない時にプレビューに出す見本の数字
const AUDIENCE_SAMPLE = {
  running: true,
  sources: [
    { id: 'youtube', viewers: 128, comments: 342 },
    { id: 'twitch', viewers: 35, comments: 87 },
    { id: 'peercast', viewers: 12, comments: 156 },
  ],
};
{
  const css = document.createElement('style');
  css.textContent = window.CommentCommon.AUDIENCE_CSS;
  document.head.appendChild(css);
}
const audiencePreview = document.getElementById('audiencePreview');
function renderAudienceNow() {
  if (!settings) return;
  const cfg = window.CommentCommon.mergeStyle(settings.style).audience;
  const live = audienceState && audienceState.running;
  for (const el of document.querySelectorAll('[data-audience-now]')) {
    // タブの中は、取得していない時も見本を出す(どう見えるか分かるように)
    const state = el.dataset.audienceNow === 'tab' && !live ? AUDIENCE_SAMPLE : audienceState;
    window.CommentCommon.renderAudience(el, state, { ...cfg, layout: 'row' });
  }
  document.getElementById('audienceNowTitle').textContent = live ? '今の数字' : '今の数字(取得していないので見本です)';
  if (audiencePreview && audiencePreview.contentWindow) {
    audiencePreview.contentWindow.postMessage({ type: 'commentStyle', style: settings.style }, '*');
    audiencePreview.contentWindow.postMessage({ type: 'audience', state: live ? audienceState : AUDIENCE_SAMPLE }, '*');
  }
}
audiencePreview.addEventListener('load', renderAudienceNow);
listen('comments:audience', (e) => {
  audienceState = e.payload;
  renderAudienceNow();
});
invoke('comments_audience').then((v) => {
  audienceState = v;
  renderAudienceNow();
}).catch(() => {});

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
  // 同じ設定の欄が2か所にある時(「かんたん設定」と各タブ)は、もう一方も合わせる
  for (const other of fields) {
    if (other !== el && other.dataset.key === el.dataset.key) writeField(other, getPath(settings, el.dataset.key));
  }
  rememberPresetEdit(el.dataset.key);
  renderPreview();
  renderUse();
  scheduleSave();
}

for (const el of fields) {
  el.addEventListener(el.type === 'checkbox' || el.tagName === 'SELECT' ? 'change' : 'input', () => onFieldChange(el));
}

// ---- タブの幅(左の縦のタブと設定の間をドラッグ。この画面の中だけの好みなので、この端末に保存) ----
(() => {
  const nav = document.getElementById('tabs');
  const handle = document.getElementById('navResizer');
  const KEY = 'reacast.settingsNavWidth';
  const clamp = (w) => Math.max(90, Math.min(320, Math.round(w)));
  try {
    const saved = Number(localStorage.getItem(KEY));
    if (saved) nav.style.width = `${clamp(saved)}px`;
  } catch (_) {}
  const save = () => {
    try {
      localStorage.setItem(KEY, String(nav.getBoundingClientRect().width | 0));
    } catch (_) {}
  };
  handle.addEventListener('mousedown', (e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = nav.getBoundingClientRect().width;
    handle.classList.add('dragging');
    document.body.style.cursor = 'col-resize';
    const move = (ev) => {
      nav.style.width = `${clamp(startW + ev.clientX - startX)}px`;
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      handle.classList.remove('dragging');
      document.body.style.cursor = '';
      save();
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  });
  handle.addEventListener('dblclick', () => {
    nav.style.width = '';
    try {
      localStorage.removeItem(KEY);
    } catch (_) {}
  });
})();

// ---- タブ ----
function showTab(name) {
  for (const b of document.querySelectorAll('#tabs button')) b.classList.toggle('active', b.dataset.tab === name);
  for (const s of document.querySelectorAll('main section')) s.classList.toggle('active', s.dataset.tab === name);
  document.querySelector('main').scrollTop = 0;
  if (window.HelpTips) window.HelpTips.close();
  if (name === 'thread' && typeof loadThreadChoices === 'function') loadThreadChoices();
  // 「スレッドのレス」を開いた時に、一番下(最新のレス)を表示する(設定でONの時)
  if (name === 'thread' && settings && settings.threadOpenAtBottom) {
    requestAnimationFrame(() => {
      const list = document.getElementById('threadList');
      list.scrollTop = list.scrollHeight;
    });
  }
}
document.getElementById('tabs').addEventListener('click', (e) => {
  const btn = e.target.closest('button[data-tab]');
  if (btn) showTab(btn.dataset.tab);
});
// 「声・速さを選ぶ」「スレッドのレス」など、ほかのタブを開くボタン
for (const btn of document.querySelectorAll('[data-open-tab]')) {
  btn.addEventListener('click', () => showTab(btn.dataset.openTab));
}

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
  // AquesTalkPlayerは/Tで渡した文字を読み上げて終わる(画面は出ない)。声を変える時は/P "プリセット名"
  aquestalk: { commandPath: 'C:\\aquestalkplayer\\AquesTalkPlayer.exe', commandArgs: '/T #Res#' },
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
      : kind === 'voicevox-only' ? http && (paramsEngine || engine) !== 'coeiroink'
      : kind === 'test' || kind === 'launch' ? engine !== 'command'
      : kind === engine;
    box.style.display = show ? '' : 'none';
  }
  document.getElementById('ttsExePath').value = exePaths()[engine] || '';
  document.getElementById('randomVoicesBox').style.display = settings.ttsRandom ? '' : 'none';
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
    renderRandomVoices();
  }
  if (engine !== 'command' && ttsExeCheckedFor !== engine) {
    ttsExeCheckedFor = engine;
    findExe(true);
  }
}

// ---- 読み上げソフトごとの話速・音量など(ttsEngineParams) ----
const PARAM_LEGACY = {
  speed: 'ttsSpeed', pitch: 'ttsPitch', intonation: 'ttsIntonation', volume: 'ttsVolume',
  preSilence: 'ttsPreSilence', postSilence: 'ttsPostSilence', pauseScale: 'ttsPauseScale',
};
let paramsEngine = null; // スライダーで調整しているソフト

/** 声に使っているソフト(共通の声のソフト + ランダムの声のソフト) */
function usedEngines() {
  const list = [];
  if (isHttpEngine(settings.speechEngine)) list.push(settings.speechEngine);
  if (settings.ttsRandom) {
    for (const v of randomVoices()) if (v.engine && !list.includes(v.engine)) list.push(v.engine);
  }
  return list;
}

/** スライダーを、選んでいるソフトの調整(ttsEngineParams.<ソフト>.<項目>)に結び付ける。
 * まだ調整が無いソフトは、これまでの共通の値から始める。 */
function bindParamSliders() {
  if (!settings) return;
  const engines = usedEngines();
  if (!engines.includes(paramsEngine)) paramsEngine = engines[0] || null;
  const sel = document.getElementById('ttsParamsEngine');
  sel.textContent = '';
  for (const e of engines) {
    const o = document.createElement('option');
    o.value = e;
    o.textContent = (RANDOM_ENGINES.find(([id]) => id === e) || [e, e === 'voicevoxcompat' ? 'VOICEVOX互換' : e])[1];
    sel.appendChild(o);
  }
  sel.value = paramsEngine || '';
  // 1つのソフトしか使っていない時は選ぶ欄を出さない
  document.getElementById('paramsEngineRow').style.display = engines.length > 1 ? '' : 'none';
  if (!paramsEngine) return;
  if (!settings.ttsEngineParams || typeof settings.ttsEngineParams !== 'object') settings.ttsEngineParams = {};
  if (!settings.ttsEngineParams[paramsEngine]) {
    const p = {};
    for (const [f, legacy] of Object.entries(PARAM_LEGACY)) p[f] = Number(settings[legacy] ?? document.querySelector(`[data-param="${f}"]`).dataset.default);
    settings.ttsEngineParams[paramsEngine] = p;
  }
  for (const el of document.querySelectorAll('[data-param]')) {
    el.dataset.key = `ttsEngineParams.${paramsEngine}.${el.dataset.param}`;
  }
}

document.getElementById('ttsParamsEngine').addEventListener('change', (e) => {
  paramsEngine = e.target.value;
  renderFields();
});

/** 読み上げソフトごとのexeの場所 */
function exePaths() {
  if (!settings.ttsExePaths || typeof settings.ttsExePaths !== 'object') settings.ttsExePaths = {};
  return settings.ttsExePaths;
}

/** 読み上げソフトのexeを探す。指定が無ければ見つかった場所を覚えておく(起動中の
 * プロセスから見つかった場所は、ソフトを閉じた後の自動起動に使うため)。
 * engineを省くと今選んでいるソフト(結果を画面に出す)。 */
async function findExe(quiet, engine) {
  const current = !engine;
  engine = engine || settings.speechEngine;
  const found = await invoke('tts_find_exe', { settings: { ...settings, speechEngine: engine } }).catch(() => null);
  const paths = exePaths();
  if (found && (!paths[engine] || !quiet)) {
    paths[engine] = found;
    if (current) document.getElementById('ttsExePath').value = found;
    scheduleSave();
  }
  if (!current) return;
  const msg = document.getElementById('ttsExeMsg');
  if (found) msg.textContent = `見つかりました: ${found}`;
  else if (!paths[engine]) msg.textContent = '見つかりませんでした。ソフトを起動してから「自動で探す」を押すか、「参照...」で場所を指定してください。';
  else if (!quiet) msg.textContent = '自動では見つかりませんでした(指定した場所を使います)。';
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

// 読み上げソフトごとの声の一覧(ランダムの声の欄で使い回す)
const speakerCache = {};

/** そのソフトの声の一覧を取る(今選んでいるソフトなら接続先の指定も使う)。 */
async function listSpeakers(engine) {
  const url = engine === settings.speechEngine ? settings.ttsUrl : '';
  const list = await invoke('tts_list_speakers', { settings: { ...settings, speechEngine: engine, ttsUrl: url } });
  speakerCache[engine] = list;
  return list;
}

async function fetchSpeakers(quiet) {
  const msg = document.getElementById('ttsSpeakerMsg');
  if (!quiet) msg.textContent = '取得中…';
  try {
    const list = await listSpeakers(settings.speechEngine);
    fillSpeakerSelects(list);
    // まだ声を選んでいなければ、最初の声にしておく
    if (!settings.ttsSpeaker && list.length) {
      settings.ttsSpeaker = list[0].id;
      fillSpeakerSelects(list);
      scheduleSave();
    }
    msg.textContent = `${list.length}種類の声があります。`;
    renderRandomVoices();
  } catch (e) {
    fillSpeakerSelects([]);
    msg.textContent = quiet ? '読み上げソフトを起動してから「声の一覧を取得」を押してください。' : String(e);
  }
}

// ---- ランダムの声(割合つきの一覧。別々の読み上げソフトの声を混ぜられる) ----
const RANDOM_ENGINES = [
  ['voicevox', 'VOICEVOX'],
  ['coeiroink', 'COEIROINK(v2)'],
  ['coeiroinkv1', 'COEIROINK(v1)'],
  ['aivisspeech', 'AivisSpeech'],
  ['sharevox', 'SHAREVOX'],
];
const randomFetching = new Set();

function randomVoices() {
  if (!Array.isArray(settings.ttsRandomVoices)) settings.ttsRandomVoices = [];
  return settings.ttsRandomVoices;
}

function renderRandomVoices() {
  const box = document.getElementById('randomVoices');
  if (!box || !settings.ttsRandom) return;
  const voices = randomVoices();
  const total = voices.reduce((a, v) => a + (Number(v.weight) > 0 ? Number(v.weight) : 0), 0);
  box.textContent = '';
  voices.forEach((v, i) => {
    const row = document.createElement('div');
    row.className = 'random-row';
    const eng = document.createElement('select');
    for (const [id, label] of RANDOM_ENGINES) {
      const o = document.createElement('option');
      o.value = id;
      o.textContent = label;
      eng.appendChild(o);
    }
    eng.value = v.engine;
    eng.addEventListener('change', () => {
      v.engine = eng.value;
      v.speaker = '';
      renderFields();
      scheduleSave();
    });
    const sp = document.createElement('select');
    const list = speakerCache[v.engine];
    const none = document.createElement('option');
    none.value = '';
    none.textContent = list ? '(選んでください)' : '(ソフトを起動すると一覧が出ます)';
    sp.appendChild(none);
    for (const s of list || []) {
      const o = document.createElement('option');
      o.value = s.id;
      o.textContent = s.name;
      sp.appendChild(o);
    }
    if (v.speaker && !(list || []).some((s) => s.id === v.speaker)) {
      const o = document.createElement('option');
      o.value = v.speaker;
      o.textContent = `(保存済み: ${v.speaker})`;
      sp.appendChild(o);
    }
    sp.value = v.speaker || '';
    sp.addEventListener('change', () => {
      v.speaker = sp.value;
      scheduleSave();
    });
    const w = document.createElement('input');
    w.type = 'number';
    w.min = '0';
    w.value = String(v.weight ?? 0);
    const pct = document.createElement('span');
    pct.className = 'unit';
    pct.textContent = total > 0 && v.weight > 0 ? `${Math.round((v.weight / total) * 100)}%` : '0%';
    w.addEventListener('input', () => {
      v.weight = Math.max(0, Math.floor(Number(w.value) || 0));
      scheduleSave();
    });
    w.addEventListener('change', renderRandomVoices);
    const del = document.createElement('button');
    del.className = 'secondary';
    del.textContent = '削除';
    del.addEventListener('click', () => {
      voices.splice(i, 1);
      renderFields();
      scheduleSave();
    });
    row.append(eng, sp, w, pct, del);
    box.appendChild(row);
    // まだ一覧が無いソフトは取りに行く(起動していればそのソフトのexeの場所も覚える)
    if (!list && !randomFetching.has(v.engine)) {
      randomFetching.add(v.engine);
      const engine = v.engine;
      listSpeakers(engine)
        .then(() => {
          findExe(true, engine);
          renderRandomVoices();
        })
        .catch(() => {})
        .finally(() => setTimeout(() => randomFetching.delete(engine), 5000));
    }
  });
  document.getElementById('randomVoicesMsg').textContent = voices.length
    ? '割合は数字の比率です(合計が100でなくても大丈夫)。0にすると使いません。'
    : '「声を追加」で、ランダムに使う声を足してください(一覧が空のうちは上の「声」で読みます)。';
}

document.getElementById('randomAddBtn').addEventListener('click', () => {
  const engine = RANDOM_ENGINES.some(([id]) => id === settings.speechEngine) ? settings.speechEngine : 'voicevox';
  randomVoices().push({ engine, speaker: '', weight: 50 });
  renderFields();
  scheduleSave();
});
document.getElementById('ttsRandom').addEventListener('change', renderFields);

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

// 読み上げソフトの選択は「かんたん設定」と「声・速さ」の2か所にある
for (const sel of document.querySelectorAll('select[data-engine-select]')) sel.addEventListener('change', () => {
  // ソフトが変わると接続先も声の番号も別物になるので、選び直してもらう
  settings.ttsUrl = '';
  settings.ttsSpeaker = '';
  settings.ttsSpeakerYoutube = '';
  settings.ttsSpeakerTwitch = '';
  document.getElementById('ttsSpeakerMsg').textContent = '';
  document.getElementById('ttsTestMsg').textContent = '';
  document.getElementById('ttsExeMsg').textContent = '';
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
document.getElementById('ttsExePath').addEventListener('input', (e) => {
  exePaths()[settings.speechEngine] = e.target.value;
  scheduleSave();
});
document.getElementById('ttsPickExe').addEventListener('click', async () => {
  const path = await invoke('comments_pick_exe').catch(() => null);
  if (!path) return;
  exePaths()[settings.speechEngine] = path;
  document.getElementById('ttsExeMsg').textContent = '';
  renderFields();
  scheduleSave();
});
// ---- アドオン(src-tauri/src/addons.rs。作り方はリポジトリの「アドオンの作り方.md」) ----
const PAGE_LABELS = { subtitle: 'コメント字幕', comments: 'コメント一覧', nico: '流れるコメント', voice: '自分の声の字幕', audience: '視聴者数', poll: 'アンケート', overlay: 'リアクション' };
async function renderAddons() {
  const listEl = document.getElementById('addonsList');
  let data;
  try {
    data = await invoke('addons_list');
  } catch (e) {
    listEl.textContent = String(e);
    return;
  }
  if (!data) return;
  document.getElementById('addonsSafeMode').checked = !!data.safeMode;
  document.getElementById('addonsFolderNote').textContent = `アドオンのフォルダ: ${data.folder}(この中に、アドオンごとのフォルダを入れます)`;
  listEl.textContent = '';
  if (!data.addons.length) {
    const p = document.createElement('div');
    p.className = 'addon-empty';
    p.textContent = 'アドオンはまだありません。もらったアドオンのフォルダを、「アドオンのフォルダを開く」で開いた所に入れて、「読み込み直す」を押してください。';
    listEl.appendChild(p);
  }
  for (const a of data.addons) {
    const card = document.createElement('label');
    card.className = 'addon-card';
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.checked = a.enabled;
    box.disabled = !a.compatible || !!a.error;
    box.addEventListener('change', () => {
      invoke('addons_set_enabled', { id: a.id, on: box.checked }).catch((e) => showStatusError(String(e)));
    });
    card.appendChild(box);
    const main = document.createElement('div');
    main.className = 'addon-main';
    const line = (cls, text) => {
      if (!text) return;
      const d = document.createElement('div');
      d.className = cls;
      d.textContent = text;
      main.appendChild(d);
    };
    line('addon-name', a.name);
    line('addon-meta', [a.author && `作った人: ${a.author}`, a.version && `版: ${a.version}`, `フォルダ: ${a.id}`].filter(Boolean).join(' / '));
    line('addon-desc', a.description);
    const uses = [];
    if (a.cssPages.length) uses.push(`見た目(${a.cssPages.map((p) => PAGE_LABELS[p] || p).join('・')})`);
    if (a.names) uses.push(`名前を置き換える(${a.names}個の名前)`);
    if (a.tts.length) uses.push(`読み上げソフトのボタン(${a.tts.map((t) => t.label).join('・')})`);
    line('addon-uses', uses.length ? `変えるもの: ${uses.join('、')}` : '');
    if (!a.compatible) line('addon-err', `このアドオンには、ReaCast ${a.minReaCast} 以降が必要です(今は ${data.appVersion})。ReaCastを更新してください。`);
    line('addon-err', a.error);
    card.appendChild(main);
    listEl.appendChild(card);
  }
  renderAddonCommandPresets();
}
async function renderAddonCommandPresets() {
  const el = document.getElementById('addonCommandPresets');
  const presets = await invoke('addons_tts_presets').catch(() => []);
  el.textContent = '';
  for (const p of presets) {
    const b = document.createElement('button');
    b.className = 'secondary';
    b.textContent = `${p.label}(アドオン)`;
    b.title = `${p.commandPath} ${p.commandArgs}`;
    b.addEventListener('click', () => confirmAddonTts(p));
    el.appendChild(b);
  }
}
// アドオンの読み上げソフトのボタン: 動かすソフトの場所を見せて、確かめてから欄に入れる
function confirmAddonTts(p) {
  const box = document.getElementById('addonTtsConfirm');
  box.textContent = '';
  box.classList.remove('hidden');
  const line = (cls, text) => {
    const d = document.createElement('div');
    if (cls) d.className = cls;
    d.textContent = text;
    box.appendChild(d);
  };
  line('', `アドオン「${p.addon}」の「${p.label}」を使う設定にします。読み上げのたびに、次のソフトを動かします:`);
  line('addon-tts-path', `${p.commandPath} ${p.commandArgs}`);
  if (p.inAddon) {
    line('addon-tts-warn', '⚠ このソフトは、アドオンのフォルダに入っていたものです。アドオンを作った人を信頼できる時だけ使ってください(知らない人のソフトは、ウイルスのことがあります)。');
  }
  const row = document.createElement('div');
  row.className = 'inline';
  const ok = document.createElement('button');
  ok.textContent = 'この設定にする';
  ok.addEventListener('click', () => {
    settings.commandPath = p.commandPath;
    settings.commandArgs = p.commandArgs;
    box.classList.add('hidden');
    renderFields();
    scheduleSave();
  });
  const cancel = document.createElement('button');
  cancel.className = 'secondary';
  cancel.textContent = 'やめる';
  cancel.addEventListener('click', () => box.classList.add('hidden'));
  row.append(ok, cancel);
  box.appendChild(row);
}
document.getElementById('addonsOpenFolder').addEventListener('click', () => invoke('addons_open_folder').catch((e) => showStatusError(String(e))));
document.getElementById('addonsReload').addEventListener('click', () => invoke('addons_reload').then(renderAddons).catch((e) => showStatusError(String(e))));
document.getElementById('addonsGuide').addEventListener('click', () => invoke('comments_open_addon_guide').catch((e) => showStatusError(String(e))));
document.getElementById('addonsSafeMode').addEventListener('change', (e) => {
  invoke('addons_set_safe_mode', { on: e.target.checked }).catch((err) => showStatusError(String(err)));
});
listen('addons:changed', renderAddons);
renderAddons();

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

// ---- 全部の設定を最初の状態に戻す(押し間違いを防ぐため、5秒以内にもう一度押した時だけ) ----
(() => {
  const btn = document.getElementById('resetAllBtn');
  const msg = document.getElementById('resetAllMsg');
  let armedUntil = 0;
  let timer = null;
  btn.addEventListener('click', async () => {
    if (Date.now() > armedUntil) {
      armedUntil = Date.now() + 5000;
      btn.textContent = '本当に戻す(もう一度押す)';
      msg.textContent = 'すべての設定が消えます。戻す時は5秒以内にもう一度押してください';
      clearTimeout(timer);
      timer = setTimeout(() => {
        btn.textContent = '全部の設定を最初の状態に戻す';
        msg.textContent = '';
      }, 5000);
      return;
    }
    armedUntil = 0;
    clearTimeout(timer);
    btn.textContent = '全部の設定を最初の状態に戻す';
    try {
      clearTimeout(saveTimer);
      settings = await invoke('comments_reset_settings');
      renderFields();
      msg.textContent = '最初の状態に戻しました';
    } catch (e) {
      msg.textContent = `戻せませんでした: ${e}`;
    }
  });
})();

// ---- タブごとの「このタブの設定を最初の状態に戻す」 ----
// 戻すのは、そのタブにある欄の設定だけ(ほかのタブの設定はそのまま)。欄が無くてもそのタブの
// 設定とみなすものは下の表に書く。見た目のタブは、プリセットとその手直しも含めて見た目を丸ごと戻す。
const TAB_RESET_EXTRA = {
  speech: ['ttsEngineParams', 'ttsRandomVoices', 'ttsExePaths', ...Object.values(PARAM_LEGACY)],
  output: ['desktopRect', 'desktopListRect', 'desktopMonitorId'],
};
const TAB_RESET_STYLE = { subtitle: 'subtitle', list: 'list', nico: 'nico', aa: 'aa', voice: 'voice' };

async function resetTab(section) {
  const tab = section.dataset.tab;
  const defaults = await invoke('comments_default_settings');
  defaults.style = C.mergeStyle(null);
  const keyOf = (el) => el.dataset.key || el.dataset.hotkey;
  const inTab = (el) => el.closest('section') === section;
  const all = Array.from(document.querySelectorAll('[data-key], [data-hotkey]'));
  // 話速などのスライダーはソフトごとの欄(ttsEngineParams.<ソフト>.…)なので、ttsEngineParamsごと戻す
  const keys = new Set(all.filter(inTab).map(keyOf).filter((k) => !k.startsWith('ttsEngineParams.')));
  for (const k of TAB_RESET_EXTRA[tab] || []) keys.add(k);
  const part = TAB_RESET_STYLE[tab];
  if (part) {
    // 見た目を丸ごと戻す。ただし、ほかのタブにある欄の値(「デスクトップに表示」タブの
    // 「プリセットの見た目にする」など)は残す
    const keep = all.filter((el) => !inTab(el) && keyOf(el).startsWith(`style.${part}.`)).map((el) => [keyOf(el), getPath(settings, keyOf(el))]);
    settings.style[part] = JSON.parse(JSON.stringify(defaults.style[part]));
    if (settings.style.presets && settings.style.presets[part]) delete settings.style.presets[part];
    for (const [k, v] of keep) setPath(settings, k, v);
    for (const k of keys) if (k.startsWith(`style.${part}.`)) keys.delete(k);
  }
  for (const k of keys) {
    const v = getPath(defaults, k);
    setPath(settings, k, v === undefined ? null : JSON.parse(JSON.stringify(v)));
  }
  renderFields();
  scheduleSave();
  if (section.querySelector('[data-hotkey]')) setTimeout(() => applyHotkeys(), 400);
}

for (const section of document.querySelectorAll('main > section[data-tab]')) {
  if (['thread', 'misc', 'addons'].includes(section.dataset.tab)) continue;
  const row = document.createElement('div');
  row.className = 'inline tab-reset';
  const btn = document.createElement('button');
  btn.className = 'secondary';
  const LABEL = 'このタブの設定を最初の状態に戻す';
  btn.textContent = LABEL;
  const msg = document.createElement('span');
  msg.className = 'unit';
  row.append(btn, msg);
  section.appendChild(row);
  let armedUntil = 0;
  let timer = null;
  btn.addEventListener('click', async () => {
    if (Date.now() > armedUntil) {
      armedUntil = Date.now() + 5000;
      btn.textContent = '本当に戻す(もう一度押す)';
      msg.textContent = 'このタブの設定だけが最初の状態に戻ります。戻す時は5秒以内にもう一度押してください';
      clearTimeout(timer);
      timer = setTimeout(() => {
        btn.textContent = LABEL;
        msg.textContent = '';
      }, 5000);
      return;
    }
    armedUntil = 0;
    clearTimeout(timer);
    btn.textContent = LABEL;
    try {
      await resetTab(section);
      msg.textContent = 'このタブの設定を最初の状態に戻しました';
    } catch (e) {
      msg.textContent = `戻せませんでした: ${e}`;
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

// ---- フォント名: 手で入力する欄 + 全部のフォントから選ぶ一覧 ----
// (以前は入力欄の候補(datalist)だけだったが、入力済みの文字に合うものしか出ないため、
// 「BIZ UDゴシック」が入っているとBIZ UD…しか選べないように見えていた)
let fontNames = { web: Object.keys(C.WEB_FONTS), installed: [] };
const fontSelects = [];

function fillFontSelect(sel, current) {
  sel.textContent = '';
  const head = document.createElement('option');
  head.value = '';
  head.textContent = '一覧から選ぶ…';
  sel.appendChild(head);
  for (const [label, names] of [
    ['ウェブフォント(ネットから読み込みます)', fontNames.web],
    ['このPCのフォント', fontNames.installed],
  ]) {
    if (!names.length) continue;
    const group = document.createElement('optgroup');
    group.label = label;
    for (const n of names) {
      const o = document.createElement('option');
      o.value = n;
      o.textContent = n;
      group.appendChild(o);
    }
    sel.appendChild(group);
  }
  sel.value = [...fontNames.web, ...fontNames.installed].includes(current) ? current : '';
}

function setupFontPickers() {
  for (const input of document.querySelectorAll('input[list="fontList"]')) {
    input.removeAttribute('list');
    const row = document.createElement('div');
    row.className = 'font-row';
    input.parentNode.insertBefore(row, input);
    row.appendChild(input);
    const sel = document.createElement('select');
    sel.title = '全部のフォントから選べます(右の欄に直接入力もできます。「A, B」のように2つ並べると、Aに無い文字はBで表示します)';
    row.appendChild(sel);
    fillFontSelect(sel, input.value);
    sel.addEventListener('change', () => {
      if (!sel.value) return;
      input.value = sel.value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    input.addEventListener('input', () => fillFontSelect(sel, input.value));
    fontSelects.push([sel, input]);
  }
}
setupFontPickers();

/** 設定を読み込み直した時など、入力欄に合わせて一覧の選択を直す */
function renderFontPickers() {
  for (const [sel, input] of fontSelects) fillFontSelect(sel, input.value);
}

(async () => {
  fontNames.installed = await invoke('list_system_fonts').catch(() => []);
  renderFontPickers();
})();

// ---- コメント一覧のプリセット ----
// デザインに関わる項目だけを置き換える(フォント・件数・表示順などはそのまま)
const LIST_PRESETS = {
  // 掲示板風: 昔の2ちゃんねるのレス。「番号 ：名前：日付」の見出し(名前は緑の太字)、
  // 本文は少し字下げ。新しいレスは黄色く光ってから灰色に戻る
  bbs: {
    fontFamily: 'ＭＳ Ｐゴシック',
    headFontSize: 0, headBold: false, numberColor: '#000000', nameColor: '#228b22', dateColor: '#000000',
    bodyColor: '#000000', bodyBold: false, outlineWidth: 0, outlineColor: '#ffffff',
    itemBgColor: '#efefef', itemBgOpacity: 100, itemPadding: 8, itemRadius: 0,
    customCss: [
      '.item:not(.aa) { border-bottom: 1px solid #c4c4c4; animation: bbs-new 2000ms ease-out; }',
      // 区切りの「：」は、後ろに名前・日付がある時だけ(見出しに出すものは各自の設定のまま)
      ".head .no:not(:last-child)::after { content: ' ：'; }",
      ".head .name { font-weight: bold; }",
      ".head .name:not(:last-child)::after { content: '：'; color: #000000; font-weight: normal; }",
      '.body { margin-left: 1.6em; }',
      '@keyframes bbs-new { 0%, 35% { background-color: #ffffcc; } }',
    ].join('\n'),
  },
  default: {},
  yellowLine: {
    fontFamily: 'M PLUS 1p',
    headFontSize: 14, headBold: true, numberColor: '#ff6060', nameColor: '#60c0ff', dateColor: '#bbbbbb',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 2, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 50, itemPadding: 6, itemRadius: 6,
    customCss: '.item { border-left: 4px solid #ffd23f; }',
  },
  bubble: {
    fontFamily: 'Zen Maru Gothic',
    headFontSize: 13, headBold: true, numberColor: '#e05a00', nameColor: '#555555', dateColor: '#888888',
    bodyColor: '#222222', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#ffffff', itemBgOpacity: 90, itemPadding: 8, itemRadius: 14,
    customCss: '.item { box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35); }',
  },
  neon: {
    fontFamily: 'RocknRoll One',
    headFontSize: 14, headBold: true, numberColor: '#3cf2ff', nameColor: '#ff4fd8', dateColor: '#9a9ab8',
    bodyColor: '#ffffff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#120a24', itemBgOpacity: 60, itemPadding: 6, itemRadius: 4,
    customCss: '.body { text-shadow: 0 0 6px #3cf2ff, 0 0 12px #3cf2ff; }\n.item { border: 1px solid rgba(60, 242, 255, 0.4); }',
  },
  // ペルソナ5風: 黒い札に白い太字、左に赤い太線と赤い影。札は全部同じ角度で少しだけ斜めに
  // (交互に傾けたり、番号・名前まで別々に傾けるとごちゃついて読みにくかったため)。
  // 本文は読みやすい太めのゴシック、番号だけ赤い札に極太の文字でアクセント
  p5: {
    fontFamily: 'Zen Kaku Gothic New',
    headFontSize: 13, headBold: true, numberColor: '#ffffff', nameColor: '#ff5a5a', dateColor: '#9a9a9a',
    bodyColor: '#ffffff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#0b0b0b', itemBgOpacity: 92, itemPadding: 8, itemRadius: 0,
    customCss: [
      "@import url('https://fonts.googleapis.com/css2?family=Dela+Gothic+One&display=swap');",
      '.item { transform: skewX(-8deg); border-left: 6px solid #e60012; box-shadow: 4px 4px 0 #e60012; margin: 0 10px 0 6px; }',
      '.item .content { transform: skewX(8deg); }',
      ".head .no { background: #e60012; padding: 0 0.45em; font-family: 'Dela Gothic One', sans-serif; font-weight: normal; letter-spacing: 0.04em; }",
      '.body { line-height: 1.45; letter-spacing: 0.02em; }',
    ].join('\n'),
  },
  // FF風(スーパーファミコンのRPGのウィンドウ風): 青いグラデーションの地、白い縁取りの角丸、
  // 影付きの白い文字、ドット文字、名前の前に指のカーソル
  ff: {
    fontFamily: 'DotGothic16',
    headFontSize: 0, headBold: false, numberColor: '#ffe27a', nameColor: '#ffffff', dateColor: '#aeb8ff',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#1a2a8c', itemBgOpacity: 100, itemPadding: 10, itemRadius: 6,
    customCss: [
      '.item:not(.aa) { -webkit-font-smoothing: none; }',
      '.item { background: linear-gradient(180deg, #5b6fe0 0%, #2a3aa8 45%, #101a6c 100%) !important; border: 3px solid #eef0ff; border-radius: 7px; box-shadow: 0 0 0 2px #1a1a33, inset 0 0 0 1px #8090e0, 2px 3px 0 3px rgba(0, 0, 0, 0.35); }',
      '.head, .body { text-shadow: 2px 2px 0 #10103a; }',
      ".head .name::before { content: '☞'; margin-right: 0.3em; color: #ffffff; }",
    ].join('\n'),
  },
  // ゆめかわ(女性の配信者向けのかわいい系): パステルピンクの角丸の札、丸ゴシック、
  // 番号の前にハート、右上にきらっ
  cute: {
    fontFamily: 'Zen Maru Gothic',
    headFontSize: 13, headBold: true, numberColor: '#ff6fb1', nameColor: '#b28dff', dateColor: '#c9a3c0',
    bodyColor: '#7a3b69', bodyBold: true, outlineWidth: 0, outlineColor: '#ffffff',
    itemBgColor: '#ffe9f4', itemBgOpacity: 95, itemPadding: 9, itemRadius: 16,
    customCss: [
      '.item { border: 2px solid #ffb6d9; box-shadow: 0 3px 0 #ffc9e3, 0 4px 10px rgba(255, 105, 180, 0.25); }',
      ".head .no::before { content: '♡ '; color: #ff8cc6; }",
      ".item::after { content: '✦'; position: absolute; right: 8px; top: 4px; color: #b28dff; font-size: 0.75em; }",
    ].join('\n'),
  },
  // 病みかわ: パステルピンクの札に黒い太枠と黒い影、内側にピンクの縫い目。角にばんそうこう、
  // 番号の前に十字。新しいレスはぽんっと出て、ふるふるっと震える
  yami: {
    fontFamily: 'Hachi Maru Pop',
    headFontSize: 13, headBold: false, numberColor: '#9b6bd6', nameColor: '#e0457b', dateColor: '#a7879f',
    bodyColor: '#3a1f3d', bodyBold: false, outlineWidth: 0, outlineColor: '#ffffff',
    itemBgColor: '#ffdbe9', itemBgOpacity: 96, itemPadding: 10, itemRadius: 14,
    customCss: [
      '.item { border: 2px solid #3a1f3d; box-shadow: 3px 3px 0 #3a1f3d; outline: 2px dashed #ff9cc2; outline-offset: -6px; margin-right: 4px; margin-bottom: calc(var(--item-gap, 10px) + 4px); animation: yami-in 700ms ease-out; }',
      ".head .no::before { content: '✚ '; color: #ff8fb8; }",
      ".item::after { content: ''; position: absolute; right: 8px; top: 3px; width: 1.7em; height: 0.56em; transform: rotate(28deg); background: linear-gradient(90deg, #ffc9d6 30%, transparent 30%, transparent 70%, #ffc9d6 70%), radial-gradient(circle, #e9a0b4 28%, transparent 32%) center / 0.2em 0.2em, #fff0f4; border: 1px solid #e9a0b4; border-radius: 0.3em; }",
      '@keyframes yami-in { 0% { opacity: 0; transform: scale(0.6) rotate(4deg); } 35% { opacity: 1; transform: scale(1.04) rotate(-2deg); } 50% { transform: translateX(-3px) rotate(1deg); } 62% { transform: translateX(3px) rotate(-1deg); } 74% { transform: translateX(-1px); } 100% { transform: none; } }',
    ].join('\n'),
  },
  // 羽ペン: 古い紙に羽ペンで書いたような。英字は筆記体(Dancing Script)、日本語はペン字風(Zen Kurenaido)。
  // 紙の色の札に、うすい罫線とセピア色のインク
  quill: {
    fontFamily: 'Dancing Script, Zen Kurenaido',
    headFontSize: 13, headBold: false, numberColor: '#8a4b1c', nameColor: '#5a3b22', dateColor: '#a08a6a',
    bodyColor: '#3b2a1a', bodyBold: false, outlineWidth: 0, outlineColor: '#ffffff',
    itemBgColor: '#f6eedb', itemBgOpacity: 97, itemPadding: 9, itemRadius: 3,
    customCss: [
      '.item { border: 1px solid #d8c7a0; box-shadow: 1px 2px 0 rgba(90, 60, 20, 0.25), 0 3px 8px rgba(0, 0, 0, 0.25); background-image: repeating-linear-gradient(transparent 0, transparent 1.35em, rgba(140, 110, 70, 0.18) 1.35em, rgba(140, 110, 70, 0.18) calc(1.35em + 1px)); }',
      ".head .no::after { content: ' ✒'; color: #8a4b1c; }",
    ].join('\n'),
  },
  // DQ風(スーパーファミコンのドラクエ): 黒いウィンドウに白い角丸の太い縁、白いドット文字、
  // 名前の前に「＊」、一番新しいレスの最後に点滅する▼
  dq: {
    fontFamily: 'DotGothic16',
    headFontSize: 0, headBold: false, numberColor: '#ffffff', nameColor: '#ffffff', dateColor: '#bbbbbb',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 92, itemPadding: 10, itemRadius: 8,
    customCss: [
      '.item:not(.aa) { -webkit-font-smoothing: none; border: 3px solid #ffffff; box-shadow: 0 0 0 2px #000000, inset 0 0 0 2px #000000; animation: dq-pop 150ms steps(3, end); }',
      ".head .name::before { content: '＊'; margin-right: 0.15em; }",
      "#list:not(.bottom) .item:first-child .body::after, #list.bottom .item:last-child .body::after { content: ' ▼'; animation: dq-blink 0.8s steps(1) infinite; }",
      '@keyframes dq-blink { 50% { opacity: 0; } }',
      '@keyframes dq-pop { from { transform: scale(1, 0.1); } to { transform: scale(1, 1); } }',
    ].join('\n'),
  },
  // OC風: 右寄せ、白い名前の札+黒い本文の吹き出しに稲妻のしっぽ。右からバチッと飛び込む。
  // 応援(スパチャ等)は本文の吹き出しがその色になる
  oc: {
    fontFamily: 'Noto Sans JP',
    headFontSize: 12, headBold: true, numberColor: '#000000', nameColor: '#000000', dateColor: '#555555',
    bodyColor: '#ffffff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 0, itemPadding: 2, itemRadius: 0,
    customCss: [
      '.item { flex-direction: row-reverse; text-align: right; animation: oc-bolt 420ms cubic-bezier(.2, 1.4, .4, 1); }',
      '.item .content { display: flex; flex-direction: column; align-items: flex-end; margin-right: 18px; }',
      '.head { background: #ffffff; border: 2px solid #111111; padding: 0 0.5em; margin-right: 14px; margin-bottom: -3px; box-shadow: 2px 2px 0 rgba(0, 0, 0, 0.25); position: relative; z-index: 1; }',
      '.body { position: relative; background: #111111; padding: 0.25em 0.7em; box-shadow: 4px 4px 0 rgba(0, 0, 0, 0.2); text-align: left; }',
      ".body::after { content: ''; position: absolute; right: -16px; bottom: 4px; width: 20px; height: 24px; background: inherit; clip-path: polygon(0 10%, 100% 0, 55% 45%, 100% 55%, 10% 100%, 35% 55%, 0 60%); }",
      '.item.paid { background: transparent !important; box-shadow: none; }',
      '.item.paid .body { background: var(--paid-color); color: var(--paid-text) !important; }',
      '.item.paid .head span:not(.paid-badge) { color: #000000 !important; }',
      '@keyframes oc-bolt { 0% { opacity: 0; transform: translateX(70px) skewX(-14deg); } 60% { opacity: 1; transform: translateX(-8px) skewX(4deg); } 80% { transform: translateX(3px); } 100% { transform: translateX(0); } }',
    ].join('\n'),
  },
  // オーロラ(おすすめ): 夜空色のすりガラスの札に、ゆっくり色が流れるオーロラの縁。
  // 見出しはオーロラ色の文字、ぼかしからふわっと浮かび上がる
  aurora: {
    fontFamily: 'M PLUS Rounded 1c',
    headFontSize: 12, headBold: true, numberColor: '#9ad8ff', nameColor: '#c9b6ff', dateColor: '#8f9bc0',
    bodyColor: '#f2f5ff', bodyBold: true, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#0d1230', itemBgOpacity: 72, itemPadding: 10, itemRadius: 14,
    customCss: [
      '.item:not(.paid) { border: 2px solid transparent; background: linear-gradient(rgba(13, 18, 48, 0.78), rgba(13, 18, 48, 0.78)) padding-box, linear-gradient(90deg, #7ce7ff, #a68cff, #ff8fd8, #7ce7ff) border-box !important; background-size: 100% 100%, 300% 100% !important; animation: aurora-in 600ms ease-out, aurora-flow 8s linear infinite; box-shadow: 0 4px 18px rgba(80, 120, 255, 0.25); }',
      '.head .no, .head .name { background: linear-gradient(90deg, #7ce7ff, #c79bff); -webkit-background-clip: text; background-clip: text; color: transparent !important; }',
      '.body { text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5); }',
      '@keyframes aurora-flow { from { background-position: 0 0, 0% 0; } to { background-position: 0 0, 300% 0; } }',
      '@keyframes aurora-in { from { opacity: 0; filter: blur(8px); transform: translateY(12px); } to { opacity: 1; filter: blur(0); transform: translateY(0); } }',
    ].join('\n'),
  },
  // レトロ(ファミコン風): ドット文字、黒地に白い二重枠のメッセージウィンドウ
  // 英数字はファミコン風のPress Start 2P、日本語はドット文字のDotGothic16
  retro: {
    fontFamily: 'Press Start 2P, DotGothic16',
    headFontSize: 0, headBold: false, numberColor: '#f8d800', nameColor: '#7cf87c', dateColor: '#a8a8a8',
    bodyColor: '#ffffff', bodyBold: false, outlineWidth: 0, outlineColor: '#000000',
    itemBgColor: '#000000', itemBgOpacity: 100, itemPadding: 10, itemRadius: 4,
    customCss: [
      '.item:not(.aa) { -webkit-font-smoothing: none; }',
      '.item { border: 3px solid #ffffff; box-shadow: 0 0 0 3px #000000, inset 0 0 0 3px #000000; }',
      ".head .name::before { content: '▶'; margin-right: 0.2em; }",
      // 一番新しいレスの最後に、点滅する「▼」(新しい順でも古い順でも)
      "#list:not(.bottom) .item:first-child .body::after, #list.bottom .item:last-child .body::after { content: ' ▼'; animation: retro-blink 1s steps(1) infinite; }",
      '@keyframes retro-blink { 50% { opacity: 0; } }',
    ].join('\n'),
  },
};
// ---- コメント字幕のプリセット(コメント一覧と同じデザイン+そのデザインに合う出し方) ----
const SUBTITLE_PRESETS = {
  default: {},
  // 掲示板風: 昔の2ちゃんねるのレス(灰色の背景・ＭＳ Ｐゴシック・名前は緑の太字)。
  // 回線が遅かった頃のように上から少しずつ読み込まれて、新着の黄色がすっと消える
  bbs: {
    fontFamily: 'ＭＳ Ｐゴシック', bold: false, italic: false, color: '#000000', outlineWidth: 0, outlineColor: '#ffffff',
    bgColor: '#efefef', bgOpacity: 100, padding: 10, radius: 0, appear: 'bbs-load', translationColor: '#800000',
    customCss: [
      '#box { border: 1px solid #a8a8a8; box-shadow: 2px 2px 0 rgba(0, 0, 0, 0.3); line-height: 1.45; }',
      '.res-head { color: #228b22; font-weight: bold; margin-bottom: 0.15em; }',
    ].join('\n'),
  },
  yellowLine: {
    fontFamily: 'M PLUS 1p', bold: true, color: '#ffffff', outlineWidth: 2, outlineColor: '#000000',
    bgColor: '#000000', bgOpacity: 55, padding: 8, radius: 6, appear: 'wipe-left', translationColor: '#ffe066',
    customCss: '#box { border-left: 7px solid #ffd23f; }',
  },
  // 吹き出し: 左(キャラクターの側)にしっぽ、左から膨らんで出てくる
  bubble: {
    fontFamily: 'Zen Maru Gothic', bold: true, color: '#333333', outlineWidth: 0, outlineColor: '#ffffff',
    bgColor: '#ffffff', bgOpacity: 96, padding: 10, radius: 18, appear: 'pop-left', translationColor: '#c05000',
    customCss: [
      '#box { margin-left: 16px; box-shadow: 0 3px 8px rgba(0, 0, 0, 0.35); }',
      "#box::before { content: ''; position: absolute; left: -15px; bottom: 16px; border-style: solid; border-width: 9px 16px 9px 0; border-color: transparent rgba(255, 255, 255, 0.96) transparent transparent; }",
      '.res-head { color: #e05a00; }',
    ].join('\n'),
  },
  neon: {
    fontFamily: 'RocknRoll One', bold: false, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#120a24', bgOpacity: 55, padding: 8, radius: 6, appear: 'neon-flicker', translationColor: '#ff9df0',
    customCss: [
      '#box { text-shadow: 0 0 6px #3cf2ff, 0 0 14px #3cf2ff !important; border: 1px solid rgba(60, 242, 255, 0.5); }',
      '.res-head { color: #ff4fd8; text-shadow: 0 0 6px #ff4fd8; }',
    ].join('\n'),
  },
  p5: {
    fontFamily: 'Zen Kaku Gothic New', bold: true, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#0b0b0b', bgOpacity: 92, padding: 9, radius: 0, appear: 'p5-slash', translationColor: '#ff5a5a',
    customCss: [
      "@import url('https://fonts.googleapis.com/css2?family=Dela+Gothic+One&display=swap');",
      '#box { border-left: 8px solid #e60012; box-shadow: 6px 6px 0 #e60012; }',
      ".res-head { display: table; background: #e60012; color: #ffffff; padding: 0 0.45em; margin-bottom: 0.2em; font-family: 'Dela Gothic One', sans-serif; font-weight: normal; transform: skewX(-10deg); }",
    ].join('\n'),
  },
  ff: {
    fontFamily: 'DotGothic16', bold: false, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#1a2a8c', bgOpacity: 100, padding: 12, radius: 8, appear: 'ff-open', translationColor: '#ffe27a',
    customCss: [
      '#box { background: linear-gradient(180deg, #5b6fe0 0%, #2a3aa8 45%, #101a6c 100%) !important; border: 3px solid #eef0ff; box-shadow: 0 0 0 2px #1a1a33, inset 0 0 0 1px #8090e0; text-shadow: 2px 2px 0 #10103a !important; -webkit-font-smoothing: none; }',
      '.res-head { color: #ffe27a; }',
    ].join('\n'),
  },
  retro: {
    fontFamily: 'Press Start 2P, DotGothic16', bold: false, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#000000', bgOpacity: 100, padding: 12, radius: 4, appear: 'typewriter', translationColor: '#7cf87c',
    customCss: [
      '#box { border: 3px solid #ffffff; box-shadow: 0 0 0 3px #000000, inset 0 0 0 3px #000000; -webkit-font-smoothing: none; }',
      '.res-head { color: #f8d800; }',
      "#box::after { content: ' ▼'; animation: retro-blink 1s steps(1) infinite; }",
      '@keyframes retro-blink { 50% { opacity: 0; } }',
    ].join('\n'),
  },
  // 羽ペン: 紙に羽ペンで、インクがにじむように左から書かれていく
  quill: {
    fontFamily: 'Dancing Script, Zen Kurenaido', bold: false, color: '#3b2a1a', outlineWidth: 0, outlineColor: '#ffffff',
    bgColor: '#f6eedb', bgOpacity: 97, padding: 12, radius: 3, appear: 'quill-write', translationColor: '#8a4b1c',
    customCss: [
      '#box { border: 1px solid #d8c7a0; box-shadow: 2px 3px 0 rgba(90, 60, 20, 0.25), 0 4px 12px rgba(0, 0, 0, 0.3); background-image: repeating-linear-gradient(transparent 0, transparent 1.3em, rgba(140, 110, 70, 0.18) 1.3em, rgba(140, 110, 70, 0.18) calc(1.3em + 1px)); }',
      ".res-head { color: #8a4b1c; }",
      ".res-head::after { content: ' ✒'; }",
    ].join('\n'),
  },
  // DQ風: 黒いウィンドウに白い角丸の太い縁、ドット文字で1文字ずつ。「＊」と点滅する▼
  dq: {
    fontFamily: 'DotGothic16', bold: false, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#000000', bgOpacity: 95, padding: 12, radius: 10, appear: 'typewriter', translationColor: '#ffe27a',
    customCss: [
      '#box { -webkit-font-smoothing: none; border: 4px solid #ffffff; box-shadow: 0 0 0 3px #000000, inset 0 0 0 3px #000000; }',
      ".res-head::before { content: '＊ '; }",
      "#box::after { content: ' ▼'; animation: dq-blink 0.8s steps(1) infinite; }",
      '@keyframes dq-blink { 50% { opacity: 0; } }',
    ].join('\n'),
  },
  // OC風: 白い名前の札+黒い吹き出し、右に稲妻のしっぽ。右からバチッと飛び込む
  oc: {
    fontFamily: 'Noto Sans JP', bold: true, color: '#ffffff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#111111', bgOpacity: 100, padding: 8, radius: 0, appear: 'oc-bolt', translationColor: '#ffd23f',
    customCss: [
      '#box { margin: 1.1em 22px 0 0; box-shadow: 5px 5px 0 rgba(0, 0, 0, 0.22); }',
      '.res-head { display: table; background: #ffffff; color: #111111; border: 2px solid #111111; padding: 0 0.5em; margin: -1.25em 0 0.15em -0.2em; font-size: 0.55em; box-shadow: 2px 2px 0 rgba(0, 0, 0, 0.25); }',
      "#box::after { content: ''; position: absolute; right: -20px; bottom: 6px; width: 24px; height: 30px; background: #111111; clip-path: polygon(0 10%, 100% 0, 55% 45%, 100% 55%, 10% 100%, 35% 55%, 0 60%); }",
    ].join('\n'),
  },
  // オーロラ(おすすめ): 夜空色のすりガラスに、ゆっくり色が流れるオーロラの縁。ぼかしからふわっと浮かぶ
  aurora: {
    fontFamily: 'M PLUS Rounded 1c', bold: true, color: '#f2f5ff', outlineWidth: 0, outlineColor: '#000000',
    bgColor: '#0d1230', bgOpacity: 78, padding: 12, radius: 18, appear: 'aurora-rise', translationColor: '#9ad8ff',
    customCss: [
      '#box { border: 2px solid transparent; background: linear-gradient(rgba(13, 18, 48, 0.8), rgba(13, 18, 48, 0.8)) padding-box, linear-gradient(90deg, #7ce7ff, #a68cff, #ff8fd8, #7ce7ff) border-box !important; background-size: 100% 100%, 300% 100% !important; box-shadow: 0 6px 24px rgba(80, 120, 255, 0.3); text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5) !important; }',
      '#box.visible, #box.appear-aurora-rise.visible { animation: aurora-rise 900ms cubic-bezier(.2, .8, .2, 1), aurora-flow 8s linear infinite; }',
      '.res-head { background: linear-gradient(90deg, #7ce7ff, #c79bff); -webkit-background-clip: text; background-clip: text; color: transparent; }',
      '@keyframes aurora-flow { from { background-position: 0 0, 0% 0; } to { background-position: 0 0, 300% 0; } }',
    ].join('\n'),
  },
  // 病みかわ: パステルピンクに黒い太枠と黒い影、内側にピンクの縫い目、角にばんそうこう。
  // ぽんっと出て、ふるふるっと震えたあと、下の縁から黒いしずくがとろっと垂れる
  yami: {
    fontFamily: 'Hachi Maru Pop', bold: false, color: '#3a1f3d', outlineWidth: 0, outlineColor: '#ffffff',
    bgColor: '#ffdbe9', bgOpacity: 97, padding: 12, radius: 18, appear: 'yami-pop', translationColor: '#9b6bd6',
    customCss: [
      '#box { margin: 0.5em 0.6em 1em 0.2em; border: 3px solid #3a1f3d; box-shadow: 5px 5px 0 #3a1f3d; outline: 2px dashed #ff9cc2; outline-offset: -8px; }',
      ".res-head { color: #e0457b; }",
      ".res-head::before { content: '✚ '; color: #ff8fb8; }",
      "#box::before { content: ''; position: absolute; right: -0.55em; top: -0.3em; width: 1.9em; height: 0.62em; transform: rotate(30deg); background: linear-gradient(90deg, #ffc9d6 30%, transparent 30%, transparent 70%, #ffc9d6 70%), radial-gradient(circle, #e9a0b4 28%, transparent 32%) center / 0.2em 0.2em, #fff0f4; border: 1px solid #e9a0b4; border-radius: 0.3em; }",
      // しずく(1つの形を box-shadow で増やす。長さは影の位置でずらす)
      "#box::after { content: ''; position: absolute; left: 16%; top: 100%; width: 0.34em; height: 0.8em; margin-top: -2px; background: #3a1f3d; border-radius: 0 0 0.17em 0.17em / 0 0 0.3em 0.3em; box-shadow: 2.2em -0.32em 0 #3a1f3d, 5.6em -0.14em 0 -0.04em #3a1f3d; transform-origin: top; }",
      '#box.visible::after { animation: yami-drip 1400ms ease-in 500ms both; }',
      '@keyframes yami-drip { from { transform: scaleY(0); } to { transform: scaleY(1); } }',
    ].join('\n'),
  },
  // ゆめかわ: パステルピンクの丸い吹き出し、ハートときらきらがふわふわ、ぷるんと弾んで出る
  cute: {
    fontFamily: 'Zen Maru Gothic', bold: true, color: '#7a3b69', outlineWidth: 0, outlineColor: '#ffffff',
    bgColor: '#ffe9f4', bgOpacity: 96, padding: 10, radius: 22, appear: 'cute-bounce', translationColor: '#b26ad9',
    customCss: [
      '#box { margin: 0.6em 0.5em 0.3em 0.6em; border: 3px solid #ffb6d9; box-shadow: 0 4px 0 #ffc9e3, 0 6px 14px rgba(255, 105, 180, 0.35); }',
      ".res-head { color: #ff6fb1; }",
      "#box::before { content: '♡'; position: absolute; left: -0.55em; top: -0.6em; color: #ff8cc6; font-size: 0.9em; animation: cute-float 1.6s ease-in-out infinite; }",
      "#box::after { content: '✦'; position: absolute; right: -0.35em; top: -0.55em; color: #b28dff; font-size: 0.7em; animation: cute-float 1.6s ease-in-out 0.4s infinite; }",
      '@keyframes cute-float { 0%, 100% { transform: translateY(0); } 50% { transform: translateY(-5px); } }',
    ].join('\n'),
  },
};

// ---- プリセットの切り替え・手直しの記憶・初期値に戻す(コメント一覧・コメント字幕) ----
// 手直しした内容は settings.style.presets.<list|subtitle>.custom.<プリセット> に覚えておく
const PRESET_GROUPS = {
  list: { presets: LIST_PRESETS, keys: C.LIST_PRESET_KEYS, part: 'list', select: 'listPreset' },
  subtitle: { presets: SUBTITLE_PRESETS, keys: C.SUBTITLE_PRESET_KEYS, part: 'subtitle', select: 'subtitlePreset' },
};

function presetState(group) {
  if (!settings.style.presets || typeof settings.style.presets !== 'object') settings.style.presets = {};
  const all = settings.style.presets;
  if (!all[group] || typeof all[group] !== 'object') {
    // この仕組みより前に適用したプリセットは、カスタムCSSが同じものを今のプリセットとみなす
    const g = PRESET_GROUPS[group];
    const css = String((settings.style[g.part] || {}).customCss || '');
    const found = Object.keys(g.presets).find((id) => id !== 'default' && css && (g.presets[id].customCss || '') === css);
    all[group] = { current: found || 'default', custom: {} };
  }
  if (!all[group].custom || typeof all[group].custom !== 'object') all[group].custom = {};
  return all[group];
}

/** プリセットの元の値(手直しなし)。プリセットに無い項目は既定値 */
function presetDefaults(group, id) {
  const g = PRESET_GROUPS[group];
  const preset = g.presets[id] || {};
  const defaults = C.DEFAULT_STYLE[g.part];
  const out = {};
  for (const k of g.keys) out[k] = k in preset ? preset[k] : defaults[k];
  return out;
}

function applyPreset(group, id, useCustom) {
  const g = PRESET_GROUPS[group];
  const st = presetState(group);
  if (!settings.style[g.part]) settings.style[g.part] = {};
  const values = { ...presetDefaults(group, id), ...(useCustom ? st.custom[id] || {} : {}) };
  for (const k of g.keys) settings.style[g.part][k] = values[k];
  st.current = id;
  renderFields();
  scheduleSave();
  if (group === 'subtitle') renderPreview(true);
}

/** 手直しした時: 今のプリセットとの違いを覚えておく(同じ値に戻した項目は忘れる) */
function rememberPresetEdit(key) {
  for (const [group, g] of Object.entries(PRESET_GROUPS)) {
    const prefix = `style.${g.part}.`;
    if (!key.startsWith(prefix) || !g.keys.includes(key.slice(prefix.length))) continue;
    const st = presetState(group);
    const base = presetDefaults(group, st.current);
    const diff = {};
    for (const k of g.keys) {
      const v = settings.style[g.part] ? settings.style[g.part][k] : undefined;
      if (v !== undefined && v !== base[k]) diff[k] = v;
    }
    if (Object.keys(diff).length) st.custom[st.current] = diff;
    else delete st.custom[st.current];
  }
}

function renderPresetSelects() {
  for (const [group, g] of Object.entries(PRESET_GROUPS)) {
    const value = presetState(group).current in g.presets ? presetState(group).current : 'default';
    for (const sel of document.querySelectorAll(`[data-preset-group="${group}"]`)) sel.value = value;
  }
}

// 「デスクトップに表示」タブのプリセットの欄は、元のタブの欄と同じ選択肢にする
for (const sel of document.querySelectorAll('[data-preset-mirror]')) {
  sel.innerHTML = document.getElementById(sel.dataset.presetMirror).innerHTML;
}
for (const sel of document.querySelectorAll('[data-preset-group]')) {
  sel.addEventListener('change', () => applyPreset(sel.dataset.presetGroup, sel.value, true));
}
for (const btn of document.querySelectorAll('[data-preset-reset]')) {
  btn.addEventListener('click', () => {
    const group = btn.dataset.presetReset;
    const st = presetState(group);
    delete st.custom[st.current];
    applyPreset(group, st.current, false);
  });
}

// ---- レス着信音 ----
const chimeMsgEl = document.getElementById('chimeMsg');
document.getElementById('chimePickBtn').addEventListener('click', async () => {
  const path = await invoke('comments_pick_sound').catch(() => null);
  if (!path) return;
  settings.chimePath = path;
  renderFields();
  scheduleSave();
});
document.getElementById('chimeTestBtn').addEventListener('click', async () => {
  chimeMsgEl.style.color = '';
  chimeMsgEl.textContent = '';
  try {
    await invoke('comments_test_chime', { settings });
  } catch (e) {
    chimeMsgEl.style.color = 'var(--c-ff8a8a)';
    chimeMsgEl.textContent = String(e);
  }
});

// ---- 見た目の共有(共有コード・ファイル) ----
// 中身は {app:'ReaCast', kind:'preset', version:1, group, base, values}。
// baseは元にしたプリセット、valuesはプリセットで置き換える項目(PRESET_KEYS)の今の値。
// 共有コードはこのJSONをUTF-8のbase64にして、先頭に PRESET_CODE_PREFIX を付けたもの。
const PRESET_CODE_PREFIX = 'ReaCastPreset1:';
const PRESET_GROUP_NAMES = { subtitle: 'コメント字幕', list: 'コメント一覧' };

function presetShareData(group) {
  const g = PRESET_GROUPS[group];
  const cur = C.mergeStyle(settings.style)[g.part];
  const values = {};
  for (const k of g.keys) values[k] = cur[k];
  return { app: 'ReaCast', kind: 'preset', version: 1, group, base: presetState(group).current, values };
}

function toBase64(text) {
  const bytes = new TextEncoder().encode(text);
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function fromBase64(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** 共有コード(またはファイルの中身のJSON)を確かめて、読み込める形にする。
 * 知らない項目・型の違う値・長すぎる値は捨てる(壊れた共有コードで画面がおかしくならないように) */
function parsePresetShare(text) {
  let raw = String(text || '').trim();
  if (!raw) throw new Error('共有コードを貼り付けてください');
  let obj;
  try {
    if (raw.startsWith(PRESET_CODE_PREFIX)) raw = fromBase64(raw.slice(PRESET_CODE_PREFIX.length).replace(/\s+/g, ''));
    obj = JSON.parse(raw);
  } catch {
    throw new Error('共有コードの形式が正しくありません(途中で切れていないか確かめてください)');
  }
  if (!obj || obj.app !== 'ReaCast' || obj.kind !== 'preset' || !PRESET_GROUPS[obj.group] || !obj.values || typeof obj.values !== 'object') {
    throw new Error('ReaCastの見た目の共有コードではありません');
  }
  const g = PRESET_GROUPS[obj.group];
  const defaults = C.DEFAULT_STYLE[g.part];
  const values = {};
  for (const k of g.keys) {
    const v = obj.values[k];
    if (v === undefined || typeof v !== typeof defaults[k]) continue;
    if (typeof v === 'string' && v.length > (k === 'customCss' ? 30000 : 200)) continue;
    if (typeof v === 'number' && !Number.isFinite(v)) continue;
    values[k] = v;
  }
  const base = Object.prototype.hasOwnProperty.call(g.presets, obj.base) ? obj.base : 'default';
  return { group: obj.group, base, values };
}

/** 読み込んだ見た目を使う。元にしたプリセットを選んで、違う所を「手直し」として覚える
 * (「このプリセットの初期値に戻す」も今まで通り使える) */
function applyPresetShare(data) {
  const st = presetState(data.group);
  const base = presetDefaults(data.group, data.base);
  const diff = {};
  for (const [k, v] of Object.entries(data.values)) if (v !== base[k]) diff[k] = v;
  if (Object.keys(diff).length) st.custom[data.base] = diff;
  else delete st.custom[data.base];
  applyPreset(data.group, data.base, true);
}

function presetMsg(group, text) {
  const el = document.querySelector(`[data-preset-msg="${group}"]`);
  if (!el) return;
  el.textContent = text;
  clearTimeout(el._timer);
  el._timer = setTimeout(() => (el.textContent = ''), 6000);
}

/** プリセットの名前(選択肢の文字の、括弧の前まで) */
function presetLabel(group, id) {
  const sel = document.getElementById(PRESET_GROUPS[group].select);
  const o = sel && Array.from(sel.options).find((x) => x.value === id);
  return o ? o.textContent.replace(/\(.*$/, '') : id;
}

for (const btn of document.querySelectorAll('[data-preset-share]')) {
  btn.addEventListener('click', async () => {
    const group = btn.dataset.presetShare;
    const code = PRESET_CODE_PREFIX + toBase64(JSON.stringify(presetShareData(group)));
    try {
      await navigator.clipboard.writeText(code);
      presetMsg(group, '共有コードをコピーしました。そのまま貼り付けて渡せます');
    } catch {
      presetMsg(group, 'コピーできませんでした。「ファイルに書き出す」を使ってください');
    }
  });
}
for (const btn of document.querySelectorAll('[data-preset-export]')) {
  btn.addEventListener('click', async () => {
    const group = btn.dataset.presetExport;
    const data = presetShareData(group);
    const name = `ReaCastの見た目(${PRESET_GROUP_NAMES[group]}・${presetLabel(group, data.base)}).json`;
    try {
      const path = await invoke('comments_save_preset_file', { fileName: name, text: JSON.stringify(data, null, 2) });
      if (path) presetMsg(group, `書き出しました: ${path}`);
    } catch (e) {
      presetMsg(group, String(e));
    }
  });
}

const presetImportDialog = document.getElementById('presetImportDialog');
const presetImportText = document.getElementById('presetImportText');
const presetImportInfo = document.getElementById('presetImportInfo');
let presetImportGroup = 'subtitle';

/** 貼り付けた内容を確かめて、何が読み込まれるかを窓の下に出す */
function previewPresetImport() {
  presetImportInfo.style.color = '';
  if (!presetImportText.value.trim()) {
    presetImportInfo.textContent = '';
    return null;
  }
  try {
    const data = parsePresetShare(presetImportText.value);
    const name = presetLabel(data.group, data.base);
    const notes = [`${PRESET_GROUP_NAMES[data.group]}の見た目(元のプリセット: ${name})`];
    if (data.group !== presetImportGroup) notes.push(`${PRESET_GROUP_NAMES[data.group]}の共有コードなので、${PRESET_GROUP_NAMES[data.group]}に読み込みます`);
    notes.push(`今の「${name}」の手直しは、読み込んだ内容に置き換わります`);
    // カスタムCSSが外のサーバーの画像・フォントを読み込むと、OBSで開いている間そのサーバーに
    // こちらのIPアドレスが伝わる。知らない人の共有コードの時に気づけるようにする
    if (/url\s*\(\s*['"]?\s*(https?:)?\/\/|@import/i.test(data.values.customCss || '')) {
      notes.push('※ カスタムCSSが外のサイトの画像・フォントを読み込みます(信頼できる人の共有コードか確かめてください)');
    }
    presetImportInfo.textContent = notes.join('\n');
    return data;
  } catch (e) {
    presetImportInfo.textContent = e.message;
    presetImportInfo.style.color = 'var(--c-ff8a8a)';
    return null;
  }
}

for (const btn of document.querySelectorAll('[data-preset-import]')) {
  btn.addEventListener('click', () => {
    presetImportGroup = btn.dataset.presetImport;
    presetImportText.value = '';
    presetImportInfo.textContent = '';
    presetImportDialog.classList.add('visible');
    presetImportText.focus();
  });
}
presetImportText.addEventListener('input', previewPresetImport);
document.getElementById('presetImportFile').addEventListener('click', async () => {
  try {
    const text = await invoke('comments_open_preset_file');
    if (text != null) {
      presetImportText.value = text;
      previewPresetImport();
    }
  } catch (e) {
    presetImportInfo.textContent = String(e);
    presetImportInfo.style.color = 'var(--c-ff8a8a)';
  }
});
document.getElementById('presetImportCancel').addEventListener('click', () => presetImportDialog.classList.remove('visible'));
document.getElementById('presetImportOk').addEventListener('click', () => {
  const data = previewPresetImport();
  if (!data) {
    if (!presetImportText.value.trim()) presetImportInfo.textContent = '共有コードを貼り付けるか、ファイルを選んでください';
    return;
  }
  applyPresetShare(data);
  presetImportDialog.classList.remove('visible');
  presetMsg(data.group, '読み込みました');
});

// ---- 設定の共有コード(全部・タブごと) ----
// 中身は {app:'ReaCast', kind:'settings', version:1, scope:'all'|タブ, values:{設定の場所: 値}}。
// 共有コードはこのJSONをUTF-8のbase64にして、先頭に SETTINGS_CODE_PREFIX を付けたもの。
// URL・名前・PCごとの場所(ソフト・音・アイコンのフォルダ、マイク、出力先)は入れない
const SETTINGS_CODE_PREFIX = 'ReaCastSettings1:';
const SHARE_EXCLUDE = new Set([
  'threadUrl', 'subThreadUrl', 'youtubeTarget', 'twitchChannel', 'postName', 'postMail', 'createName', 'createMail',
  'commandPath', 'chimePath', 'iconFolderBbs', 'iconFolderYoutube', 'iconFolderTwitch', 'ttsDevice', 'ttsExePath',
  'ttsExePaths', 'voiceMic', 'peercastPort', 'desktopMonitorId', 'desktopRect', 'desktopListRect',
]);
const SHARE_SKIP_TABS = ['easy', 'thread', 'misc', 'addons'];

/** そのタブで共有する設定の場所の一覧。見た目のタブは見た目を丸ごと(プリセットの手直しも) */
function tabShareKeys(section) {
  const tab = section.dataset.tab;
  const keys = new Set();
  for (const el of section.querySelectorAll('[data-key], [data-hotkey]')) {
    if (el.closest('section') !== section) continue;
    const k = el.dataset.key || el.dataset.hotkey;
    keys.add(k.startsWith('ttsEngineParams.') ? 'ttsEngineParams' : k);
  }
  for (const k of TAB_RESET_EXTRA[tab] || []) keys.add(k);
  const part = TAB_RESET_STYLE[tab];
  if (part) {
    for (const k of Array.from(keys)) if (k.startsWith(`style.${part}.`)) keys.delete(k);
    keys.add(`style.${part}`);
    if (PRESET_GROUPS[part]) keys.add(`style.presets.${part}`);
  }
  for (const k of SHARE_EXCLUDE) keys.delete(k);
  return keys;
}

function shareSections() {
  return Array.from(document.querySelectorAll('main > section[data-tab]')).filter((s) => !SHARE_SKIP_TABS.includes(s.dataset.tab));
}

function shareKeys(scope) {
  if (scope !== 'all') {
    const section = document.querySelector(`main > section[data-tab="${scope}"]`);
    return section && !SHARE_SKIP_TABS.includes(scope) ? tabShareKeys(section) : new Set();
  }
  const keys = new Set();
  for (const s of shareSections()) for (const k of tabShareKeys(s)) keys.add(k);
  // 見た目は全部(「デスクトップに表示」タブにある見た目の欄も含めて)
  for (const k of Array.from(keys)) if (k.startsWith('style.')) keys.delete(k);
  keys.add('style');
  return keys;
}

function tabName(tab) {
  if (tab === 'all') return '全部の設定';
  const b = document.querySelector(`#tabs button[data-tab="${tab}"]`);
  return `「${b ? b.textContent.trim() : tab}」タブ`;
}

function settingsShareCode(scope) {
  const values = {};
  for (const k of shareKeys(scope)) {
    const v = getPath(settings, k);
    if (v !== undefined) values[k] = v;
  }
  // まだ一度も変えていない見た目の項目も同じになるように、既定値で埋めて渡す
  if (values.style) values.style = { ...C.mergeStyle(values.style), presets: values.style.presets };
  for (const [k, v] of Object.entries(values)) {
    const m = /^style\.(\w+)$/.exec(k);
    if (m && C.DEFAULT_STYLE[m[1]]) values[k] = C.mergeStyle({ [m[1]]: v })[m[1]];
  }
  return SETTINGS_CODE_PREFIX + toBase64(JSON.stringify({ app: 'ReaCast', kind: 'settings', version: 1, scope, values }));
}

/** 型が今の設定と同じ値だけを受け取る(壊れた共有コードで画面がおかしくならないように) */
function sameShape(v, ref) {
  if (ref === undefined || ref === null) {
    return v === null || ['string', 'number', 'boolean'].includes(typeof v) || (typeof v === 'object' && JSON.stringify(v).length < 200000);
  }
  if (Array.isArray(ref)) return Array.isArray(v);
  if (typeof v !== typeof ref) return false;
  if (typeof v === 'string') return v.length <= 50000;
  if (typeof v === 'number') return Number.isFinite(v);
  return true;
}

function cleanStylePart(part, v) {
  const defaults = C.DEFAULT_STYLE[part];
  const out = {};
  if (!v || typeof v !== 'object') return out;
  for (const [k, x] of Object.entries(v)) if (k in defaults && sameShape(x, defaults[k])) out[k] = x;
  return out;
}

/** 共有コードを確かめて {scope, values} にする */
function parseSettingsShare(text) {
  let raw = String(text || '').trim();
  if (!raw) throw new Error('共有コードを貼り付けてください');
  let obj;
  try {
    if (raw.startsWith(SETTINGS_CODE_PREFIX)) raw = fromBase64(raw.slice(SETTINGS_CODE_PREFIX.length).replace(/\s+/g, ''));
    obj = JSON.parse(raw);
  } catch {
    throw new Error('共有コードの形式が正しくありません(途中で切れていないか確かめてください)');
  }
  if (!obj || obj.app !== 'ReaCast' || obj.kind !== 'settings' || typeof obj.scope !== 'string' || !obj.values || typeof obj.values !== 'object') {
    throw new Error('ReaCastの設定の共有コードではありません');
  }
  const allowed = shareKeys(obj.scope);
  if (!allowed.size) throw new Error('このバージョンのReaCastには無いタブの共有コードです');
  const values = {};
  for (const [k, v] of Object.entries(obj.values)) {
    if (!allowed.has(k)) continue;
    if (k === 'style') {
      if (!v || typeof v !== 'object') continue;
      const style = {};
      for (const part of Object.keys(C.DEFAULT_STYLE)) style[part] = cleanStylePart(part, v[part]);
      if (v.presets && typeof v.presets === 'object' && JSON.stringify(v.presets).length < 200000) style.presets = v.presets;
      values.style = style;
      continue;
    }
    const m = /^style\.(\w+)$/.exec(k);
    if (m && C.DEFAULT_STYLE[m[1]]) {
      values[k] = cleanStylePart(m[1], v);
      continue;
    }
    if (k.startsWith('style.presets.')) {
      if (v && typeof v === 'object' && JSON.stringify(v).length < 200000) values[k] = v;
      continue;
    }
    if (sameShape(v, getPath(settings, k))) values[k] = v;
  }
  return { scope: obj.scope, values };
}

function applySettingsShare(data) {
  const copy = (x) => JSON.parse(JSON.stringify(x));
  for (const [k, v] of Object.entries(data.values)) {
    if (k === 'style') {
      for (const [part, x] of Object.entries(v)) {
        if (part === 'presets') settings.style.presets = copy(x);
        else settings.style[part] = { ...C.DEFAULT_STYLE[part], ...copy(x) };
      }
    } else if (/^style\.\w+$/.test(k) && C.DEFAULT_STYLE[k.slice(6)]) {
      settings.style[k.slice(6)] = { ...C.DEFAULT_STYLE[k.slice(6)], ...copy(v) };
    } else {
      setPath(settings, k, copy(v));
    }
  }
  // 見た目のプリセットの選択肢も、読み込んだものに合わせる
  for (const group of Object.keys(PRESET_GROUPS)) {
    const st = settings.style.presets && settings.style.presets[group];
    for (const sel of document.querySelectorAll(`[data-preset-group="${group}"]`)) if (st && st.current) sel.value = st.current;
  }
  renderFields();
  scheduleSave();
  if (Object.keys(data.values).some((k) => k.startsWith('hotkey'))) setTimeout(() => applyHotkeys(), 400);
}

async function copySettingsShare(scope, msgEl) {
  try {
    await navigator.clipboard.writeText(settingsShareCode(scope));
    msgEl.textContent = `${tabName(scope)}の共有コードをコピーしました(URL・名前・PCの中の場所は入っていません)`;
  } catch {
    msgEl.textContent = 'コピーできませんでした';
  }
}

const settingsImportDialog = document.getElementById('settingsImportDialog');
const settingsImportText = document.getElementById('settingsImportText');
const settingsImportInfo = document.getElementById('settingsImportInfo');
let settingsImportMsgEl = null;

function previewSettingsImport() {
  settingsImportInfo.style.color = '';
  const text = settingsImportText.value.trim();
  if (!text) {
    settingsImportInfo.textContent = '';
    return null;
  }
  try {
    // 見た目の共有コード(プリセットの横のボタンで作ったもの)も、ここで読み込めるようにする
    if (text.startsWith(PRESET_CODE_PREFIX)) {
      const p = parsePresetShare(text);
      settingsImportInfo.textContent = `${PRESET_GROUP_NAMES[p.group]}の見た目の共有コードです(元のプリセット: ${presetLabel(p.group, p.base)})`;
      return { preset: p };
    }
    const data = parseSettingsShare(text);
    const notes = [`${tabName(data.scope)}の共有コードです(${Object.keys(data.values).length}項目)`];
    notes.push(
      data.scope === 'all'
        ? '今の設定が、読み込んだ設定に置き換わります(URL・名前・PCの中の場所はそのまま)'
        : `${tabName(data.scope)}の設定が、読み込んだ設定に置き換わります(ほかのタブはそのまま)`,
    );
    const json = JSON.stringify(data.values);
    // カスタムCSSが外のサーバーの画像・フォントを読み込むと、OBSで開いている間そのサーバーに
    // こちらのIPアドレスが伝わる。知らない人の共有コードの時に気づけるようにする
    if (/url\s*\(\s*['"]?\s*(https?:)?\/\/|@import/i.test(json)) notes.push('※ カスタムCSSが外のサイトの画像・フォントを読み込みます(信頼できる人の共有コードか確かめてください)');
    if ('commandArgs' in data.values || 'ttsUrl' in data.values) notes.push('※ 読み上げソフトに渡す引数・URLも置き換わります');
    settingsImportInfo.textContent = notes.join('\n');
    return data;
  } catch (e) {
    settingsImportInfo.textContent = e.message;
    settingsImportInfo.style.color = 'var(--c-ff8a8a)';
    return null;
  }
}

function openSettingsImport(msgEl) {
  settingsImportMsgEl = msgEl;
  settingsImportText.value = '';
  settingsImportInfo.textContent = '';
  settingsImportDialog.classList.add('visible');
  settingsImportText.focus();
}
settingsImportText.addEventListener('input', previewSettingsImport);
document.getElementById('settingsImportCancel').addEventListener('click', () => settingsImportDialog.classList.remove('visible'));
document.getElementById('settingsImportOk').addEventListener('click', () => {
  const data = previewSettingsImport();
  if (!data) {
    if (!settingsImportText.value.trim()) settingsImportInfo.textContent = '共有コードを貼り付けてください';
    return;
  }
  if (data.preset) applyPresetShare(data.preset);
  else applySettingsShare(data);
  settingsImportDialog.classList.remove('visible');
  if (settingsImportMsgEl) settingsImportMsgEl.textContent = '読み込みました';
});

function shareButtons(scope, msgEl) {
  const copy = document.createElement('button');
  copy.className = 'secondary';
  copy.textContent = scope === 'all' ? '全部の設定の共有コードをコピー' : 'このタブの共有コードをコピー';
  copy.addEventListener('click', () => copySettingsShare(scope, msgEl));
  const load = document.createElement('button');
  load.className = 'secondary';
  load.textContent = '共有コードを読み込む';
  load.addEventListener('click', () => openSettingsImport(msgEl));
  return [copy, load];
}

for (const section of shareSections()) {
  const row = section.querySelector(':scope > .tab-reset');
  if (!row) continue;
  const share = document.createElement('div');
  share.className = 'inline tab-share';
  const msg = document.createElement('span');
  msg.className = 'unit';
  share.append(...shareButtons(section.dataset.tab, msg), msg);
  row.before(share);
}
{
  const msg = document.getElementById('shareAllMsg');
  document.getElementById('shareAllButtons').prepend(...shareButtons('all', msg));
}

// ---- デスクトップ字幕・デスクトップのレス一覧の位置調整 ----
// target: 'subtitle'(コメント字幕) / 'list'(コメント一覧)
const adjusting = { subtitle: false, list: false };
function renderAdjust() {
  for (const btn of document.querySelectorAll('[data-desktop-adjust]')) {
    btn.textContent = adjusting[btn.dataset.desktopAdjust] ? '位置の調整を終了(保存)' : 'マウスで位置を調整する';
  }
}
for (const btn of document.querySelectorAll('[data-desktop-adjust]')) {
  const target = btn.dataset.desktopAdjust;
  btn.addEventListener('click', async () => {
    const enabled = target === 'list' ? settings.desktopListEnabled : settings.desktopEnabled;
    if (!enabled && !adjusting[target]) {
      showStatusError(`「${target === 'list' ? 'コメント一覧' : 'コメント字幕'}」の「デスクトップに表示する」をONにしてください`);
      return;
    }
    try {
      await invoke('comments_desktop_adjust', { on: !adjusting[target], target });
    } catch (e) {
      showStatusError(String(e));
    }
  });
}
for (const btn of document.querySelectorAll('[data-desktop-reset]')) {
  btn.addEventListener('click', () => {
    invoke('comments_desktop_reset', { target: btn.dataset.desktopReset }).catch((e) => showStatusError(String(e)));
  });
}

// ---- OBSでの設定方法(GitHubの説明ページをいつものブラウザで開く) ----
for (const btn of document.querySelectorAll('[data-open-guide]')) {
  btn.addEventListener('click', () => invoke('comments_open_obs_guide').catch((e) => showStatusError(String(e))));
}

// ---- かんたん設定の「リアクションを表示する」(コントロールパネルの「リアクション機能を使う」と同じ) ----
const easyReactions = document.getElementById('easyReactionsEnabled');
invoke('cfg_get', { key: 'reactionsEnabled' }).then((v) => {
  easyReactions.checked = v !== false;
});
easyReactions.addEventListener('change', () => {
  invoke('set_reactions_enabled', { enabled: easyReactions.checked }).catch((e) => showStatusError(String(e)));
});
listen('reactions:enabled', (e) => {
  easyReactions.checked = !!e.payload;
});

// ---- 次スレの>>1のテンプレート編集(右側に建てた時の見え方をすぐ出す) ----
const templateEl = document.querySelector('[data-key="createBodyTemplate"]');
let draftTimer = null;

/** 入力途中の設定でタイトルと>>1を作って表示する(実際には建てない)。 */
async function renderDraft() {
  const text = templateEl.value;
  // 掲示板の本文はShift_JIS(したらばはEUC-JP)で数えるので、全角は2バイトとして数える
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
  // シンプルモードでは上の状態表示が隠れているので、そちらにも出す
  const simple = document.getElementById('simpleStatus');
  simple.className = 'simple-status ng';
  simple.textContent = text;
  statusEl.textContent = '';
  const span = document.createElement('span');
  span.className = 'ng';
  span.textContent = text;
  statusEl.appendChild(span);
}

function renderStatus(s) {
  renderSimpleStatus(s);
  renderSourceNotes(s);
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
  const sources = s.sources || [];
  if (sources.length) {
    const chips = document.createElement('div');
    chips.className = 'src-chips';
    for (const src of sources) {
      const chip = document.createElement('span');
      chip.className = 'src-chip';
      // 詳しい説明はマウスを乗せると出す(札には括弧の前までの短い説明)
      chip.title = `${src.label}: ${src.message}`;
      const dot = document.createElement('i');
      dot.className = src.state === 'ok' ? 'ok' : src.state === 'error' ? 'ng' : '';
      const short = String(src.message || '').split(/[(（]/)[0];
      chip.append(dot, `${src.label}: ${short}`);
      chips.appendChild(chip);
    }
    statusEl.appendChild(chips);
  }
}

// ---- 自分の声の字幕(音声認識はEdgeの窓で行う。src-tauri/src/voice.rs) ----
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
    renderVoiceDevices(v && v.devices);
  })
  .catch(() => {});
listen('voice:status', (e) => renderVoiceStatus(e.payload));
// ホットキーで切り替えた時もボタンの表示を合わせる
listen('voice:muted', (e) => renderVoiceMuted(e.payload));

// 声の大きさのメーター・マイクの一覧(音声認識の窓から届く)
const voiceMeterBar = document.getElementById('voiceMeterBar');
let voiceMeterTimer = null;
listen('voice:level', (e) => {
  const level = Math.max(0, Math.min(1, Number(e.payload) || 0));
  voiceMeterBar.style.width = `${Math.round(level * 100)}%`;
  voiceMeterBar.style.background = level > 0.9 ? 'var(--c-ff8a8a)' : 'var(--c-7dff9a)';
  // 届かなくなったら(マイクを止めた・窓を閉じた)0に戻す
  clearTimeout(voiceMeterTimer);
  voiceMeterTimer = setTimeout(() => (voiceMeterBar.style.width = '0'), 1500);
});
const voiceMicSelect = document.getElementById('voiceMicSelect');
function renderVoiceDevices(list) {
  if (!Array.isArray(list)) return;
  const current = settings ? settings.voiceMic || '' : voiceMicSelect.value;
  for (const o of Array.from(voiceMicSelect.options)) if (o.value) o.remove();
  for (const name of list) {
    const o = document.createElement('option');
    o.value = name;
    o.textContent = name;
    voiceMicSelect.appendChild(o);
  }
  writeField(voiceMicSelect, current);
}
listen('voice:devices', (e) => renderVoiceDevices(e.payload));

// ---- 新しい絵文字のフォント(使いたい人だけダウンロード。src-tauri/src/emoji_font.rs) ----
const emojiFontStatusEl = document.getElementById('emojiFontStatus');
const emojiFontBtn = document.getElementById('emojiFontBtn');
const emojiFontAllBox = document.getElementById('emojiFontAll');
let emojiFontInstalled = false;
function renderEmojiFont(status) {
  const installed = !!(status && status.installed);
  emojiFontInstalled = installed;
  emojiFontAllBox.checked = !!(status && status.all);
  emojiFontAllBox.disabled = !installed;
  document.getElementById('emojiFontAllNote').textContent = installed ? 'ONにすると、Win10・Win11・OBSで絵文字の絵がそろいます' : '先に上のダウンロードをしてください';
  emojiFontStatusEl.textContent = installed ? 'ダウンロード済み(Windowsに無い絵文字も表示します)' : 'ダウンロードしていません';
  emojiFontBtn.textContent = installed ? '削除する' : 'ダウンロードする(約3MB)';
  emojiFontBtn.disabled = false;
  // ダウンロードしてONにするまでは「おすすめ」を出す
  for (const el of document.querySelectorAll('[data-emoji-recommend]')) el.classList.toggle('hidden', installed && !!(status && status.all));
}
// 「Googleの絵文字にする」: ダウンロードしていなければダウンロードしてから、すべての絵文字をGoogleの絵文字にする
for (const btn of document.querySelectorAll('[data-emoji-recommend-btn]')) {
  btn.addEventListener('click', async () => {
    const buttons = document.querySelectorAll('[data-emoji-recommend-btn]');
    for (const b of buttons) { b.disabled = true; b.textContent = 'ダウンロードしています…'; }
    try {
      if (!emojiFontInstalled) await invoke('emoji_font_download');
      await invoke('emoji_font_set_all', { all: true });
    } catch (err) {
      showStatusError(String((err && err.message) || err));
    }
    for (const b of buttons) { b.disabled = false; b.textContent = 'Googleの絵文字にする'; }
    renderEmojiFont(await invoke('emoji_font_status').catch(() => null));
  });
}
invoke('emoji_font_status').then(renderEmojiFont).catch(() => {});
emojiFontAllBox.addEventListener('change', () => {
  invoke('emoji_font_set_all', { all: emojiFontAllBox.checked }).catch((e) => showStatusError(String(e)));
});
listen('emoji-font:changed', (e) => renderEmojiFont(e.payload));
emojiFontBtn.addEventListener('click', async () => {
  emojiFontBtn.disabled = true;
  emojiFontStatusEl.textContent = emojiFontInstalled ? '削除しています…' : 'ダウンロードしています…';
  try {
    await invoke(emojiFontInstalled ? 'emoji_font_remove' : 'emoji_font_download');
  } catch (err) {
    emojiFontStatusEl.textContent = String((err && err.message) || err);
    emojiFontBtn.disabled = false;
    return;
  }
  renderEmojiFont(await invoke('emoji_font_status').catch(() => null));
});

// ---- PCの中で翻訳(Chrome/Edgeの翻訳の窓。src-tauri/src/local_translate.rs) ----
function renderTranslateStatus(v) {
  if (!v || !v.state) return;
  for (const el of document.querySelectorAll('.translate-status')) {
    el.textContent = v.message || '';
    el.style.color =
      v.state === 'ready' ? 'var(--c-7dff9a)' : v.state === 'error' || v.state === 'unsupported' ? 'var(--c-ff8a8a)' : v.state === 'needs-click' ? 'var(--c-ffd36b)' : '';
  }
  for (const btn of document.querySelectorAll('.translate-show-btn')) btn.hidden = v.state === 'off' || v.state === 'error';
}
invoke('translate_status').then(renderTranslateStatus).catch(() => {});
listen('translate:status', (e) => renderTranslateStatus(e.payload));
for (const btn of document.querySelectorAll('.translate-show-btn')) {
  btn.addEventListener('click', () => invoke('translate_show_window').catch(() => {}));
}
document.getElementById('voiceReopenBtn').addEventListener('click', async () => {
  // 入力途中の設定(言語など)も反映してから開き直す
  clearTimeout(saveTimer);
  await invoke('comments_save_settings', { settings }).catch(() => {});
  invoke('voice_reopen').catch(() => {});
});

// Edgeの音声認識の使用時間(Edgeが記録している分を読むだけ。「自分の声の字幕」タブを開いた時と、開いている間は30秒おき)
const voiceEdgeUsageEl = document.getElementById('voiceEdgeUsage');
function hm(secs) {
  const m = Math.max(0, Math.round(secs / 60));
  return m >= 60 ? `${Math.floor(m / 60)}時間${m % 60}分` : `${m}分`;
}
async function renderEdgeUsage() {
  const u = await invoke('voice_edge_usage').catch(() => null);
  if (!u) {
    voiceEdgeUsageEl.textContent = 'まだ記録がありません(ReaCastからEdgeで聞き取ると記録されます)';
    voiceEdgeUsageEl.style.color = '';
    return;
  }
  const left = u.limitSecs - u.usedSecs;
  let text = `約${hm(u.usedSecs)} / 約${hm(u.limitSecs)}(残り約${hm(left)})`;
  if (u.checkpointMs) {
    const d = new Date(u.checkpointMs);
    text += ` 最後の記録: ${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  }
  voiceEdgeUsageEl.textContent = left <= 0 ? `上限に達しています(${text})` : text;
  voiceEdgeUsageEl.style.color = left < 3600 ? 'var(--c-ffd36b)' : '';
}
document.querySelector('nav button[data-tab="voice"]').addEventListener('click', renderEdgeUsage);
setInterval(() => {
  if (document.querySelector('main section[data-tab="voice"].active')) renderEdgeUsage();
}, 30000);
renderEdgeUsage();

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

// ホットキーの案内・エラーは「ホットキー」タブに出す
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
  const from = document.createElement('button');
  from.className = 'secondary';
  from.textContent = 'ここから';
  from.title = 'このレスから順に読み上げ直します';
  from.addEventListener('click', () => readFrom(p.no));
  const reply = document.createElement('button');
  reply.className = 'secondary';
  reply.textContent = '返信';
  reply.title = '下の書き込み欄に「>>番号」を入れます';
  reply.addEventListener('click', () => insertReplyAnchor(p.no));
  // 別のスレッドを見ている時は、読み上げ直しはできない(返信だけ)
  if (viewUrl) head.append(no, name, date, reply);
  else head.append(no, name, date, btn, from, reply);
  const body = document.createElement('div');
  body.className = 'post-body';
  body.textContent = p.body || '';
  el.append(head, body);
  el.addEventListener('contextmenu', (e) => openPostMenu(e, p));
  return el;
}

// ---- レスの右クリックメニュー(コピー・検索・ここから読む・NGワード・読み方に追加) ----
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
  const short = selected.slice(0, 20);
  // nullは区切り線
  const items = [
    [selected ? `「${short}」をコピー` : '文字を選んでから右クリックするとコピーできます', selected ? () => copyText(selected, `「${short}」をコピーしました`) : null],
    ['このレスの本文をコピー', p.body ? () => copyText(p.body, `レス${p.no}の本文をコピーしました`) : null],
    [selected ? `「${short}」をGoogleで検索` : '文字を選んでから右クリックすると検索できます', selected ? () => invoke('comments_search_web', { text: selected }).catch((err) => (postMenuMsgEl.textContent = String(err))) : null],
    null,
    [`ここから読み始める(レス${p.no}〜)`, p.no > 0 && !viewUrl ? () => readFrom(p.no) : null],
    null,
    [selected ? `「${short}」をNGワードに追加` : '文字を選んでから右クリックするとNGワードに追加できます', selected ? () => addNgWord(selected) : null],
    [selected ? `「${selected.slice(0, 20)}」の読み方を登録…` : '文字を選んでから右クリックすると読み方を登録できます', selected ? () => openReadingDialog(selected) : null],
    [p.name ? `名前「${p.name.slice(0, 20)}」をNGワードに追加` : '名前がありません', p.name ? () => addNgWord(p.name) : null],
    [id ? `ID「${id}」をNGワードに追加` : 'IDがありません', id ? () => addNgWord(`ID:${id}`) : null],
  ];
  postMenuEl.textContent = '';
  for (const item of items) {
    if (!item) {
      postMenuEl.appendChild(document.createElement('hr'));
      continue;
    }
    const [label, action] = item;
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

async function copyText(text, done) {
  try {
    await navigator.clipboard.writeText(text);
    postMenuMsgEl.textContent = done;
  } catch (err) {
    postMenuMsgEl.textContent = `コピーできませんでした: ${err}`;
  }
}

async function readFrom(no) {
  try {
    const n = await invoke('comments_read_from', { no });
    postMenuMsgEl.textContent = `レス${no}から読み上げ直します(${n}件)`;
  } catch (err) {
    postMenuMsgEl.textContent = String(err);
  }
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
document.querySelector('main').addEventListener('scroll', closePostMenu);

// ---- 表示するスレッドを選ぶ(前のスレッドなどを見るだけ) ----
/** 表示している別のスレッドのURL(空なら今読んでいるスレッド) */
let viewUrl = '';
const threadViewSelect = document.getElementById('threadViewSelect');
const threadViewSwitch = document.getElementById('threadViewSwitch');
const threadViewMsg = document.getElementById('threadViewMsg');
let threadChoicesLoading = false;

async function loadThreadChoices() {
  if (threadChoicesLoading) return;
  const status = await invoke('comments_status').catch(() => null);
  const url = (status && status.threadUrl) || (settings && settings.threadUrl) || '';
  if (!url.trim()) return;
  threadChoicesLoading = true;
  try {
    const { threads } = await invoke('comments_list_threads', { url });
    for (const o of Array.from(threadViewSelect.options)) if (o.value) o.remove();
    for (const t of threads) {
      if (t.current) continue;
      const o = document.createElement('option');
      o.value = t.url;
      o.textContent = `${t.title}(${t.count})`;
      threadViewSelect.appendChild(o);
    }
    // 見ているスレッドが一覧から消えていても(落ちた等)、選んだままにしておく
    if (viewUrl && !Array.from(threadViewSelect.options).some((o) => o.value === viewUrl)) {
      const o = document.createElement('option');
      o.value = viewUrl;
      o.textContent = viewUrl;
      threadViewSelect.appendChild(o);
    }
    threadViewSelect.value = viewUrl;
  } catch (err) {
    threadViewMsg.textContent = String(err);
  } finally {
    threadChoicesLoading = false;
  }
}

async function showThreadView(url) {
  viewUrl = url;
  threadViewSwitch.style.display = url ? '' : 'none';
  threadViewMsg.textContent = url ? '読み込んでいます…' : '';
  threadListEl.textContent = '';
  try {
    const r = url ? await invoke('comments_view_thread', { url }) : await invoke('comments_get_thread');
    if (viewUrl !== url) return;
    threadListEl.textContent = '';
    if (!(r.posts || []).length) resetPosts();
    appendPosts(r.posts || []);
    threadListEl.scrollTop = threadListEl.scrollHeight;
    threadViewMsg.textContent = url ? '見るだけ(読み上げ・字幕には出ません)' : '';
  } catch (err) {
    if (viewUrl === url) threadViewMsg.textContent = String(err);
  }
}

threadViewSelect.addEventListener('change', () => showThreadView(threadViewSelect.value));
document.getElementById('threadViewReload').addEventListener('click', async () => {
  await loadThreadChoices();
  if (viewUrl) showThreadView(viewUrl);
});
threadViewSwitch.addEventListener('click', async () => {
  if (!viewUrl) return;
  try {
    await invoke('comments_switch_thread', { url: viewUrl });
    threadViewSelect.value = '';
    await showThreadView('');
    loadThreadChoices();
  } catch (err) {
    threadViewMsg.textContent = String(err);
  }
});

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
    await invoke('comments_post', { body, url: viewUrl || null });
    // 見ているだけのスレッドは自動で読み込まないので、書き込んだら読み直す
    if (viewUrl) showThreadView(viewUrl);
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
  // Ctrl+Enter・Shift+Enter(設定でONの時)で書き込む(普通のEnterは改行。日本語の変換中の
  // Enterは確定なので書き込まない)
  const shiftPosts = !settings || settings.postShiftEnter !== false;
  if (e.key === 'Enter' && (e.ctrlKey || (e.shiftKey && shiftPosts)) && !e.isComposing) {
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

// ---- シンプルモード(使うものだけを大きく出す。この画面の中だけの好みなので、この端末に保存) ----
const SIMPLE_KEY = 'reacast.simpleMode';
const SETUP_DONE_KEY = 'reacast.setupDone';
const METHOD_KEY = 'reacast.streamMethod';
function storeGet(key) {
  try {
    return localStorage.getItem(key);
  } catch (_) {
    return null;
  }
}
function storeSet(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch (_) {}
}
function setSimpleMode(on) {
  document.body.classList.toggle('simple', on);
  if (window.HelpTips) window.HelpTips.close();
  storeSet(SIMPLE_KEY, on ? '1' : '0');
  if (on && settings) checkTts();
}
if (storeGet(SIMPLE_KEY) === '1') document.body.classList.add('simple');
for (const btn of document.querySelectorAll('#simpleModeOn, [data-simple-on]')) btn.addEventListener('click', () => setSimpleMode(true));
document.getElementById('simpleModeOff').addEventListener('click', () => setSimpleMode(false));

function hasSource() {
  return !!(
    (isUsed('useBbs') && (settings.threadUrl || settings.subThreadUrl)) ||
    (isUsed('useYoutube') && settings.youtubeTarget) ||
    (isUsed('useTwitch') && settings.twitchChannel)
  );
}
document.getElementById('simpleStart').addEventListener('click', () => {
  if (!hasSource()) {
    showStatusError('コメントの取得元が入っていません。掲示板・YouTube・Twitchのどれかを入れてください');
    return;
  }
  startBtn.click();
});
document.getElementById('simpleStop').addEventListener('click', () => stopBtn.click());
document.getElementById('simplePause').addEventListener('click', () => pauseBtn.click());
document.getElementById('simpleSkip').addEventListener('click', () => document.getElementById('skipBtn').click());
document.getElementById('simpleTest').addEventListener('click', () => {
  const text = document.getElementById('simpleTestText').value.trim();
  if (text) invoke('comments_test', { text });
});
// 細かい設定のボタンは、普通の画面に戻ってそのタブを開く
for (const btn of document.querySelectorAll('[data-simple-open]')) {
  btn.addEventListener('click', () => {
    setSimpleMode(false);
    showTab(btn.dataset.simpleOpen);
  });
}
// 読み上げソフトの選択肢は「声・速さ」タブのものを使い回す
for (const sel of document.querySelectorAll('[data-engine-mirror]')) sel.innerHTML = document.getElementById('speechEngine').innerHTML;

// 「視聴者のリアクションを表示する」(かんたん設定・コントロールパネルと同じ)
const reactionBoxes = document.querySelectorAll('[data-reactions-enabled]');
invoke('cfg_get', { key: 'reactionsEnabled' }).then((v) => {
  for (const box of reactionBoxes) box.checked = v !== false;
});
for (const box of reactionBoxes) {
  box.addEventListener('change', () => {
    invoke('set_reactions_enabled', { enabled: box.checked }).catch((e) => showStatusError(String(e)));
  });
}
listen('reactions:enabled', (e) => {
  for (const box of reactionBoxes) box.checked = !!e.payload;
});

// ---- 配信への出し方(OBSにURLを貼る / 画面をそのまま映す)。選んだ方の枠だけを出す ----
function streamMethod() {
  const saved = storeGet(METHOD_KEY);
  if (saved === 'obs' || saved === 'desktop') return saved;
  if (settings && (settings.desktopEnabled || settings.desktopListEnabled)) return 'desktop';
  return '';
}
function setStreamMethod(method) {
  storeSet(METHOD_KEY, method);
  // 画面をそのまま映す時は字幕をPCの画面に出す。URLを貼る時は、自分の画面に二重に出ないよう消す
  settings.desktopEnabled = method === 'desktop';
  if (method === 'obs') settings.desktopListEnabled = false;
  renderFields();
  scheduleSave();
  renderMethod();
}
/** 「使う」のチェックがONの時だけ、その下の欄を出す */
function renderShowIf() {
  for (const el of document.querySelectorAll('[data-show-if]')) el.classList.toggle('off', !getPath(settings, el.dataset.showIf));
}
for (const el of document.querySelectorAll('[data-key]')) {
  if (el.type === 'checkbox') el.addEventListener('change', () => settings && renderShowIf());
}
// 「音を選ぶ…」など、ほかのタブにあるボタンをそのまま押す
for (const btn of document.querySelectorAll('[data-click]')) {
  btn.addEventListener('click', () => document.getElementById(btn.dataset.click).click());
}

function renderMethod() {
  const method = streamMethod();
  for (const el of document.querySelectorAll('[data-method-only]')) el.classList.toggle('off', !!method && el.dataset.methodOnly !== method);
  if (settings) renderShowIf();
  for (const card of document.querySelectorAll('[data-method-card]')) card.classList.toggle('hidden', !!method && card.dataset.methodCard !== method);
  for (const el of document.querySelectorAll('[data-method-detail]')) el.classList.toggle('show', el.dataset.methodDetail === method);
  for (const btn of document.querySelectorAll('[data-wiz-method]')) btn.classList.toggle('selected', btn.dataset.wizMethod === method);
}
for (const btn of document.querySelectorAll('[data-set-method], [data-wiz-method]')) {
  btn.addEventListener('click', () => setStreamMethod(btn.dataset.setMethod || btn.dataset.wizMethod));
}

// ---- 読み上げソフトにつながるかを確かめて、つながらない時は何をすればいいかを出す ----
let ttsCheckSeq = 0;
async function checkTts() {
  const boxes = document.querySelectorAll('[data-tts-check]');
  const show = (cls, text, buttons) => {
    // 声の一覧は、つながった時だけ出す(つながっていないと選べないので)
    for (const el of document.querySelectorAll('[data-tts-ok-only]')) el.classList.toggle('tts-hidden', cls !== 'ok');
    for (const box of boxes) {
      box.className = `tts-check ${cls}`;
      box.textContent = text;
      if (buttons) {
        const row = document.createElement('div');
        row.className = 'inline';
        if (buttons.retry) {
          const b = document.createElement('button');
          b.className = 'secondary';
          b.textContent = 'もう一度確かめる';
          b.addEventListener('click', checkTts);
          row.appendChild(b);
        }
        if (buttons.download) {
          const b = document.createElement('button');
          b.className = 'secondary';
          b.textContent = 'ダウンロードページを開く';
          b.addEventListener('click', () => invoke('comments_open_tts_download', { engine: buttons.download }).catch((e) => showStatusError(String(e))));
          row.appendChild(b);
        }
        box.appendChild(row);
      }
    }
  };
  const engine = settings.speechEngine || 'command';
  const sel = document.getElementById('speechEngine');
  const name = (Array.from(sel.options).find((o) => o.value === engine) || { textContent: engine }).textContent;
  if (!settings.speechEnabled) return show('', '読み上げはOFFです。');
  if (engine === 'bouyomi') return show('', '棒読みちゃんを起動しておいてください(声・速さは棒読みちゃんの設定どおりに読みます)。', { download: 'bouyomi' });
  if (engine === 'command') return show('', 'ソフトの場所などは「声・速さの細かい設定」で決めてください。');
  const seq = ++ttsCheckSeq;
  show('', `${name}につながるか確かめています…`);
  try {
    const list = await listSpeakers(engine);
    if (seq !== ttsCheckSeq) return;
    fillSpeakerSelects(list);
    if (!settings.ttsSpeaker && list.length) {
      settings.ttsSpeaker = list[0].id;
      fillSpeakerSelects(list);
      scheduleSave();
    }
    show('ok', `✓ ${name}につながりました(声が${list.length}種類あります)。`);
  } catch (_) {
    if (seq !== ttsCheckSeq) return;
    show('ng', `✗ ${name}につながりません。${name}を起動してから「もう一度確かめる」を押してください。入っていない時は、ダウンロードページから入れてください(無料)。`, {
      retry: true,
      download: engine === 'voicevoxcompat' ? '' : engine,
    });
  }
}
for (const el of document.querySelectorAll('[data-engine-select], [data-key="speechEnabled"]')) {
  el.addEventListener('change', () => setTimeout(checkTts, 0));
}

// ---- 状態(シンプルモードの一番上) ----
function renderSimpleStatus(s) {
  const el = document.getElementById('simpleStatus');
  document.getElementById('simpleStop').disabled = !s.running;
  document.getElementById('simpleStart').textContent = s.running ? '▶ 再開始' : '▶ 開始';
  document.getElementById('simplePause').textContent = s.paused ? '読み上げを再開' : '一時停止';
  document.getElementById('simpleSkip').disabled = !s.running;
  el.className = 'simple-status';
  const sources = document.getElementById('simpleSources');
  sources.textContent = '';
  if (!s.running) {
    el.textContent = s.paused ? '停止中(読み上げは一時停止中)' : '停止中 —「▶ 開始」を押すとコメントの取得を始めます';
    return;
  }
  if (s.error) {
    el.className = 'simple-status ng';
    el.textContent = `うまく取得できていません: ${s.error}`;
  } else {
    el.className = 'simple-status on';
    el.textContent = s.paused ? '取得中(読み上げは一時停止中)' : '取得中 — コメントが届くと字幕に出して読み上げます';
  }
  const chip = (state, text) => {
    const span = document.createElement('span');
    const dot = document.createElement('i');
    dot.className = state;
    span.append(dot, text);
    sources.appendChild(span);
  };
  if (s.threadUrl) chip(s.error ? 'ng' : 'ok', s.threadTitle ? `掲示板:「${s.threadTitle}」 レス${s.lastNo}` : `掲示板: レス${s.lastNo}`);
  for (const src of s.sources || []) {
    const state = src.state === 'ok' ? 'ok' : src.state === 'error' ? 'ng' : '';
    chip(state, `${src.label}: ${String(src.message || '').split(/[(（]/)[0]}`);
  }
}

// ---- 取得元の知らせ(取得中に変えた時の「再開始で反映」と、まだ読めていない取得元) ----
let lastStatus = null;
// 今取得している取得元(開始した時の欄の中身)。止まっている時はnull
let appliedSources = null;
function sourceSnapshot() {
  return JSON.stringify([
    document.getElementById('threadUrl').value.trim(),
    (settings.subThreadUrl || '').trim(),
    (settings.youtubeTarget || '').trim(),
    (settings.twitchChannel || '').trim(),
    isUsed('useBbs'),
    isUsed('useYoutube'),
    isUsed('useTwitch'),
  ]);
}
startBtn.addEventListener('click', () => {
  appliedSources = sourceSnapshot();
});
for (const el of document.querySelectorAll('[data-key="threadUrl"], [data-key="subThreadUrl"], [data-key="youtubeTarget"], [data-key="twitchChannel"]')) {
  el.addEventListener('input', () => lastStatus && renderSourceNotes(lastStatus));
}

/** まだ読めていない取得元の、どうすればいいかの一言 */
function sourceHint(src) {
  const m = String(src.message || '');
  if (src.id === 'bbs2' && src.state === 'error') return '「2つ目の掲示板」のURLが合っているか、その掲示板をブラウザで開けるかを確かめてください(自動でやり直します)。';
  if (src.state === 'error') return 'インターネットの接続を確かめてください(30秒ごとに自動でやり直します)。';
  if (src.id === 'youtube' && m.includes('配信が始まるのを待っています')) {
    return 'チャンネルで公開の配信が始まると、自動で読み始めます。限定公開の配信や、すぐに読みたい時は、YouTubeの欄に配信のURL(watch?v=…)を入れて「再開始」を押してください。';
  }
  if (src.id === 'youtube' && m.includes('チャットを読み込めません')) return '配信が始まっているか、チャットがオフになっていないか確かめてください。';
  if (src.id === 'youtube' && m.includes('終了')) return '次の配信が始まると、自動で読み始めます。';
  return '';
}

// ×で隠した知らせ(取得元id→その時の文)。文が変わったらまた出す
const dismissedWarn = {};
// 「どうすれば?」を開いている取得元(状態の更新で閉じないように)
const openWarnHints = new Set();

function renderSourceNotes(s) {
  lastStatus = s;
  const startButtons = [startBtn, document.getElementById('simpleStart')];
  if (!s.running) appliedSources = null;
  else if (appliedSources === null) appliedSources = sourceSnapshot();
  const pending = s.running && settings && appliedSources !== sourceSnapshot();
  for (const el of document.querySelectorAll('[data-pending-note]')) {
    el.textContent = pending ? '取得元を変えました。「再開始」を押すと反映されます(今は前の取得元のまま読んでいます)。' : '';
  }
  for (const b of startButtons) b.classList.toggle('attention', pending);
  // 即時通知(jpnkn)は、つながらなくても定期取得で読めるので知らせない。つなぎ始めの「接続中…」も除く
  const problems = s.running ? (s.sources || []).filter((src) => !src.id.startsWith('jpnkn') && src.state !== 'ok' && !String(src.message || '').startsWith('接続中')) : [];
  const shown = problems.filter((src) => dismissedWarn[src.id] !== src.message);
  for (const el of document.querySelectorAll('[data-source-warn]')) {
    el.textContent = '';
    for (const src of shown) {
      const line = document.createElement('div');
      // 配信待ち・配信終了はいつものことなので目立たせない
      line.className = 'warn-line' + (src.state === 'error' ? ' bad' : '');
      const text = document.createElement('span');
      text.textContent = `${src.state === 'error' ? '⚠ ' : ''}${src.label}: ${src.message}`;
      line.appendChild(text);
      const hint = sourceHint(src);
      if (hint) {
        const link = document.createElement('button');
        link.className = 'warn-link';
        link.textContent = openWarnHints.has(src.id) ? '閉じる' : 'どうすれば?';
        link.addEventListener('click', () => {
          if (openWarnHints.has(src.id)) openWarnHints.delete(src.id);
          else openWarnHints.add(src.id);
          renderSourceNotes(lastStatus);
        });
        line.appendChild(link);
      }
      const close = document.createElement('button');
      close.className = 'warn-close';
      close.title = 'この知らせを隠す(変わったらまた出ます)';
      close.textContent = '×';
      close.addEventListener('click', () => {
        dismissedWarn[src.id] = src.message;
        renderSourceNotes(lastStatus);
      });
      line.appendChild(close);
      if (hint && openWarnHints.has(src.id)) {
        const h = document.createElement('div');
        h.className = 'warn-hint';
        h.textContent = hint;
        line.appendChild(h);
      }
      el.appendChild(line);
    }
  }
}

// ---- かんたんセットアップ(最初の案内。何度でもやり直せる) ----
const wizard = document.getElementById('setupWizard');
const WIZ_STEPS = Array.from(wizard.querySelectorAll('.wiz-step')).map((s) => s.dataset.step);
let wizIndex = 0;
let wizSourceWarned = false;
function renderWizard() {
  const step = WIZ_STEPS[wizIndex];
  for (const s of wizard.querySelectorAll('.wiz-step')) s.classList.toggle('active', s.dataset.step === step);
  const progress = document.getElementById('wizProgress');
  progress.textContent = '';
  // 「ようこそ」と「準備ができました」を除いた4つの手順を、棒で表す
  for (let i = 1; i < WIZ_STEPS.length - 1; i++) {
    const bar = document.createElement('i');
    if (i <= wizIndex) bar.className = 'on';
    progress.appendChild(bar);
  }
  progress.style.visibility = step === 'welcome' || step === 'done' ? 'hidden' : '';
  document.getElementById('wizBack').style.visibility = wizIndex === 0 ? 'hidden' : '';
  document.getElementById('wizLater').style.display = step === 'welcome' ? '' : 'none';
  document.getElementById('wizNext').textContent = step === 'welcome' ? '始める' : step === 'done' ? '完了' : '次へ';
  document.getElementById('wizSourceWarn').textContent = '';
  renderMethod();
  if (step === 'speech') checkTts();
  wizard.querySelector('.wiz-box').scrollTop = 0;
}
function openSetup() {
  if (window.HelpTips) window.HelpTips.close();
  setSimpleMode(true);
  wizIndex = 0;
  wizSourceWarned = false;
  wizard.classList.add('open');
  renderWizard();
}
function closeSetup() {
  wizard.classList.remove('open');
}
document.getElementById('wizNext').addEventListener('click', () => {
  const step = WIZ_STEPS[wizIndex];
  if (step === 'source' && !hasSource() && !wizSourceWarned) {
    wizSourceWarned = true;
    document.getElementById('wizSourceWarn').textContent = 'どれも入っていません。あとで入れる時は、このまま「次へ」を押してください。';
    return;
  }
  if (step === 'method' && !streamMethod()) {
    wizard.querySelector('.wiz-choices').animate(
      [{ transform: 'translateX(0)' }, { transform: 'translateX(-6px)' }, { transform: 'translateX(6px)' }, { transform: 'translateX(0)' }],
      { duration: 240 },
    );
    return;
  }
  if (step === 'done') {
    storeSet(SETUP_DONE_KEY, '1');
    renderSetupButtons();
    closeSetup();
    return;
  }
  wizIndex++;
  renderWizard();
});
document.getElementById('wizBack').addEventListener('click', () => {
  if (wizIndex > 0) wizIndex--;
  renderWizard();
});
for (const btn of wizard.querySelectorAll('[data-wiz-close]')) btn.addEventListener('click', closeSetup);
for (const btn of document.querySelectorAll('[data-setup-open]')) btn.addEventListener('click', openSetup);
document.getElementById('wizTestBtn').addEventListener('click', () => {
  invoke('comments_test', { text: 'テスト表示です。字幕と読み上げを確認できます' });
});
// コントロールパネルの「セットアップを始める」(窓が開いている時)
listen('settings:setup', openSetup);

// 「セットアップをやり直す」は、終えた後はシンプルモードの一番下へ移す(上にあると間違えて
// 押してしまうため)。コントロールパネルの「かんたんセットアップを表示する」がOFFなら出さない
function renderSetupButtons() {
  const done = storeGet(SETUP_DONE_KEY) === '1' || hasSource();
  const show = storeGet('reacast.showSetup') !== '0';
  for (const btn of document.querySelectorAll('[data-setup-open]')) btn.style.display = show ? '' : 'none';
  const btn = document.getElementById('simpleSetupBtn');
  if (done) document.getElementById('simpleFoot').appendChild(btn);
  else document.getElementById('simpleModeOff').before(btn);
}
window.addEventListener('storage', (e) => {
  if (e.key === 'reacast.showSetup' || e.key === SETUP_DONE_KEY) renderSetupButtons();
});

/** 設定を読み込んだ後に呼ぶ。初めての人は、シンプルモードとセットアップの案内から始める */
function initSimpleMode() {
  renderMethod();
  renderSetupButtons();
  const fromControl = new URLSearchParams(location.search).get('setup') === '1';
  const firstTime = storeGet(SETUP_DONE_KEY) !== '1' && storeGet(SIMPLE_KEY) === null && !hasSource();
  if (fromControl || firstTime) openSetup();
  else if (document.body.classList.contains('simple')) checkTts();
}

(async () => {
  settings = await invoke('comments_get_settings');
  settings.style = C.mergeStyle(settings.style);
  await loadMonitors();
  renderFields();
  initSimpleMode();
  renderStatus(await invoke('comments_status'));
  listen('comments:status', (e) => renderStatus(e.payload));
  // マウスで位置を決めた時など、Rust側で設定が変わった時に表示し直す
  listen('comments:settings', (e) => {
    settings = e.payload;
    settings.style = C.mergeStyle(settings.style);
    renderFields();
  });
  adjusting.subtitle = await invoke('comments_desktop_adjusting', { target: 'subtitle' });
  adjusting.list = await invoke('comments_desktop_adjusting', { target: 'list' });
  renderAdjust();
  listen('subtitle:adjust', (e) => {
    adjusting.subtitle = !!e.payload;
    renderAdjust();
  });
  listen('dlist:adjust', (e) => {
    adjusting.list = !!e.payload;
    renderAdjust();
  });
  const thread = await invoke('comments_get_thread');
  appendPosts(thread.posts || []);
  threadListEl.scrollTop = threadListEl.scrollHeight;
  listen('comments:posts', (e) => {
    // 別のスレッドを見ている間は、読み上げているスレッドのレスを混ぜない
    if (viewUrl) return;
    if (e.payload.reset) resetPosts();
    appendPosts(e.payload.posts || []);
  });
})();
