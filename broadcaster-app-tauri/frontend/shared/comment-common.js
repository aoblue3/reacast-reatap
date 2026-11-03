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
    peercast:
      '<svg viewBox="0 0 24 24" aria-label="PeerCast"><circle cx="12" cy="12" r="12" fill="#2e8b57"/><circle cx="12" cy="10" r="2.4" fill="#fff"/><path d="M11 12h2l2 8h-2l-.5-2h-1l-.5 2H9z" fill="#fff"/><path d="M7.3 5.3a6.6 6.6 0 0 0 0 9.4M16.7 5.3a6.6 6.6 0 0 1 0 9.4" stroke="#fff" stroke-width="1.6" fill="none" stroke-linecap="round"/></svg>',
    total:
      '<svg viewBox="0 0 24 24" aria-label="合計"><rect width="24" height="24" rx="6" fill="#3b6df0"/><path d="M7 6h10v2.4h-6.6l4 3.6-4 3.6H17V18H7v-2.2l4.4-3.8L7 8.2z" fill="#fff"/></svg>',
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
      icon.innerHTML = AUDIENCE_ICONS[r.id];
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
    .aud-icon svg { width: 1.25em; height: 1.25em; display: block; filter: drop-shadow(0 1px 2px rgba(0,0,0,0.5)); }
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
