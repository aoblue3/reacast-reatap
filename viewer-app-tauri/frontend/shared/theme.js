// 設定画面の表示テーマ(OSに合わせる/ライト/ダーク)。各画面の<head>で読み込み、
// <html data-theme="light|dark">を付ける(色は各画面のCSS変数で切り替える)。
// Windowsのタイトルバー(ウィンドウの枠)もsetThemeで同じ色にする。
// 選択はlocalStorageに保存する。同じアプリの画面どうしは保存先が共通なので、
// 1つの画面で変えると、開いている他の画面にもstorageイベントで反映される。
// 文字の大きさ(小=今まで通り/中/大)も同じ仕組みで、<select data-ui-size>で選ぶ。画面全体を
// 拡大する(WebView2の拡大。Ctrl+プラスと同じ)ので、文字もボタンも余白も一緒に大きくなる。
// broadcaster-app-tauri/viewer-app-tauri の shared/theme.js は同じ内容。
(function () {
  const SIZE_KEY = 'uiSize';
  const SIZES = { small: 1, medium: 1.15, large: 1.3 };

  function savedSize() {
    try {
      const v = localStorage.getItem(SIZE_KEY);
      return v in SIZES ? v : 'small';
    } catch (_) {
      return 'small';
    }
  }

  function applySize() {
    const size = savedSize();
    try {
      const wv = window.__TAURI__ && window.__TAURI__.webview.getCurrentWebview();
      if (wv) wv.setZoom(SIZES[size]).catch(() => {});
    } catch (_) {}
    for (const el of document.querySelectorAll('select[data-ui-size]')) el.value = size;
  }

  applySize();
  window.addEventListener('storage', (e) => {
    if (e.key === SIZE_KEY) applySize();
  });
  document.addEventListener('DOMContentLoaded', () => {
    for (const el of document.querySelectorAll('select[data-ui-size]')) {
      el.value = savedSize();
      el.addEventListener('change', () => {
        try {
          localStorage.setItem(SIZE_KEY, el.value);
        } catch (_) {}
        applySize();
      });
    }
  });
})();

(function () {
  const KEY = 'uiTheme';
  const media = window.matchMedia('(prefers-color-scheme: dark)');

  function saved() {
    try {
      const v = localStorage.getItem(KEY);
      return v === 'light' || v === 'dark' ? v : 'system';
    } catch (_) {
      return 'system';
    }
  }

  function apply() {
    const pref = saved();
    document.documentElement.dataset.theme = pref === 'system' ? (media.matches ? 'dark' : 'light') : pref;
    try {
      const w = window.__TAURI__ && window.__TAURI__.window.getCurrentWindow();
      // nullはOSの設定に従う
      if (w) w.setTheme(pref === 'system' ? null : pref).catch(() => {});
    } catch (_) {}
    for (const el of document.querySelectorAll('select[data-ui-theme]')) el.value = pref;
  }

  apply();
  media.addEventListener('change', apply);
  window.addEventListener('storage', (e) => {
    if (e.key === KEY) apply();
  });
  document.addEventListener('DOMContentLoaded', () => {
    for (const el of document.querySelectorAll('select[data-ui-theme]')) {
      el.value = saved();
      el.addEventListener('change', () => {
        try {
          localStorage.setItem(KEY, el.value);
        } catch (_) {}
        apply();
      });
    }
  });
})();
