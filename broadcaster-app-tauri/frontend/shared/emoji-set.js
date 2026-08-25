'use strict';
/**
 * 登録済み絵文字セット。
 *
 * anim: 配信者アプリのオーバーレイ(broadcaster-app-tauri/frontend/overlay.js)が、
 * そのリアクションに合った専用アニメーションを選ぶためのキー。視聴者アプリ側では
 * 使わないが、2つのアプリでファイルの内容を完全に一致させておくため(overlay.js側の
 * コメント参照)、こちらにも同じ値を持たせてある。
 *
 * color: 絵文字ではなく普通の文字として表示する項目(例: 「草」)専用。実際の
 * 絵文字(😂等)は常に自前の色で描画されCSSのcolorを無視するため、これが必要になる
 * のは「草」のような特殊な項目だけ。
 *
 * randomChars: 表示するたびにこの配列からランダムで1文字(1項目)を選んで
 * 表示する。指定が無ければ常にcharを使う(例: 「草」はランダムで「草」と「w」を
 * 混ぜたいのでこちらを使う)。
 */
const EMOJI_SET = [
  { id: 'lol', char: '😂', label: '笑', anim: 'laugh' },
  { id: 'clap', char: '👏', label: '拍手', anim: 'clap' },
  // ❤️(U+2764+VS16)がフォント環境によって黒く表示されてしまう問題があったため、
  // 単体で色付き絵文字として安定して描画される💖(スパークリングハート)に変更。
  { id: 'heart', char: '💖', label: 'ハート', anim: 'heartbeat' },
  { id: 'shock', char: '😮', label: '驚き', anim: 'pop' },
  { id: 'fire', char: '🔥', label: '熱い', anim: 'flicker' },
  { id: 'cry', char: '😭', label: '泣', anim: 'sob' },
  { id: 'gg', char: '🎉', label: 'GG', anim: 'confetti' },
  { id: 'skull', char: '💀', label: '死んだ', anim: 'faint' },
  { id: 'please', char: '🥺', label: 'お願い', anim: 'plead' },
  // randomChars: 表示のたびにこの中からランダムで1つを選ぶ(草筆記体の「草」と
  // 半角「w」を両方混ぜてほしいという要望のため)。指定が無ければ従来通りcharを使う。
  {
    id: 'kusa',
    char: '草',
    randomChars: ['草', 'w'],
    label: '草(w)',
    anim: 'grass',
    color: '#5ad65a',
  },
  { id: 'nice', char: '👍', label: 'ナイス', anim: 'thumbsup' },
  { id: 'scary', char: '😱', label: 'ヒヤッと', anim: 'jumpscare' },
  { id: 'ganbare', char: '💪', label: 'がんばれ', anim: 'flex' },
  { id: 'sleepy', char: '😴', label: '眠い', anim: 'doze' },
  { id: 'headshot', char: '🎯', label: '的中', anim: 'bullseye' },
  // ---- ここから追加分 ----
  { id: 'zany', char: '🤪', label: 'おどけ', anim: 'zany' },
  { id: 'think', char: '🤔', label: '考え中', anim: 'think' },
  { id: 'ok', char: '🆗', label: 'OK', anim: 'ok' },
  { id: 'banana', char: '🍌', label: 'バナナ', anim: 'banana' },
  { id: 'frog', char: '🐸', label: 'カエル', anim: 'frog' },
  { id: 'mango', char: '🥭', label: 'マンゴー', anim: 'mango' },
  { id: 'peach', char: '🍑', label: 'もも', anim: 'peach' },
  { id: 'cheers', char: '🍻', label: '乾杯', anim: 'cheers' },
  // ⏱(U+23F1)も⬆⬇➡⬅と同じく異体字セレクタ無しだとテキスト表示(黒い文字)に
  // フォールバックしうる絵文字のため、同様にU+FE0Fを付けておく。
  { id: 'stopwatch', char: '⏱️', label: '計測中', anim: 'tick' },
  { id: 'clock5', char: '🕔', label: '時間', anim: 'tick' },
  { id: 'arrowup', char: '⬆️', label: '↑', anim: 'arrowUp' },
  { id: 'arrowdown', char: '⬇️', label: '↓', anim: 'arrowDown' },
  { id: 'arrowright', char: '➡️', label: '→', anim: 'arrowRight' },
  { id: 'arrowleft', char: '⬅️', label: '←', anim: 'arrowLeft' },

  // ---- ここからネガティブリアクション(ReaCast側の「受け付けるリアクション」
  // 設定で既定OFFにしてある。DEFAULT_DISABLED_REACTION_IDS(control.js)参照) ----
  { id: 'poop', char: '💩', label: 'うんち', anim: 'poop' },
  { id: 'tengu', char: '👺', label: '天狗', anim: 'tengu' },
  { id: 'angry', char: '😡', label: '怒り', anim: 'angry' },
  { id: 'boo', char: '👎', label: 'ブーイング', anim: 'boo' },
  { id: 'provoke', char: '🖕', label: '挑発', anim: 'provoke' },

  // ---- ここから新規ポジティブリアクション(既定ON) ----
  { id: 'bomb', char: '💣', label: '爆弾', anim: 'bomb' },
  { id: 'firecracker', char: '🧨', label: '爆竹', anim: 'firecracker' },
  { id: 'ramen', char: '🍜', label: 'ラーメン', anim: 'ramen' },
  { id: 'sushi', char: '🍣', label: '寿司', anim: 'sushi' },
  { id: 'cake', char: '🎂', label: 'おめでとう', anim: 'cake' },
  { id: 'soccer', char: '⚽', label: 'サッカー', anim: 'soccer' },

  // ---- ここから短文リアクション(colorはkusaと同じく「絵文字ではなく
  // 文字として色付き表示する」既存の仕組みを流用。ニコニコ動画のコメントを
  // イメージし、右から左に流れて消えるnicoflowアニメーションを共通で使う。
  // overlay.jsのANIM_PRESETS.nicoflow参照) ----
  { id: 'sorena', char: 'それな', label: 'それな', anim: 'nicoflow', color: '#ffffff' },
  { id: 'wakaru', char: 'わかる', label: 'わかる', anim: 'nicoflow', color: '#6fe3ff' },
  { id: 'umai', char: 'うまい', label: 'うまい', anim: 'nicoflow', color: '#ffd93d' },
  { id: 'kami', char: '神', label: '神', anim: 'nicoflow', color: '#ff7ad9' },

  // ---- ここから追加の動物リアクション ----
  { id: 'baby', char: '👶', label: '赤ちゃん', anim: 'crawl' },
  { id: 'penguin', char: '🐧', label: 'ペンギン', anim: 'waddle' },
  { id: 'dog', char: '🐶', label: 'ワンワン', anim: 'bark' },
  { id: 'pig', char: '🐷', label: 'ブーブー', anim: 'oink' },
  { id: 'fist', char: '👊', label: '拳', anim: 'fistbump' },
];

const EMOJI_BY_ID = new Map(EMOJI_SET.map((e) => [e.id, e]));

function isValidEmojiId(id) {
  return EMOJI_BY_ID.has(id);
}

window.EmojiSet = { EMOJI_SET, EMOJI_BY_ID, isValidEmojiId };
