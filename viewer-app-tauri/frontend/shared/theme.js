// 設定画面の表示テーマ(OSに合わせる/ライト/ダーク)。各画面の<head>で読み込み、
// <html data-theme="light|dark">を付ける(色は各画面のCSS変数で切り替える)。
// Windowsのタイトルバー(ウィンドウの枠)もsetThemeで同じ色にする。
// 選択はlocalStorageに保存する。同じアプリの画面どうしは保存先が共通なので、
// 1つの画面で変えると、開いている他の画面にもstorageイベントで反映される。
// broadcaster-app-tauri/viewer-app-tauri の shared/theme.js は同じ内容。
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
