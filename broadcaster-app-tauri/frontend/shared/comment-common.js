'use strict';
/**
 * コメント読み上げ・字幕機能の画面(subtitle.html / comments.html /
 * comment_settings.html)で共通に使う部品。
 *
 * 字幕・レス一覧の画面は、OBSのブラウザソースとして開かれた時も、
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
      align: 'center',
      // 縦の基準: 'top'=表示範囲の上端から下へ(1行目の位置が毎回同じ) / 'bottom'=下端に揃えて上へ
      valign: 'top',
      showResNumber: true,
      showName: false,
      showTime: false,
      offsetX: 0,
      offsetY: 0,
      // アンカー先のレスを先に表示する時の文字色(斜体で表示する)
      quoteColor: '#d6d6d6',
      // レスの翻訳(「下の字幕でレスを翻訳する」)の色と、本文に対する大きさ
      translationColor: '#ffe066',
      translationScale: 0.75,
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
      customCss: '',
    },
    // ニコ生風(右から左に流れる)
    nico: {
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
    // 話した字幕(voice.html)
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
  };
  const loadedWebFonts = new Set();
  function loadWebFont(name) {
    const family = WEB_FONTS[name];
    if (!family || loadedWebFonts.has(name) || typeof document === 'undefined') return;
    loadedWebFonts.add(name);
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = `https://fonts.googleapis.com/css2?family=${family}&display=swap`;
    document.head.appendChild(link);
  }

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
    return names.length ? `${names.map((n) => `"${n}"`).join(', ')}, sans-serif` : 'sans-serif';
  }

  /** obs_bridgeに繋ぎ、切れたら繋ぎ直し続ける。 */
  function connectBridge(onMessage) {
    const ws = new WebSocket(`ws://127.0.0.1:${BRIDGE_WS_PORT}`);
    ws.addEventListener('message', (event) => {
      let data;
      try {
        data = JSON.parse(event.data);
      } catch {
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

  window.CommentCommon = {
    DEFAULT_STYLE,
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
