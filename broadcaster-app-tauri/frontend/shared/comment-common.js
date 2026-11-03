'use strict';
/**
 * コメント読み上げ・字幕機能の画面(subtitle.html / comments.html /
 * comment_settings.html)で共通に使う部品。
 *
 * コメント字幕・コメント一覧の画面は、OBSのブラウザソースとして開かれた時も、
 * ReaCastのデスクトップ字幕ウィンドウとして開かれた時も、同じく
 * obs_bridge(ReaCastがローカルに立てているWebSocket)から表示内容を
 * 受け取る。どちらも同じメッセージを同時に受け取るので、表示がずれない。
 */
(function () {
  const BRIDGE_WS_PORT = 18771; // obs_bridge.rsのOBS_WS_PORTと同じ

  // 見た目の既定値。Rust側は見た目の設定を解釈せずそのまま配るだけなので、
  // 既定値はここで一元管理する(設定画面の初期値にもこれを使う)。
  const DEFAULT_STYLE = {
    subtitle: {
      fontFamily: 'BIZ UDゴシック',
      fontSize: 40,
      bold: true,
      italic: false,
      color: '#ffffff',
      outlineWidth: 3,
      outlineColor: '#1b1f7a',
      // 字幕の出し方: 'instant'=瞬間表示 / 'typewriter'=1文字ずつ / 'fade'=フェードイン /
      // 'slide-left'=左からスライド / 'slide-up'=下からスライド
      appear: 'instant',
      vertical: false,
      align: 'left',
      // 縦の基準: 'top'=表示範囲の上端から下へ(1行目の位置が毎回同じ) / 'bottom'=下端に揃えて上へ
      valign: 'top',
      showResNumber: true,
      showName: false,
      showTime: false,
      offsetX: 0,
      offsetY: 0,
      // アンカー先のレスを先に表示する時の文字色(斜体で表示する)
      quoteColor: '#d6d6d6',
      // レスの翻訳(「コメント字幕でレスを翻訳する」)の色と、本文に対する大きさ
      translationColor: '#ffe066',
      translationScale: 0.75,
      // 字幕の背景(不透明度0なら背景なし)・余白(0なら既定)・角の丸み
      bgColor: '#000000',
      bgOpacity: 0,
      padding: 0,
      radius: 0,
      // 今読んでいるスレッドのタイトルを小さく出す(OBSのコメント字幕だけ。既定OFF)。
      // 位置は表示範囲の四隅・上下の真ん中(top-left等)と、そこからのずらし(px)
      threadTitleShow: false,
      threadTitlePos: 'top-left',
      threadTitleOffsetX: 0,
      threadTitleOffsetY: 0,
      threadTitleSize: 18,
      threadTitleColor: '#ffffff',
      threadTitleBgColor: '#000000',
      threadTitleBgOpacity: 55,
      // デスクトップ字幕にもプリセットの見た目を使う(OFFなら、デスクトップ字幕は
      // 文字の大きさ・位置などはそのままで、プリセットの部分だけ既定の見た目にする)
      desktopUsePreset: true,
      customCss: '',
    },
    aa: {
      // 2ちゃんねるのAAはＭＳ Ｐゴシックで描かれる前提(MS UI Gothicはかなの幅が違ってずれる)
      fontFamily: 'ＭＳ Ｐゴシック',
      pgothic: true,
      fontSize: 30,
      color: '#ffffff',
      outlineWidth: 3,
      outlineColor: '#1b1f7a',
      shrinkToWidth: true,
      shrinkToHeight: true,
    },
    list: {
      fontFamily: 'BIZ UDゴシック',
      fontSize: 22,
      // 見出し(レス番号・名前・時刻)の文字の大きさ(px。0なら本文の80%)と色
      headFontSize: 0,
      headBold: true,
      numberColor: '#7dff7d',
      nameColor: '#7dff7d',
      dateColor: '#bbbbbb',
      bodyColor: '#ffffff',
      bodyBold: false,
      outlineWidth: 2,
      outlineColor: '#000000',
      // レスごとの背景(不透明度0なら背景なし)
      itemBgColor: '#000000',
      itemBgOpacity: 0,
      itemPadding: 0,
      itemRadius: 6,
      maxItems: 30,
      newestFirst: true,
      // 行間(文字の大きさに対する倍率)と、レスとレスの間の空き(px)
      lineHeight: 1.3,
      itemGap: 10,
      showResNumber: true,
      showName: true,
      showDate: false,
      wrap: true,
      separateLines: true,
      // 'none'=表示しない / 'chat'=レス一覧に画像のサムネイルを表示する
      thumbnails: 'none',
      hideImageUrl: false,
      showIcon: false,
      iconSize: 40,
      // 配信サイトのコメントに「YouTube」「Twitch」の印を付ける
      showSource: true,
      // スーパーチャット・ビッツ等の応援を、色付きの札と金額で目立たせる
      showPaid: true,
      customCss: '',
    },
    // 流れるコメント(右から左に流れる)
    nico: {
      // OFFにすると流さない(OBSのソースを消さなくても止められる)
      enabled: true,
      fontFamily: 'BIZ UDゴシック',
      fontSize: 40,
      bold: true,
      color: '#ffffff',
      outlineWidth: 2.5,
      outlineColor: '#000000',
      // 画面を横切るのにかかる秒数(ニコニコと同じく、長いコメントほど速く流れる)
      durationSec: 5,
      opacity: 100,
      // 同時に流す最大の行数(0なら画面の高さに収まるだけ)
      maxLines: 0,
      showAA: false,
      // AAだけを流す / 読み上げの順番を待たず、届いた瞬間に流す
      aaOnly: false,
      instant: false,
      customCss: '',
    },
    // 視聴者数・コメント数(audience.html と、コメント一覧の上)
    audience: {
      // 'separate'=取得元ごと / 'total'=合計だけ / 'both'=取得元ごと+合計
      mode: 'separate',
      showViewers: true,
      showComments: true,
      // 'row'=横に並べる / 'column'=縦に並べる
      layout: 'row',
      fontFamily: 'BIZ UDゴシック',
      fontSize: 28,
      bold: true,
      color: '#ffffff',
      outlineWidth: 2.5,
      outlineColor: '#000000',
      bgColor: '#000000',
      bgOpacity: 0,
      // コメント一覧の上にも出す(大きさはコメント一覧の文字に合わせる)
      showInList: false,
      // 合計の印: 'sigma' | 'people' | 'star' | 'text'(「合計」の文字) | 'none'
      totalIcon: 'sigma',
      customCss: '',
    },
    // 自分の声の字幕(voice.html)
    voice: {
      fontFamily: 'BIZ UDゴシック',
      fontSize: 36,
      bold: true,
      color: '#ffffff',
      // 翻訳の文字の大きさ・色
      translationFontSize: 30,
      translationColor: '#ffe066',
      outlineWidth: 3,
      outlineColor: '#000000',
      // 'both'=話した言葉と翻訳 / 'original'=話した言葉だけ / 'translation'=翻訳だけ
      show: 'both',
      align: 'center',
      valign: 'bottom',
      // 1つの段落(話した1文+翻訳)を出しておく最低の時間(ミリ秒)。長い文はもっと長く出す
      holdMs: 6000,
      // 同時に出しておく段落の数(古いものから消える)
      maxBlocks: 3,
      customCss: '',
    },
  };

  function mergeStyle(style) {
    const s = style && typeof style === 'object' ? style : {};
    const out = {};
    for (const key of Object.keys(DEFAULT_STYLE)) {
      out[key] = { ...DEFAULT_STYLE[key], ...(s[key] && typeof s[key] === 'object' ? s[key] : {}) };
    }
    // 以前の「瞬間表示する」チェックボックス(instant)だけが保存されている設定からの引き継ぎ
    const sub = s.subtitle || {};
    if (sub.appear === undefined && sub.instant === false) out.subtitle.appear = 'typewriter';
    // 以前の既定(MS UI Gothic)のままのAAのフォントは、一度だけＭＳ Ｐゴシックにする
    // (pgothicが無い=この引き継ぎ前の設定。その後に選び直したフォントはそのまま)
    const aa = s.aa || {};
    if (!aa.pgothic && (!aa.fontFamily || aa.fontFamily === 'MS UI Gothic')) out.aa.fontFamily = DEFAULT_STYLE.aa.fontFamily;
    return out;
  }

  /** 文字の縁取り。text-shadowを円周上に並べて作る(OBSのブラウザソースの
   * 古めのChromiumでも同じ見た目になるよう、-webkit-text-strokeは使わない)。 */
  function outlineShadow(width, color) {
    const w = Number(width) || 0;
    if (w <= 0) return 'none';
    const steps = Math.max(8, Math.min(24, Math.round(w * 6)));
    const parts = [];
    for (let i = 0; i < steps; i++) {
      const a = (i / steps) * Math.PI * 2;
      parts.push(`${(Math.cos(a) * w).toFixed(2)}px ${(Math.sin(a) * w).toFixed(2)}px 0 ${color}`);
    }
    return parts.join(', ');
  }

  /** 選べるウェブフォント(Google Fonts。ReaCastには入れず、表示する時にネットから
   * 読み込むので、容量は増えない)。値はGoogle Fonts のURLのfamily=の部分。 */
  const WEB_FONTS = {
    'Noto Sans JP': 'Noto+Sans+JP:wght@400;700;900',
    'M PLUS 1p': 'M+PLUS+1p:wght@500;800',
    'M PLUS Rounded 1c': 'M+PLUS+Rounded+1c:wght@500;800',
    'Zen Kaku Gothic New': 'Zen+Kaku+Gothic+New:wght@500;700',
    'Zen Maru Gothic': 'Zen+Maru+Gothic:wght@500;700',
    'Kosugi Maru': 'Kosugi+Maru',
    'Kiwi Maru': 'Kiwi+Maru:wght@500',
    'Noto Serif JP': 'Noto+Serif+JP:wght@400;700',
    'Shippori Mincho': 'Shippori+Mincho:wght@500;700',
    'Klee One': 'Klee+One:wght@600',
    'Yomogi': 'Yomogi',
    'Hachi Maru Pop': 'Hachi+Maru+Pop',
    'Yusei Magic': 'Yusei+Magic',
    'Mochiy Pop One': 'Mochiy+Pop+One',
    'Potta One': 'Potta+One',
    'RocknRoll One': 'RocknRoll+One',
    'Reggae One': 'Reggae+One',
    'Rampart One': 'Rampart+One',
    'Train One': 'Train+One',
    'Stick': 'Stick',
    'Dela Gothic One': 'Dela+Gothic+One',
    'DotGothic16': 'DotGothic16',
    'Press Start 2P': 'Press+Start+2P',
    'Dancing Script': 'Dancing+Script:wght@500;700',
    'Zen Kurenaido': 'Zen+Kurenaido',
    'Yuji Syuku': 'Yuji+Syuku',
  };
  const loadedWebFonts = new Set();
  function loadWebFont(name) {
    const family = WEB_FONTS[name];
    if (!family || loadedWebFonts.has(name) || typeof document === 'undefined') return;
    loadedWebFonts.add(name);
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = `https://fonts.googleapis.com/css2?family=${family}&display=swap`;
    // 読み込み終わったら、フォント自体もすぐ取ってくる(使う文字が出るまで取りに行かない
    // ことがあり、選んでも見た目が変わらないように見えたため)
    link.onload = () => {
      if (document.fonts && document.fonts.load) {
        document.fonts.load(`20px "${name}"`, 'あア亜Aa1').catch(() => {});
      }
    };
    document.head.appendChild(link);
  }

  /**
   * 新しい絵文字のフォント(設定画面でダウンロードした人だけ。src-tauri/src/emoji_font.rs)。
   * Windowsの絵文字のフォントに無い絵文字(🫍など)だけ、これで表示する(フォントの並びの
   * 最後に置くので、今まで表示できていた絵文字の見た目は変わらない)。絵文字の範囲の文字が
   * 出てきた時だけ読み込む。ダウンロードしていなければ読み込みに失敗するだけ。
   */
  const EMOJI_FONT = '"ReaCast Emoji"';
  // 「すべての絵文字をGoogleの絵文字にする」がONの時だけ読める(OFFなら404で、次のフォントになる)
  const ALL_EMOJI_FONT = '"ReaCast All Emoji"';
  function setupEmojiFont(version) {
    let el = document.getElementById('reacast-emoji-font');
    if (!el) {
      el = document.createElement('style');
      el.id = 'reacast-emoji-font';
      document.head.appendChild(el);
    }
    const range = 'U+00A9, U+00AE, U+203C-2BFF, U+3030, U+303D, U+3297, U+3299, U+1F000-1FBFF, U+E0020-E007F';
    el.textContent =
      `@font-face { font-family: "ReaCast Emoji"; src: url("http://127.0.0.1:18772/emoji-font.ttf?v=${version || 0}") format("truetype"); unicode-range: ${range}; }\n` +
      `@font-face { font-family: "ReaCast All Emoji"; src: url("http://127.0.0.1:18772/emoji-font-all.ttf?v=${version || 0}") format("truetype"); unicode-range: ${range}; }`;
  }
  setupEmojiFont(0);

  /** フォント名をCSSのfont-familyにする。「A, B」のように複数並べると、Aに無い文字は
   * Bで表示する(英数字だけのフォント + 日本語のフォント、のような組み合わせ用)。
   * ウェブフォントなら、その場で読み込む。 */
  function cssFontFamily(name) {
    const names = String(name || '')
      .replace(/["\\]/g, '')
      .split(/[,、]/)
      .map((n) => n.trim())
      .filter(Boolean);
    names.forEach(loadWebFont);
    return names.length
      ? `${ALL_EMOJI_FONT}, ${names.map((n) => `"${n}"`).join(', ')}, ${EMOJI_FONT}, sans-serif`
      : `${ALL_EMOJI_FONT}, ${EMOJI_FONT}, sans-serif`;
  }

  /** obs_bridgeに繋ぎ、切れたら繋ぎ直し続ける。 */
  /** ReaCastがアップデートされて起動し直したら(繋ぎ直した時のバージョンが前と違えば)、
   * 画面を読み込み直す(OBSのブラウザソースが古い画面のまま動き続けないように)。 */
  let seenAppVersion = null;
  function checkAppVersion(version) {
    if (seenAppVersion !== null && seenAppVersion !== version) location.reload();
    seenAppVersion = version;
  }

  function connectBridge(onMessage) {
    const ws = new WebSocket(`ws://127.0.0.1:${BRIDGE_WS_PORT}`);
    ws.addEventListener('message', (event) => {
      let data;
      try {
        data = JSON.parse(event.data);
      } catch {
        return;
      }
      if (data && data.type === 'appVersion') {
        checkAppVersion(data.version);
        return;
      }
      // 新しい絵文字のフォントをダウンロード・削除した → 読み込み直す
      if (data && data.type === 'emojiFont') {
        setupEmojiFont(Date.now());
        return;
      }
      if (data && typeof data.type === 'string') onMessage(data);
    });
    ws.addEventListener('close', () => setTimeout(() => connectBridge(onMessage), 1500));
    ws.addEventListener('error', () => ws.close());
  }

  /** #rrggbb と不透明度(0〜100)から rgba() を作る。 */
  function rgba(hex, opacityPercent) {
    const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(String(hex || ''));
    const a = Math.max(0, Math.min(100, Number(opacityPercent) || 0)) / 100;
    if (!m) return `rgba(0,0,0,${a})`;
    return `rgba(${parseInt(m[1], 16)},${parseInt(m[2], 16)},${parseInt(m[3], 16)},${a})`;
  }

  /** 設定の「カスタムCSS」をページに差し込む(配信者が自分の画面用に書くCSS)。 */
  function applyCustomCss(css) {
    let el = document.getElementById('reacast-custom-css');
    if (!el) {
      el = document.createElement('style');
      el.id = 'reacast-custom-css';
      document.head.appendChild(el);
    }
    el.textContent = String(css || '');
  }

  /** 本文に含まれる画像のURL(サムネイル表示用)。 */
  const IMAGE_URL_RE = /https?:\/\/[^\s"'<>]+?\.(?:png|jpe?g|gif|webp)(?:\?[^\s"'<>]*)?(?=$|[\s"'<>])/gi;
  function findImageUrls(text) {
    return String(text || '').match(IMAGE_URL_RE) || [];
  }

  /** レスの見出し(番号・名前・時刻)を、表示設定に合わせて組み立てる。 */
  function resHeader(res, opts) {
    const parts = [];
    if (opts.showResNumber && res.no > 0 && !res.system) parts.push(String(res.no));
    if (opts.showName && res.name) parts.push(res.name);
    if (opts.showDate && res.date) parts.push(res.date);
    return parts.join(' ');
  }

  /** プリセットで置き換える項目(文字の大きさ・位置・件数などの、その人の設定は含めない) */
  const LIST_PRESET_KEYS = [
    'fontFamily', 'headFontSize', 'headBold', 'numberColor', 'nameColor', 'dateColor', 'bodyColor', 'bodyBold',
    'outlineWidth', 'outlineColor', 'itemBgColor', 'itemBgOpacity', 'itemPadding', 'itemRadius', 'customCss',
  ];
  const SUBTITLE_PRESET_KEYS = [
    'fontFamily', 'bold', 'italic', 'color', 'outlineWidth', 'outlineColor', 'bgColor', 'bgOpacity', 'padding', 'radius',
    'appear', 'translationColor', 'customCss',
  ];

  // ---- 視聴者数・コメント数 ----
  // 取得元の印(サービスを指す小さな印。YouTube・Twitchは各社の色、PeerCastはアンテナ)
  const AUDIENCE_ICONS = {
    youtube:
      '<svg viewBox="0 0 28 20" aria-label="YouTube"><rect width="28" height="20" rx="5" fill="#ff0000"/><path d="M11 5.5v9l8-4.5z" fill="#fff"/></svg>',
    twitch:
      '<svg viewBox="0 0 24 26" aria-label="Twitch"><path d="M2 0 0 5v18h6v3h3l3-3h5l7-7V0z" fill="#9146ff"/><path d="M4 2h18v11l-4 4h-6l-3 3v-3H4z" fill="#fff"/><path d="M11 6h2v6h-2zm5 0h2v6h-2z" fill="#9146ff"/></svg>',
    // PeerCastのメガホン(配信者さんが選んだ絵。外側の白は抜いてある)
    peercast: '<img alt="PeerCast" src="data:image/png;base64,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" />',
    total: '',
  };
  // 合計の印(「視聴者数・コメント数」タブの「合計の印」で選ぶ)
  const TOTAL_ICONS = {
    sigma:
      '<svg viewBox="0 0 24 24" aria-label="合計"><rect width="24" height="24" rx="6" fill="#3b6df0"/><path d="M7 6h10v2.4h-6.6l4 3.6-4 3.6H17V18H7v-2.2l4.4-3.8L7 8.2z" fill="#fff"/></svg>',
    people:
      '<svg viewBox="0 0 24 24" aria-label="合計"><rect width="24" height="24" rx="6" fill="#3b6df0"/><circle cx="9" cy="9" r="3" fill="#fff"/><circle cx="16" cy="9.5" r="2.4" fill="#fff" opacity="0.85"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0zM13 19a4.5 4.5 0 0 1 8 0z" fill="#fff"/></svg>',
    star:
      '<svg viewBox="0 0 24 24" aria-label="合計"><path d="m12 1.8 3.1 6.6 7.2.9-5.3 5 1.4 7.1L12 17.9l-6.4 3.5L7 14.3l-5.3-5 7.2-.9z" fill="#ffc83d"/></svg>',
    text: '<span class="aud-total-text">合計</span>',
    none: '',
  };
  const VIEWER_ICON =
    '<svg viewBox="0 0 24 24" aria-label="視聴者"><circle cx="12" cy="8" r="4.2" fill="currentColor"/><path d="M3.5 21a8.5 8.5 0 0 1 17 0z" fill="currentColor"/></svg>';
  const COMMENT_ICON =
    '<svg viewBox="0 0 24 24" aria-label="コメント"><path d="M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-9l-5 4v-4H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" fill="currentColor"/></svg>';

  /** 視聴者数・コメント数を描く。state = { running, sources: [{ id, viewers, comments }] } */
  function renderAudience(el, state, cfg) {
    el.textContent = '';
    const sources = (state && Array.isArray(state.sources) ? state.sources : []).filter((s) => AUDIENCE_ICONS[s.id]);
    if (!state || !state.running || !sources.length || (!cfg.showViewers && !cfg.showComments)) {
      el.classList.add('empty');
      return;
    }
    el.classList.remove('empty');
    const rows = [];
    if (cfg.mode !== 'total') rows.push(...sources);
    if (cfg.mode !== 'separate') {
      const known = sources.filter((s) => s.viewers != null);
      rows.push({
        id: 'total',
        viewers: known.length ? known.reduce((n, s) => n + s.viewers, 0) : null,
        comments: sources.reduce((n, s) => n + (Number(s.comments) || 0), 0),
      });
    }
    const fmt = (n) => (n == null ? '-' : Number(n).toLocaleString('ja-JP'));
    for (const r of rows) {
      const item = document.createElement('span');
      item.className = `aud-item aud-${r.id}`;
      const icon = document.createElement('span');
      icon.className = 'aud-icon';
      icon.innerHTML = r.id === 'total' ? TOTAL_ICONS[cfg.totalIcon] ?? TOTAL_ICONS.sigma : AUDIENCE_ICONS[r.id];
      item.appendChild(icon);
      const part = (cls, svg, n) => {
        const p = document.createElement('span');
        p.className = cls;
        p.innerHTML = svg;
        p.appendChild(document.createTextNode(fmt(n)));
        item.appendChild(p);
      };
      if (cfg.showViewers) part('aud-viewers', VIEWER_ICON, r.viewers);
      if (cfg.showComments) part('aud-comments', COMMENT_ICON, r.comments);
      el.appendChild(item);
    }
  }

  // renderAudienceで描いた物の見た目(audience.html・comments.html・設定画面で同じ)
  const AUDIENCE_CSS = `
    .aud { display: flex; flex-wrap: wrap; align-items: center; gap: 0.25em 0.9em; line-height: 1.2; }
    .aud.column { flex-direction: column; align-items: flex-start; }
    .aud.empty { display: none; }
    .aud-item { display: inline-flex; align-items: center; gap: 0.3em; white-space: nowrap; }
    .aud-icon svg, .aud-icon img { width: 1.25em; height: 1.25em; display: block; filter: drop-shadow(0 1px 2px rgba(0,0,0,0.5)); }
    .aud-icon:empty { display: none; }
    /* PeerCastのメガホンは黒いので、暗い配信画面でも見えるように白い縁を付ける */
    .aud-peercast .aud-icon img { filter: drop-shadow(0 0 1px #fff) drop-shadow(0 0 1px #fff); }
    .aud-total-text { font-size: 0.8em; padding: 0 0.3em; border-radius: 0.25em; background: #3b6df0; color: #fff; text-shadow: none; }
    .aud-viewers, .aud-comments { display: inline-flex; align-items: center; gap: 0.12em; font-variant-numeric: tabular-nums; }
    .aud-viewers svg, .aud-comments svg { width: 0.85em; height: 0.85em; opacity: 0.85; }
    .aud-comments { margin-left: 0.25em; }`;

  window.CommentCommon = {
    renderAudience,
    AUDIENCE_CSS,
    DEFAULT_STYLE,
    LIST_PRESET_KEYS,
    SUBTITLE_PRESET_KEYS,
    mergeStyle,
    outlineShadow,
    cssFontFamily,
    WEB_FONTS,
    connectBridge,
    findImageUrls,
    IMAGE_URL_RE,
    resHeader,
    rgba,
    applyCustomCss,
  };
})();
