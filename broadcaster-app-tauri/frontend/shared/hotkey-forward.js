'use strict';
/**
 * ReaCastの画面(コントロールパネル・コメント読み上げ・字幕の設定)が前面にある時の
 * ホットキー。
 *
 * ReaCastを管理者として起動していると、画面を表示しているWebView2は普通の権限で
 * 動くため、ReaCastの画面が前面にある間は、キーボードの見張り(comments/hotkeys.rs)に
 * キーが届かない(2026-10-20に確認)。前面にある画面には必ずキーが届くので、押された
 * キーをReaCastに渡して、ホットキーの割り当てと照合してもらう(同じキーを見張りでも
 * 拾った時は、Rust側で1回にまとめる)。
 */
(() => {
  const invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  if (!invoke) return;
  // 修飾キーだけを押した時は送らない
  const MODIFIERS = new Set(['ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'MetaLeft', 'MetaRight']);
  document.addEventListener(
    'keydown',
    (e) => {
      if (e.repeat || !e.code || MODIFIERS.has(e.code)) return;
      invoke('comments_hotkey_from_page', {
        code: e.code,
        ctrl: e.ctrlKey,
        alt: e.altKey,
        shift: e.shiftKey,
        meta: e.metaKey,
      }).catch(() => {});
    },
    true
  );
})();
