'use strict';
/**
 * 統計・盛り上がりマーカーの窓(stats.html)。記録はRust側(stats.rs)が持っていて、
 * ここは読んで表示するだけ。記録中の配信を見ている間は10秒おきに読み直す。
 */
const { invoke } = window.__TAURI__.core;
// リアクションの絵文字は、emoji-set.jsが置いているEMOJI_BY_ID(Map)から引く

const sessionSelect = document.getElementById('sessionSelect');
const msgEl = document.getElementById('msg');
let sessions = [];
let current = null;

const pad = (n) => String(n).padStart(2, '0');

function clock(ms) {
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function dateLabel(ms) {
  const d = new Date(ms);
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function duration(secs) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  return h ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

function emojiChar(id) {
  const e = EMOJI_BY_ID.get(id);
  return e ? e.char : id || '-';
}

function emojiLabel(id) {
  const e = EMOJI_BY_ID.get(id);
  return e ? e.label : id;
}

async function loadList(keepId) {
  try {
    sessions = await invoke('stats_list');
  } catch (e) {
    msgEl.textContent = String(e);
    return;
  }
  sessionSelect.textContent = '';
  if (!sessions.length) {
    msgEl.textContent = 'まだ記録がありません。コントロールパネルの「統計・盛り上がりマーカーを記録する」をONにすると、次に届いたリアクション・コメントから記録します。';
    document.getElementById('content').style.display = 'none';
    return;
  }
  msgEl.textContent = '';
  document.getElementById('content').style.display = '';
  for (const s of sessions) {
    const o = document.createElement('option');
    o.value = s.id;
    const mins = Math.max(1, Math.round((s.lastMs - s.startedMs) / 60000));
    o.textContent = `${dateLabel(s.startedMs)}(${mins}分・マーカー${s.markers}個)`;
    sessionSelect.appendChild(o);
  }
  sessionSelect.value = keepId && sessions.some((s) => s.id === keepId) ? keepId : sessions[0].id;
  await loadSession(sessionSelect.value);
}

async function loadSession(id) {
  current = await invoke('stats_get', { id }).catch(() => null);
  if (!current) return;
  render(current);
}

function render(s) {
  document.getElementById('tDuration').textContent = duration((s.lastMs - s.startedMs) / 1000);
  document.getElementById('tViewer').textContent = s.viewerReactions.toLocaleString();
  document.getElementById('tLinked').textContent = s.linkedReactions.toLocaleString();
  document.getElementById('tComments').textContent = s.comments.toLocaleString();

  // 多かったリアクション(上位12個)
  const totals = {};
  for (const m of s.minutes) for (const [id, n] of Object.entries(m.reactions)) totals[id] = (totals[id] || 0) + n;
  const top = Object.entries(totals).sort((a, b) => b[1] - a[1]).slice(0, 12);
  const topList = document.getElementById('topList');
  topList.textContent = '';
  if (!top.length) topList.innerHTML = '<span class="muted">まだありません</span>';
  for (const [id, n] of top) {
    const el = document.createElement('span');
    el.className = 'top-item';
    el.title = emojiLabel(id);
    const ch = document.createElement('span');
    ch.className = 'emoji';
    ch.textContent = emojiChar(id);
    el.append(ch, document.createTextNode(n.toLocaleString()));
    topList.appendChild(el);
  }

  // 1分ごとの棒グラフ
  const chart = document.getElementById('chart');
  chart.textContent = '';
  const markedMinutes = new Set(s.markers.map((m) => Math.floor(m.offsetSecs / 60)));
  const values = s.minutes.map((m) => Object.values(m.reactions).reduce((a, b) => a + b, 0) + m.comments);
  const max = Math.max(1, ...values);
  values.forEach((v, i) => {
    const bar = document.createElement('div');
    bar.className = 'bar' + (markedMinutes.has(i) ? ' marked' : '');
    bar.style.height = `${Math.round((v / max) * 100)}%`;
    bar.title = `${clock(s.startedMs + i * 60000)}〜 ${v}個`;
    chart.appendChild(bar);
  });
  document.getElementById('axisStart').textContent = clock(s.startedMs);
  document.getElementById('axisEnd').textContent = clock(s.lastMs);

  // マーカー
  const box = document.getElementById('markers');
  box.textContent = '';
  if (!s.markers.length) {
    box.innerHTML = '<div class="empty">まだ盛り上がりマーカーはありません</div>';
    return;
  }
  const table = document.createElement('table');
  table.innerHTML = '<tr><th>時刻</th><th>記録の開始から</th><th>10秒の数</th><th>多かったリアクション</th></tr>';
  for (const m of s.markers) {
    const tr = document.createElement('tr');
    for (const text of [clock(m.atMs), duration(m.offsetSecs), String(m.score)]) {
      const td = document.createElement('td');
      td.textContent = text;
      tr.appendChild(td);
    }
    const td = document.createElement('td');
    td.innerHTML = '<span class="emoji"></span> ';
    td.firstChild.textContent = m.top ? emojiChar(m.top) : '';
    td.appendChild(document.createTextNode(m.top ? emojiLabel(m.top) : 'コメント'));
    tr.appendChild(td);
    table.appendChild(tr);
  }
  box.appendChild(table);
}

sessionSelect.addEventListener('change', () => loadSession(sessionSelect.value));
document.getElementById('refreshBtn').addEventListener('click', () => loadList(sessionSelect.value));

// 消すボタンは2回押しで消す(間違えて押した時のため)
const deleteBtn = document.getElementById('deleteBtn');
let deleteArmed = null;
deleteBtn.addEventListener('click', async () => {
  if (!current) return;
  if (deleteArmed !== current.id) {
    deleteArmed = current.id;
    deleteBtn.textContent = 'もう一度押すと消します';
    setTimeout(() => {
      deleteArmed = null;
      deleteBtn.textContent = 'この記録を消す';
    }, 4000);
    return;
  }
  deleteArmed = null;
  deleteBtn.textContent = 'この記録を消す';
  try {
    await invoke('stats_delete', { id: current.id });
  } catch (e) {
    msgEl.textContent = String(e);
  }
  await loadList();
});

document.getElementById('copyMarkersBtn').addEventListener('click', async () => {
  if (!current) return;
  const lines = [`${dateLabel(current.startedMs)} の盛り上がりマーカー`];
  for (const m of current.markers) {
    lines.push(`${clock(m.atMs)}(記録の開始から ${duration(m.offsetSecs)}) ${m.top ? emojiChar(m.top) + ' ' : ''}${m.score}`);
  }
  try {
    await navigator.clipboard.writeText(lines.join('\n'));
    document.getElementById('copyMsg').textContent = 'コピーしました';
  } catch {
    document.getElementById('copyMsg').textContent = 'コピーできませんでした';
  }
  setTimeout(() => (document.getElementById('copyMsg').textContent = ''), 2000);
});

// 記録中(一番新しい記録で、最後に届いてから30分以内)を見ている間は、10秒おきに読み直す
setInterval(() => {
  if (!current || !sessions.length || current.id !== sessions[0].id) return;
  if (Date.now() - current.lastMs > 30 * 60000) return;
  loadSession(current.id);
}, 10000);

loadList();
