/**
 * アップデートの後、最初に起動した時だけ「新しくなったこと」を出す。
 * 中身はexeに埋め込んだCHANGELOG.md(Rust側のget_changelog)の、前に見たバージョンより新しい節。
 * このアプリ向けの節だけ出す(「### 追加(ReaTap)」のような見出しで分ける。アプリ名の無い見出しは両方向け)。
 * ReaCast・ReaTapの両方で同じファイルを使う(shared/whats-new.js)。
 */
(function () {
  const KEY = 'whatsNew.seenVersion';
  const MAX_VERSIONS = 5;

  function get(k) {
    try { return localStorage.getItem(k); } catch (_) { return null; }
  }
  function set(k, v) {
    try { localStorage.setItem(k, v); } catch (_) {}
  }

  /** "v1.10.0" → [1, 10, 0] */
  function parseVersion(v) {
    const m = String(v || '').match(/v?(\d+)\.(\d+)\.(\d+)/);
    return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
  }
  function newer(a, b) {
    for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
    return false;
  }

  /** CHANGELOGを [{version, title, groups: [{heading, items: [文]}]}] に分ける */
  function parseChangelog(text, app) {
    const out = [];
    let cur = null;
    let group = null;
    for (const raw of String(text).split(/\r?\n/)) {
      const line = raw.trimEnd();
      let m;
      if ((m = line.match(/^## (v\d+\.\d+\.\d+)(?::\s*(.*))?$/))) {
        cur = { version: m[1], title: m[2] || '', groups: [] };
        out.push(cur);
        group = null;
      } else if (cur && (m = line.match(/^### (.+)$/))) {
        const heading = m[1];
        // このアプリ向けの見出しだけ(中継サーバーの話は出さない)
        const label = heading.match(/[(（]([^)）]*)[)）]/);
        const forApp = label ? label[1].includes(app) : true;
        group = null;
        if (forApp && !heading.startsWith('中継サーバー')) {
          // 「追加(ReaCast)」と「追加(ReaCast・ReaTap)」は1つにまとめる
          const name = heading.replace(/[(（][^)）]*[)）]/, '').trim();
          group = cur.groups.find((g) => g.heading === name);
          if (!group) {
            group = { heading: name, items: [] };
            cur.groups.push(group);
          }
        }
      } else if (group && (m = line.match(/^- (.+)$/))) {
        group.items.push(m[1]);
      } else if (group && line.trim() && group.items.length) {
        group.items[group.items.length - 1] += line.trim();
      }
    }
    return out.filter((s) => s.groups.some((g) => g.items.length));
  }

  /** **太字** だけ太字にして、ほかは文字のまま出す */
  function richText(el, text) {
    String(text).split(/\*\*(.+?)\*\*/).forEach((part, i) => {
      if (!part) return;
      if (i % 2) {
        const b = document.createElement('b');
        b.textContent = part;
        el.appendChild(b);
      } else {
        el.appendChild(document.createTextNode(part));
      }
    });
  }

  function show(sections) {
    const overlay = document.createElement('div');
    overlay.className = 'confirm-overlay visible';
    const box = document.createElement('div');
    box.className = 'confirm-box';
    box.style.width = 'min(480px, calc(100vw - 32px))';
    const title = document.createElement('div');
    title.style.cssText = 'font-weight: bold; margin-bottom: 8px';
    title.textContent = '新しくなったこと';
    const body = document.createElement('div');
    body.className = 'confirm-message';
    body.style.whiteSpace = 'normal';
    for (const s of sections) {
      const h = document.createElement('div');
      h.style.cssText = 'font-weight: bold; margin: 6px 0 2px';
      h.textContent = s.version + (s.title ? ': ' + s.title : '');
      body.appendChild(h);
      for (const g of s.groups) {
        if (!g.items.length) continue;
        const gh = document.createElement('div');
        gh.style.cssText = 'opacity: 0.7; font-size: 12px';
        gh.textContent = g.heading;
        body.appendChild(gh);
        const ul = document.createElement('ul');
        ul.style.cssText = 'margin: 2px 0 6px; padding-left: 1.3em';
        for (const it of g.items) {
          const li = document.createElement('li');
          li.style.marginBottom = '3px';
          richText(li, it);
          ul.appendChild(li);
        }
        body.appendChild(ul);
      }
    }
    const buttons = document.createElement('div');
    buttons.className = 'confirm-buttons';
    const ok = document.createElement('button');
    ok.textContent = '閉じる';
    ok.addEventListener('click', () => overlay.remove());
    buttons.appendChild(ok);
    box.append(title, body, buttons);
    overlay.appendChild(box);
    document.body.appendChild(overlay);
    ok.focus();
  }

  /**
   * 起動時に呼ぶ。
   * @param {string} app 'ReaCast' か 'ReaTap'
   * @param {string} version 今のバージョン(開発版は空)
   * @param {string} changelog CHANGELOG.mdの中身
   * @param {boolean} firstUse はじめて使う人か(はじめての人には出さない)
   */
  function maybeShow(app, version, changelog, firstUse) {
    const now = parseVersion(version);
    if (!now) return;
    const seenText = get(KEY);
    set(KEY, version);
    const seen = parseVersion(seenText);
    if (seenText === version) return;
    // 前に見たバージョンが分からない時は、はじめての人には出さず、前から使っている人には今回の分だけ出す
    if (!seen && firstUse) return;
    const sections = parseChangelog(changelog, app)
      .filter((s) => {
        const v = parseVersion(s.version);
        return !newer(v, now) && (seen ? newer(v, seen) : s.version === version);
      })
      .slice(0, MAX_VERSIONS);
    if (sections.length) show(sections);
  }

  window.WhatsNew = { maybeShow, parseChangelog };
})();
