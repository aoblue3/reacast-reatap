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
 *
 * 並び順について: 種類を問わず追加した順に並んでいて見づらい、という指摘を受け、
 * 「表情・リアクション系(その他全般)」「食べ物系」「動物・生き物系」「短文系」
 * 「マイナス系」の5つの属性ごとにまとめてある。この並び替えはReaCast側の各種
 * 一覧(コントロールパネルのテスト送信ボタン・受け付けるリアクション・連打で
 * 大きくしないリアクション)の表示順にそのまま反映される。ReaTap側の表示順は
 * これとは別に、視聴者アプリの「配信者一覧」画面で保存されるreactionOrder
 * (main.js参照)で管理されており、既存ユーザーが保存済みのreactionOrderには
 * この並び替えの影響は一切出ない(reactionOrderに既に含まれているIDの並びが
 * 常に優先されるため。新規追加のリアクションだけが末尾に追記される)。
 */
const EMOJI_SET = [
  // ==================== 表情・リアクション系(その他全般) ====================
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
  { id: 'nice', char: '👍', label: 'ナイス', anim: 'thumbsup' },
  { id: 'scary', char: '😱', label: 'ヒヤッと', anim: 'jumpscare' },
  { id: 'ganbare', char: '💪', label: 'がんばれ', anim: 'flex' },
  { id: 'sleepy', char: '😴', label: '眠い', anim: 'doze' },
  { id: 'headshot', char: '🎯', label: '的中', anim: 'bullseye' },
  { id: 'zany', char: '🤪', label: 'おどけ', anim: 'zany' },
  { id: 'think', char: '🤔', label: '考え中', anim: 'think' },
  { id: 'ok', char: '🆗', label: 'OK', anim: 'ok' },
  // ⏱(U+23F1)も⬆⬇➡⬅と同じく異体字セレクタ無しだとテキスト表示(黒い文字)に
  // フォールバックしうる絵文字のため、同様にU+FE0Fを付けておく。
  { id: 'stopwatch', char: '⏱️', label: '計測中', anim: 'tick' },
  { id: 'clock5', char: '🕔', label: '時間', anim: 'tick' },
  { id: 'arrowup', char: '⬆️', label: '↑', anim: 'arrowUp' },
  { id: 'arrowdown', char: '⬇️', label: '↓', anim: 'arrowDown' },
  { id: 'arrowright', char: '➡️', label: '→', anim: 'arrowRight' },
  { id: 'arrowleft', char: '⬅️', label: '←', anim: 'arrowLeft' },
  { id: 'bomb', char: '💣', label: '爆弾', anim: 'bomb' },
  { id: 'firecracker', char: '🧨', label: '爆竹', anim: 'firecracker' },
  { id: 'soccer', char: '⚽', label: 'サッカー', anim: 'soccer' },
  { id: 'fist', char: '👊', label: '拳', anim: 'fistbump' },
  { id: 'fireworks', char: '🎆', label: '花火', anim: 'fireworks' },

  // ==================== 食べ物系 ====================
  { id: 'banana', char: '🍌', label: 'バナナ', anim: 'banana' },
  { id: 'mango', char: '🥭', label: 'マンゴー', anim: 'mango' },
  { id: 'peach', char: '🍑', label: 'もも', anim: 'peach' },
  { id: 'cheers', char: '🍻', label: '乾杯', anim: 'cheers' },
  { id: 'ramen', char: '🍜', label: 'ラーメン', anim: 'ramen' },
  { id: 'sushi', char: '🍣', label: '寿司', anim: 'sushi' },
  { id: 'cake', char: '🎂', label: 'おめでとう', anim: 'cake' },

  // ==================== 動物・生き物系 ====================
  { id: 'frog', char: '🐸', label: 'カエル', anim: 'frog' },
  { id: 'baby', char: '👶', label: '赤ちゃん', anim: 'crawl' },
  { id: 'penguin', char: '🐧', label: 'ペンギン', anim: 'waddle' },
  { id: 'dog', char: '🐶', label: 'ワンワン', anim: 'bark' },
  { id: 'pig', char: '🐷', label: 'ブーブー', anim: 'oink' },

  // ==================== 短文系(絵文字ではなく文字として色付き表示する。
  // ニコニコ動画のコメントをイメージし、右から左に流れて消えるnicoflow
  // アニメーションを共通で使う。overlay.jsのANIM_PRESETS.nicoflow参照。
  // 神ｗだけは特別扱いで、colorの代わりにrainbow:trueを立てて虹色が流れ
  // 続ける「ゲーミングカラー」にする(overlay.js/bar.jsのrainbow分岐参照)。
  // 他はすべて白文字で統一。 ====================
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
  { id: 'sorena', char: 'それなｗ', label: 'それなｗ', anim: 'nicoflow', color: '#ffffff' },
  { id: 'wakaru', char: 'わかるｗ', label: 'わかるｗ', anim: 'nicoflow', color: '#ffffff' },
  { id: 'umai', char: 'うまいｗ', label: 'うまいｗ', anim: 'nicoflow', color: '#ffffff' },
  { id: 'kami', char: '神ｗ', label: '神ｗ', anim: 'nicoflow', rainbow: true },
  // 乙(おつかれ)系。押すたびにこの中からランダムで1フレーズを表示する
  // (kusaと同じrandomChars方式)。ボタン自体(ReaTap)には常にcharの
  // 「乙ー」だけが表示され、実際にオーバーレイに流れる文言だけがランダムになる。
  {
    id: 'otsu',
    char: '乙ー',
    randomChars: [
      '乙ー',
      'おつかれー',
      'お疲れ様でした！',
      'ほなまた～',
      'おつです',
      '乙でした',
      'おつかれさまー',
      'また見てね',
      'バイバーイ',
    ],
    label: 'おつ(ランダム)',
    anim: 'nicoflow',
    color: '#ffffff',
  },

  // ==================== マイナス系(ReaCast側の「受け付けるリアクション」
  // 設定で既定OFFにしてある。DEFAULT_DISABLED_REACTION_IDS(control.js)参照) ====================
  { id: 'poop', char: '💩', label: 'うんち', anim: 'poop' },
  { id: 'tengu', char: '👺', label: '天狗', anim: 'tengu' },
  { id: 'angry', char: '😡', label: '怒り', anim: 'angry' },
  { id: 'boo', char: '👎', label: 'ブーイング', anim: 'boo' },
  { id: 'provoke', char: '🖕', label: '挑発', anim: 'provoke' },
];

const EMOJI_BY_ID = new Map(EMOJI_SET.map((e) => [e.id, e]));

function isValidEmojiId(id) {
  return EMOJI_BY_ID.has(id);
}

window.EmojiSet = { EMOJI_SET, EMOJI_BY_ID, isValidEmojiId };
