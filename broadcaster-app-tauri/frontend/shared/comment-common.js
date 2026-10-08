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
      customCss: '',
    },
    aa: {
      fontFamily: 'MS UI Gothic',
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

  function cssFontFamily(name) {
    const n = String(name || '').replace(/["\\]/g, '');
    return n ? `"${n}", sans-serif` : 'sans-serif';
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
    connectBridge,
    findImageUrls,
    IMAGE_URL_RE,
    resHeader,
    rgba,
    applyCustomCss,
  };
})();
