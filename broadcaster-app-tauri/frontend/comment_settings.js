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
  monitorSelect.value = settings.desktopMonitorId || '';
  renderPreview();
}

function renderPreview() {
  const s = C.mergeStyle(settings.style).subtitle;
  const p = document.getElementById('preview');
  p.style.fontFamily = C.cssFontFamily(s.fontFamily);
  p.style.fontSize = `${Math.min(80, Number(s.fontSize) || 30)}px`;
  p.style.fontWeight = s.bold ? 'bold' : 'normal';
  p.style.fontStyle = s.italic ? 'italic' : 'normal';
  p.style.color = s.color;
  p.style.textShadow = C.outlineShadow(s.outlineWidth, s.outlineColor);
}

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
  if (!s.running) {
    add('停止中');
    return;
  }
  add('取得中', 'ok');
  if (s.threadTitle) add(` 「${s.threadTitle}」`);
  add(` レス${s.lastNo}`);
  if (s.queueLen > 0) add(` / 順番待ち${s.queueLen}件`);
  if (s.warning) add(` ${s.warning}`, 'warn');
  if (s.error) add(` ${s.error}`, 'ng');
}

startBtn.addEventListener('click', async () => {
  const url = document.getElementById('threadUrl').value.trim();
  if (!url) {
    showStatusError('スレッドのURLを入力してください');
    return;
  }
  try {
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
  head.append(no, name, date, btn);
  const body = document.createElement('div');
  body.className = 'post-body';
  body.textContent = p.body || '';
  el.append(head, body);
  return el;
}

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
  const thread = await invoke('comments_get_thread');
  appendPosts(thread.posts || []);
  threadListEl.scrollTop = threadListEl.scrollHeight;
  listen('comments:posts', (e) => {
    if (e.payload.reset) resetPosts();
    appendPosts(e.payload.posts || []);
  });
})();
