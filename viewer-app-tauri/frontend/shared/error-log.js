// 画面側のエラー・警告を、Rust側のログファイル(src-tauri/src/applog.rs)にも書き残す。
// console.error/console.warn、捕まえられなかった例外、await漏れのPromiseのエラーが対象。
// OBSのブラウザソース等、Tauriの外で開かれた時は何もしない。
// broadcaster-app-tauri/viewer-app-tauri の shared/error-log.js は同じ内容。
(function () {
  const tauri = window.__TAURI__;
  if (!tauri || !tauri.core) return;

  // 同じエラーが繰り返し出てもログが膨れないよう、1分あたりの件数に上限を設ける
  const MAX_PER_MINUTE = 30;
  let windowStart = Date.now();
  let count = 0;

  function format(value) {
    if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
    if (typeof value === 'string') return value;
    try {
      return JSON.stringify(value);
    } catch (_) {
      return String(value);
    }
  }

  function send(level, parts) {
    const now = Date.now();
    if (now - windowStart > 60000) {
      windowStart = now;
      count = 0;
    }
    if (++count > MAX_PER_MINUTE) return;
    const message = parts.map(format).join(' ');
    tauri.core.invoke('log_from_frontend', { level, message }).catch(() => {});
  }

  for (const level of ['error', 'warn']) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      original(...args);
      send(level, args);
    };
  }

  window.addEventListener('error', (e) => {
    send('error', [e.error || `${e.message} (${e.filename}:${e.lineno})`]);
  });
  window.addEventListener('unhandledrejection', (e) => {
    send('error', ['処理されなかったPromiseのエラー:', e.reason]);
  });
})();
