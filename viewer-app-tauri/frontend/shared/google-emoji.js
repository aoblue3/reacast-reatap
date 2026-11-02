/*
 * Googleの絵文字(メイン画面とリアクションバーの両方で読む)。
 *
 * 設定の「Googleの絵文字で表示する」がONで、フォントをダウンロード済みの時だけ、
 * ダウンロードしたNoto Color Emoji(src-tauri/src/emoji_font.rs)をFontFaceにして足し、
 * htmlにgoogle-emojiを付けて絵文字のフォントの一番先にする(--emoji-font)。
 * ON/OFF・ダウンロード・削除は "emoji-font:changed" で、開いている画面全部に届く。
 */
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const FAMILY = 'ReaTap Google Emoji';
  let face = null;

  async function apply(status) {
    const on = !!(status && status.installed && status.on);
    if (on && !face) {
      try {
        const data = await invoke('emoji_font_data');
        face = new FontFace(FAMILY, data);
        await face.load();
        document.fonts.add(face);
      } catch (e) {
        face = null;
        console.warn('Googleの絵文字を読めませんでした', e);
        return;
      }
    }
    if (!on && face) {
      document.fonts.delete(face);
      face = null;
    }
    document.documentElement.classList.toggle('google-emoji', on && !!face);
  }

  invoke('emoji_font_status').then(apply).catch(() => {});
  window.__TAURI__.event.listen('emoji-font:changed', (e) => apply(e.payload));
})();
