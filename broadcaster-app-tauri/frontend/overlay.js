'use strict';
/**
 * メインモニター全面オーバーレイの表示ロジック(Tauri版)。
 *
 * 各リアクションごとに専用のアニメーションを付けてある。役割を分けて実装している:
 *   - .reaction-item(外側のdiv): 画面内での浮遊・ドリフト・フェードをJS側で
 *     transitionを使って動かす(以前からある仕組み)。
 *   - .reaction-glyph(内側のspan): そのリアクションの絵文字/文字そのもの。
 *     「その絵文字らしい」個別のクセ(拍手ならパンパンと弾む、炎ならチラつく、
 *     泣き顔なら震える、等)をCSSの@keyframesアニメーション(overlay.html側に
 *     定義)で表現する。外側と内側で別々の要素のtransformを使うことで、
 *     お互いのアニメーションがぶつからずに同時に効かせられる。
 *   - パーティクル(.particle): 拍手の効果線、ハートの粒、炎の火の粉、涙、
 *     紙吹雪、キラキラ等、リアクションによっては絵文字本体の周りに追加の
 *     小さな装飾を数個まとめて発生させる。
 */
const stage = document.getElementById('stage');

const emojiById = window.EmojiSet.EMOJI_BY_ID;

// Electron版 broadcaster-app/config.js の DEFAULT_OVERLAY_SETTINGS と同じ値。
// baseSizePx/comboSizeStepPxは「絵文字の大きさ」設定(既定100%)からの
// 倍率で毎回計算し直すため、ここでは基準値(BASE_*)として保持する。
const BASE_SIZE_PX = 64;
const BASE_COMBO_STEP_PX = 12;
const settings = {
  maxConcurrent: 40,
  baseSizePx: BASE_SIZE_PX,
  comboSizeStepPx: BASE_COMBO_STEP_PX,
  comboMaxSteps: 6,
  animationDurationMs: 2600,
  // スタンプの透明度(既定1=不透明)。表示中(最大不透明)の状態にだけ効く
  // (フェードイン/アウトの0はそのまま)。
  glyphOpacity: 1,
  // 連打すると絵文字が段階的に大きくなる仕様のON/OFF(既定ON=従来通り)。
  comboGrowthEnabled: true,
};

// 設定パネルの「絵文字の大きさ」で選んだ倍率(既定1=100%)を反映する。
// Tauri本体のオーバーレイと、OBSの「ブラウザ」ソース(obs_bridge.rs)の両方に
// 同じ値が効くようにしたいので、値そのものはRust側(ConfigStore)で一元管理し、
// こちらは渡された値を使って表示サイズを計算し直すだけにする。
function applyGlyphScale(scale) {
  const s = Number.isFinite(scale) && scale > 0 ? Math.min(3, Math.max(0.3, scale)) : 1;
  settings.baseSizePx = BASE_SIZE_PX * s;
  settings.comboSizeStepPx = BASE_COMBO_STEP_PX * s;
}

// 設定パネルの「スタンプの透明度」を反映する(glyphScaleと同じ考え方)。
function applyGlyphOpacity(opacity) {
  settings.glyphOpacity =
    Number.isFinite(opacity) && opacity > 0 ? Math.min(1, Math.max(0.1, opacity)) : 1;
}

// 設定パネルの「連打すると大きくなる」チェックボックスを反映する。
function applyComboGrowth(enabled) {
  settings.comboGrowthEnabled = enabled !== false;
}

/**
 * リアクションの種類(emoji-set.jsのanimフィールド)ごとのアニメーション設定。
 *   motion: 画面上での動きの種類(下のcomputeMotionPath参照)。省略時は'rise'
 *     (下から出てきて上に上がっていく、既定の動き)。
 *   riseFactor: motionが使う「移動距離」の倍率(1で標準、小さいほど動きが小さい。
 *     riseという名前だが、motionが'fall'/'collapse'の場合は下方向の距離に使う)
 *   driftFactor: 移動方向に対して直角の「揺れ」の倍率
 *   durationFactor: 全体の表示時間の倍率(ゆっくり漂わせたいものは長めに)
 *   particles: 追加で発生させる装飾パーティクルの指定(無ければ何も出さない)
 */
const ANIM_PRESETS = {
  laugh: { motion: 'rise', riseFactor: 1, durationFactor: 1.0 },
  clap: {
    motion: 'rise',
    riseFactor: 0.9,
    durationFactor: 0.9,
    particles: { shape: 'streak', count: 6, spread: 42, life: 550, color: 'rgba(255,248,214,0.9)' },
  },
  heartbeat: {
    motion: 'rise',
    riseFactor: 1.1,
    durationFactor: 1.15,
    particles: { shape: 'glyph', glyphs: ['💕', '💗', '❤️'], count: 4, spread: 34, life: 1300, drift: 'up' },
  },
  // 😮 驚き: 画面端から勢いよく飛び込んでくる(不意打ちで驚かせるイメージ)
  pop: {
    motion: 'sideIn',
    riseFactor: 0.85,
    durationFactor: 0.9,
    particles: { shape: 'ring', life: 500 },
  },
  flicker: {
    motion: 'rise',
    riseFactor: 1.15,
    durationFactor: 0.95,
    particles: { shape: 'dot', color: '#ffb347', count: 7, spread: 30, life: 900, drift: 'up' },
  },
  // 😭 泣: 上ではなく、しゃくり上げながら下に沈んでいく
  sob: {
    motion: 'fall',
    riseFactor: 0.7,
    durationFactor: 1.0,
    particles: { shape: 'glyph', glyphs: ['💧'], count: 3, spread: 20, life: 900, drift: 'down' },
  },
  // 🎉 GG: 左右に大きくジグザグに揺れながら勢いよく上昇する、一番躍動感のある動き。
  // 絵文字のデザイン上いつも同じ向き(右向き)になってしまうので、randomFlipで
  // 半分くらいの確率で左右反転させ、単調にならないようにする。
  confetti: {
    motion: 'zigzagRise',
    riseFactor: 1.0,
    driftFactor: 1.2,
    durationFactor: 1.05,
    randomFlip: true,
    particles: { shape: 'confetti', count: 16, life: 1100, drift: 'burst' },
  },
  // 💀 死んだ: その場でくずおれるように少しだけ沈む(上がらない)
  faint: { motion: 'collapse', riseFactor: 0.5, durationFactor: 1.1, noOuterRotate: true },
  // 🥺 お願い: キラキラの粒は不要とのことなので削除(動き自体はそのまま)
  plead: {
    motion: 'rise',
    riseFactor: 0.9,
    durationFactor: 1.3,
  },
  // 草: 地面から生えてくるイメージなので、下から上に伸びる動きのまま
  grass: { motion: 'rise', riseFactor: 0.95, durationFactor: 1.1 },
  thumbsup: {
    motion: 'rise',
    riseFactor: 1.05,
    durationFactor: 0.9,
    particles: { shape: 'glyph', glyphs: ['⭐'], count: 4, spread: 32, life: 900, drift: 'up' },
  },
  // 😱 ヒヤッと: 大写しでいきなり出現してから通常サイズに落ち着く「ジャンプスケア」
  jumpscare: { motion: 'burstZoom', riseFactor: 1, durationFactor: 0.95 },
  flex: {
    motion: 'rise',
    riseFactor: 1,
    durationFactor: 1.05,
    particles: { shape: 'streak', count: 3, spread: 26, life: 400, color: 'rgba(255,255,255,0.7)' },
  },
  // 😴 眠い: 舟をこぐようにゆっくり下に沈んでいく(💤は上に浮かぶ対比を付ける)
  doze: {
    motion: 'fall',
    riseFactor: 0.4,
    durationFactor: 1.6,
    particles: { shape: 'glyph', glyphs: ['💤'], count: 2, spread: 18, life: 1700, drift: 'up' },
  },
  // 🎯 的中: ダーツのように画面端から一直線に飛んできて命中する
  bullseye: {
    motion: 'sideIn',
    riseFactor: 0.85,
    durationFactor: 0.75,
    particles: { shape: 'ring', life: 350, small: true },
  },

  // ---- ここから追加分 ----

  // 🤪 おどけ: 千鳥足のようにグラグラ揺れながら上がる
  zany: { motion: 'rise', riseFactor: 0.9, durationFactor: 1.0 },
  // 🤔 考え中: あまり動かず、ゆっくり浮かんで消える。吹き出し(💭)が上に漂う
  think: {
    motion: 'rise',
    riseFactor: 0.6,
    durationFactor: 1.5,
    particles: { shape: 'glyph', glyphs: ['💭'], count: 2, spread: 22, life: 1300, drift: 'up' },
  },
  // 🆗 OK: その場にハンコを押すようにドンと出て、あまり動かず消える
  ok: {
    motion: 'collapse',
    riseFactor: 0.3,
    durationFactor: 0.85,
    noOuterRotate: true,
    glyphAnim: 'stamp',
    particles: { shape: 'ring', life: 380, small: true },
  },
  // 🍌 バナナ: くるくる回転しながら上がる
  banana: { motion: 'rise', riseFactor: 1.0, durationFactor: 1.0, glyphAnim: 'spin' },
  // 🐸 カエル: ぴょんぴょん跳ねながら画面を横切る専用モーション(下のcomputeMotionPath参照)
  frog: { motion: 'hop', riseFactor: 1, durationFactor: 1.1, glyphAnim: 'hop' },
  // 🥭🍑 フルーツ: 弾むように軽くバウンドしながら上がる(共通)
  mango: { motion: 'rise', riseFactor: 0.85, durationFactor: 1.0, glyphAnim: 'bounce' },
  peach: { motion: 'rise', riseFactor: 0.85, durationFactor: 1.0, glyphAnim: 'bounce' },
  // 🍻 乾杯: 登場時にグラスをぶつけるようにカチンと揺れ、泡(粒)が上がる
  cheers: {
    motion: 'rise',
    riseFactor: 0.95,
    durationFactor: 1.0,
    particles: { shape: 'dot', color: '#ffe08a', count: 5, spread: 26, life: 800, drift: 'up' },
  },
  // ⏱🕔 時間系: 針が行ったり来たりするように小さく振れながらゆっくり上がる(共通)
  tick: { motion: 'rise', riseFactor: 0.7, durationFactor: 1.2, glyphAnim: 'tick' },
  // ⬆⬇➡⬅ 矢印: 見たままの方向にまっすぐ飛ぶ。グリフ自体もその方向へ一瞬伸びる
  // 「ダッシュ」演出(glyphAnim:'dash'で共有)を付ける。
  arrowUp: {
    motion: 'rise',
    riseFactor: 1.3,
    driftFactor: 0,
    durationFactor: 0.8,
    glyphAnim: 'dash',
    particles: { shape: 'streak', count: 3, spread: 14, life: 350, color: 'rgba(255,255,255,0.6)' },
  },
  arrowDown: {
    motion: 'fall',
    riseFactor: 1.3,
    driftFactor: 0,
    durationFactor: 0.8,
    glyphAnim: 'dash',
  },
  arrowRight: {
    motion: 'sideIn',
    forceFromLeft: true,
    riseFactor: 1,
    durationFactor: 0.8,
    glyphAnim: 'dash',
  },
  arrowLeft: {
    motion: 'sideIn',
    forceFromLeft: false,
    riseFactor: 1,
    durationFactor: 0.8,
    glyphAnim: 'dash',
  },

  // ---- ここから追加分(ネガティブ・ポジティブ新規リアクション) ----

  // 💩: 上から落ちてきて、最後に着地でバウンドする(fallBounceモーション。
  // computeMotionPath参照)
  poop: {
    motion: 'fallBounce',
    riseFactor: 1,
    durationFactor: 1.15,
    noOuterRotate: true,
  },
  // 👺: 左から右へ勢いよく進む。到着した瞬間に、鼻の先あたりから右方向へ
  // レーザー光線のようなビームを1本出す(particleOffsetX/Yで発生位置を
  // 絵文字の中心から「鼻先」寄りにずらしている)。
  tengu: {
    motion: 'sideIn',
    forceFromLeft: true,
    riseFactor: 1,
    durationFactor: 0.85,
    glyphAnim: 'dash',
    particleOffsetX: 0.4,
    particleOffsetY: -0.1,
    particles: { shape: 'laser', life: 420, thickness: 5, color: 'rgba(255,60,60,0.95)' },
  },
  // 😡: プルプルと震えながら怒りの湯気(パーティクル)を上げる
  angry: {
    motion: 'rise',
    riseFactor: 0.85,
    durationFactor: 0.95,
    glyphAnim: 'angry',
    particles: { shape: 'dot', color: '#ff5252', count: 4, spread: 20, life: 550, drift: 'up' },
  },
  // 👎: ブーイングしながら首を振るように下へ落ちていく
  boo: {
    motion: 'fall',
    riseFactor: 0.8,
    durationFactor: 1.0,
    glyphAnim: 'boo',
  },
  // 🖕: 下から挑発的に上がってくる(💪と同じ「決めポーズ」感の自己アニメーションを流用)
  provoke: {
    motion: 'rise',
    riseFactor: 1.1,
    durationFactor: 0.9,
    glyphAnim: 'flex',
  },
  // 💣: 落下して着地したあと、2〜3秒待ってからランダムなタイミングで爆発する。
  // 待ち時間・爆発演出はspawnReaction側の専用処理(preset.explodeDelayRangeMs)で扱う。
  bomb: {
    motion: 'quickFall',
    riseFactor: 1.6,
    durationFactor: 2.6,
    noOuterRotate: true,
    explodeDelayRangeMs: [2000, 3000],
    particles: { shape: 'ring', life: 500 },
  },
  // 🧨: 爆竹らしく、画面下の方から左右どちらかランダムな斜め上方向へ跳ねる
  // ように急上昇する(popUpモーション。computeMotionPath参照)。跳ね上がった
  // 先(頂点)で火花(パーティクル)を散らす。
  firecracker: {
    motion: 'popUp',
    riseFactor: 1,
    durationFactor: 1.05,
    noOuterRotate: true,
    glyphAnim: 'firecracker',
    particles: { shape: 'dot', color: '#ffb347', count: 8, spread: 32, life: 500, drift: 'burst' },
  },
  // 🍜: ふわふわ左右に揺れながら上がっていく。湯気に見立てた薄い粒を添える
  ramen: {
    motion: 'rise',
    riseFactor: 0.9,
    driftFactor: 1.7,
    durationFactor: 1.35,
    glyphAnim: 'sway',
    particles: { shape: 'dot', color: 'rgba(255,255,255,0.55)', count: 4, spread: 16, life: 1100, drift: 'up' },
  },
  // 🍣: 勢いよく登場してその場にどんと構える(🆗のスタンプ演出違いバージョン)。
  // fullRangeY: 画面下部だけでなく、画面のどこにでも現れるようにする。
  sushi: {
    motion: 'collapse',
    riseFactor: 0.3,
    durationFactor: 0.95,
    noOuterRotate: true,
    fullRangeY: true,
    glyphAnim: 'punch',
  },
  // 🎂: 紙吹雪と一緒に楽しげに弾みながら上がっていく、お祝いムード
  cake: {
    motion: 'rise',
    riseFactor: 1.0,
    durationFactor: 1.15,
    glyphAnim: 'bounce',
    particles: { shape: 'confetti', count: 14, life: 1000 },
  },
  // ⚽: 上下左右のどこかから飛んできて、画面内で数回跳ね返ってから止まる
  // (ballBounceモーション。computeMotionPath参照)
  soccer: {
    motion: 'ballBounce',
    riseFactor: 1,
    durationFactor: 1.2,
    glyphAnim: 'spin',
  },
};

const CONFETTI_COLORS = ['#ff6b6b', '#ffd93d', '#6bcfef', '#a06bff', '#6bff9e', '#ff9e6b'];
const DRIFT_ANIM = {
  up: 'particle-up',
  down: 'particle-down',
  burst: 'particle-burst',
  twinkle: 'particle-twinkle',
};

const activeItems = [];
const comboState = new Map();
const COMBO_WINDOW_MS = 1500;

// このページは2つの方法で開かれうる:
//   1. Tauri本体の透明オーバーレイウィンドウとして(window.__TAURI__が使える)
//      → 従来通りTauriのイベント経由でリアクションを受け取る
//   2. OBSの「ブラウザ」ソースとして、ただのWebページとして開かれた場合
//      (window.__TAURI__は存在しない) → 配信者アプリが同じPC上でローカルに
//      立てているWebSocketブリッジ(obs_bridge.rs参照)に直接繋いで受け取る。
//      OBSの通常のウィンドウキャプチャだと、このアプリの透明ウィンドウの
//      透明部分がうまく抜けずデスクトップが映り込んでしまう実機不具合が
//      あったため、この方式を用意している。
if (window.__TAURI__ && window.__TAURI__.event) {
  window.__TAURI__.event.listen('overlay:reaction', (event) => {
    spawnReaction(event.payload.emoji);
  });
  // 設定パネルの「絵文字の大きさ」を起動時に反映し、設定パネルで変更される
  // たびにも(このウィンドウを閉じ直さなくても)即座に反映されるようにする。
  window.__TAURI__.event.listen('overlay:settings', (event) => {
    const payload = event.payload || {};
    if (typeof payload.glyphScale === 'number') applyGlyphScale(payload.glyphScale);
    if (typeof payload.glyphOpacity === 'number') applyGlyphOpacity(payload.glyphOpacity);
    if (typeof payload.comboGrowthEnabled === 'boolean') applyComboGrowth(payload.comboGrowthEnabled);
  });
  window.__TAURI__.core
    .invoke('get_overlay_settings')
    .then((s) => {
      applyGlyphScale(s.glyphScale);
      applyGlyphOpacity(s.glyphOpacity);
      applyComboGrowth(s.comboGrowthEnabled);
    })
    .catch(() => {});
} else {
  const OBS_BRIDGE_WS_PORT = 18771;
  connectObsBridge(OBS_BRIDGE_WS_PORT);
}

function connectObsBridge(port) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}`);
  ws.addEventListener('message', (event) => {
    try {
      const data = JSON.parse(event.data);
      if (!data) return;
      // 配信者アプリ側で設定を変更した時にも同じ内容が届く(obs_bridge.rs参照)。
      // OBS側はTauriのinvokeが使えないので、この経路が唯一の反映手段になる。
      if (data.type === 'settings') {
        if (typeof data.glyphScale === 'number') applyGlyphScale(data.glyphScale);
        if (typeof data.glyphOpacity === 'number') applyGlyphOpacity(data.glyphOpacity);
        if (typeof data.comboGrowthEnabled === 'boolean') applyComboGrowth(data.comboGrowthEnabled);
        return;
      }
      if (typeof data.emoji === 'string') spawnReaction(data.emoji);
    } catch {
      // 想定外のメッセージは無視する
    }
  });
  // 配信者アプリの再起動などで切れても、自動で繋ぎ直しを試み続ける
  ws.addEventListener('close', () => setTimeout(() => connectObsBridge(port), 1500));
  ws.addEventListener('error', () => ws.close());
}

function getComboLevel(emojiId) {
  const now = Date.now();
  const prev = comboState.get(emojiId);
  let count = 1;
  if (prev && now - prev.lastTs <= COMBO_WINDOW_MS) {
    count = Math.min(prev.count + 1, settings.comboMaxSteps);
  }
  comboState.set(emojiId, { count, lastTs: now });
  return count;
}

/**
 * 動きの種類ごとに、開始位置・初期transform・最終transform(・必要なら途中経過の
 * ウェイポイント)を計算する。「下から上に上がる」一辺倒だったのを、リアクションの
 * 性格に合わせて上から降ってくる・横から飛び込んでくる・ジグザグに揺れながら
 * 上がる・その場で急に大写しになる、といったバリエーションを持たせるための関数。
 *
 * 返り値:
 *   startX/startY: 開始位置(el.style.left/top)
 *   initialTransform/finalTransform: 開始/終了時点のtransform
 *   moveDuration: このtransition(transform)にかける時間(msの表示時間全体とは
 *     別に、動き自体は短めに終わらせたい場合(ジグザグの各区間、バーストの出現等)
 *     に使う)
 *   easing: transitionのeasing関数
 *   waypoints: [{atMs, transform}, ...] 途中でtransformを更新するタイミング
 *     (ジグザグのような多段階の動きに使う。無ければ2点間の単純な動き)
 *   endX/endY: 最終的なだいたいの位置(パーティクルの発生位置の計算に使う)
 */
// OBSのブラウザソースを小さめのサイズで使っている場合等、比例計算のままだと
// 上端・下端を突き抜けてから(画面外で)フェードが終わる=「早々に見切れて
// 消えたように見える」という指摘があったための安全マージン。上下それぞれ、
// 表示領域の8%は必ず残して、フェードアウトが終わるまで画面内に留まるようにする。
const EDGE_MARGIN_RATIO = 0.08;

function clampRiseDistance(startY, riseDistance, vh) {
  const maxRise = Math.max(0, startY - vh * EDGE_MARGIN_RATIO);
  return Math.min(riseDistance, maxRise);
}

function clampFallDistance(startY, fallDistance, vh, size) {
  const maxFall = Math.max(0, vh * (1 - EDGE_MARGIN_RATIO) - size - startY);
  return Math.min(fallDistance, maxFall);
}

// 'rise'/'fall'/'zigzagRise'以外の動きにも、それぞれの見切れ方に合わせた
// 安全マージンを追加する(ディスプレイ拡大表示やOBSブラウザソースを小さめに
// 設定している環境で、フェードし終わる前に画面端へ到達してしまう指摘への対応)。

// sideIn(画面端から登場): 到着位置が反対側の端まで行き過ぎないようにする。
// 開始位置は完全に画面外(offscreen)なので、travel(移動距離)が
// vw*(1-EDGE_MARGIN_RATIO)を超えないようにするだけで、到着側の端に十分な
// 余白が残る(オフスクリーン開始位置のサイズ分は式の中で自然に相殺される)。
function clampSideTravel(travel, vw) {
  return Math.min(travel, vw * (1 - EDGE_MARGIN_RATIO));
}

// burstZoom(大写しで出現): 開始時点でscale(2.4)まで拡大されるため、通常の
// サイズ(size)基準でランダムな開始位置を選ぶと、拡大された見た目が画面端を
// はみ出してしまうことがある。開始位置の抽選範囲に、拡大時の余分な広がり
// (overhang)を差し引いた余白を確保しておく。
function clampBurstZoomStart(vw, vh, size, maxScale) {
  const overhang = (size * (maxScale - 1)) / 2;
  const marginX = vw * EDGE_MARGIN_RATIO + overhang;
  const marginY = vh * EDGE_MARGIN_RATIO + overhang;
  const rangeX = Math.max(0, vw - marginX * 2 - size);
  const rangeY = Math.max(0, vh * 0.5 - overhang * 2);
  return {
    startX: marginX + Math.random() * rangeX,
    startY: Math.max(vh * 0.2, marginY) + Math.random() * rangeY,
  };
}

// hop(カエルが跳ねる): 跳ねるたびに一時的に本来の位置より高く上がる
// (waypointsのピーク)ので、跳躍を重ねるうちに累積した最高到達点が画面上端の
// 余白を超えないよう、跳躍の高さそのものを開始位置から逆算して制限する。
function clampHopHeight(startY, hopHeight, netRisePerHop, hopCount, vh) {
  // 最後の跳躍のピーク時が最も高く上がる瞬間: 直前までの正味上昇分
  // ((hopCount-1)回分)+ そのジャンプ自体の高さ、が画面上端の余白を
  // 超えないようにする。
  const priorNetRise = netRisePerHop * (hopCount - 1);
  const maxHopHeight = Math.max(20, startY - vh * EDGE_MARGIN_RATIO - priorNetRise);
  return Math.min(hopHeight, maxHopHeight);
}

function computeMotionPath(motion, preset, size, vw, vh, duration) {
  const driftFactor = preset.driftFactor ?? 1;
  const riseFactor = preset.riseFactor ?? 1;
  const rotate = preset.noOuterRotate ? 0 : (Math.random() - 0.5) * 60;

  if (motion === 'fall') {
    // 画面上部に出現し、ゆっくり下へ沈んでいく(泣き顔がうなだれる、眠気で船を
    // こぐように下がっていく、等)
    const startX = Math.random() * (vw - size);
    const startY = Math.random() * (vh * 0.2);
    const drift = (Math.random() - 0.5) * 200 * driftFactor;
    const fallDistance = clampFallDistance(startY, (vh * 0.55 + size) * riseFactor, vh, size);
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.6)',
      finalTransform: `translate(${drift}px, ${fallDistance}px) rotate(${rotate}deg) scale(1)`,
      moveDuration: duration,
      easing: 'cubic-bezier(.17,.67,.35,1)',
      endX: startX + drift,
      endY: startY + fallDistance,
    };
  }

  if (motion === 'sideIn') {
    // 画面端から勢いよく登場し、画面内へ流れ込む(驚き=不意打ちで視界に入って
    // くる、的中=ダーツのように命中する、を表現)。左右どちらから出すかは通常
    // ランダムだが、矢印(➡⬅)のように向きそのものに意味がある場合は
    // preset.forceFromLeftで固定できる(true=左から右へ、false=右から左へ)。
    const fromLeft = preset.forceFromLeft ?? Math.random() < 0.5;
    const startY = vh * 0.15 + Math.random() * (vh * 0.55);
    const startX = fromLeft ? -size : vw;
    const travel = clampSideTravel((vw * 0.55 + size) * driftFactor, vw);
    const endDx = fromLeft ? travel : -travel;
    const bob = (Math.random() - 0.5) * 80;
    return {
      startX,
      startY,
      initialTransform: `translate(0, 0) rotate(${fromLeft ? -20 : 20}deg) scale(0.7)`,
      finalTransform: `translate(${endDx}px, ${bob}px) rotate(${rotate}deg) scale(1)`,
      moveDuration: Math.round(duration * 0.85),
      easing: 'cubic-bezier(.22,.61,.36,1)',
      endX: startX + endDx,
      endY: startY + bob,
    };
  }

  if (motion === 'zigzagRise') {
    // 上昇しながら左右に大きく揺れる、一番躍動感のある動き(GG向け)。
    // 単純な2点間の動きではなく、waypointsで3段階の経由地点を刻んで
    // ジグザグに見せる(最後の要素が最終的な着地姿勢)。
    const startX = vw * 0.5 - size / 2 + (Math.random() - 0.5) * vw * 0.3;
    const startY = vh - size - Math.random() * (vh * 0.2);
    const riseDistance = clampRiseDistance(startY, (vh * 0.62 + size) * riseFactor, vh);
    const swing = 100 * driftFactor;
    const legDuration = Math.round(duration * 0.25);
    return {
      startX,
      startY,
      // waypoints指定時はこのinitialTransformが最初の見た目になる(rAFでwaypoints[0]に
      // 差し替わるまでの一瞬の初期値)。opacityが0の間なので実際には見えないが、
      // 他のmotionと同じ「小さく現れて広がる」入り方に揃えておく。
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.6)',
      moveDuration: legDuration,
      easing: 'cubic-bezier(.34,1.56,.64,1)',
      waypoints: [
        {
          atMs: 0,
          transform: `translate(${swing}px, -${riseDistance * 0.35}px) rotate(${rotate * 0.6}deg) scale(1.08)`,
        },
        {
          atMs: legDuration,
          transform: `translate(${-swing}px, -${riseDistance * 0.68}px) rotate(${-rotate * 0.6}deg) scale(0.96)`,
        },
        {
          atMs: legDuration * 2,
          transform: `translate(${swing * 0.4}px, -${riseDistance * 0.9}px) rotate(${rotate * 0.4}deg) scale(1.04)`,
        },
        {
          atMs: legDuration * 3,
          transform: `translate(${swing * 0.3}px, -${riseDistance}px) rotate(${rotate}deg) scale(1)`,
        },
      ],
      endX: startX,
      endY: startY - riseDistance,
    };
  }

  if (motion === 'burstZoom') {
    // 突然大写しで現れ、通常サイズに落ち着く「ジャンプスケア」的な出現(ヒヤッと向け)。
    // 開始時点でscale(2.4)まで拡大されるため、開始位置は拡大後の見た目が
    // 画面端をはみ出さない範囲から選ぶ(clampBurstZoomStart参照)。
    const BURST_ZOOM_MAX_SCALE = 2.4;
    const { startX, startY } = clampBurstZoomStart(vw, vh, size, BURST_ZOOM_MAX_SCALE);
    const jitter = (Math.random() - 0.5) * 40 * driftFactor;
    return {
      startX,
      startY,
      initialTransform: `translate(0, 0) rotate(0deg) scale(${BURST_ZOOM_MAX_SCALE})`,
      finalTransform: `translate(${jitter}px, ${jitter}px) rotate(${rotate * 0.3}deg) scale(1)`,
      moveDuration: Math.round(duration * 0.32),
      easing: 'cubic-bezier(.36,1.7,.5,1)',
      endX: startX + jitter,
      endY: startY + jitter,
    };
  }

  if (motion === 'collapse') {
    // 力尽きてその場に少し崩れ落ちる(スカル向け。グリフ自身は anim-faint で
    // 横に倒れる動きをするので、外側はあまり動かさず沈む距離だけ小さく持たせる)。
    // preset.fullRangeY: 🍣寿司のように「画面下部だけでなくどこにでも現れて
    // ほしい」場合は、開始Y座標を画面の縦方向ほぼ全域からランダムに選ぶ。
    const startX = Math.random() * (vw - size);
    let startY;
    if (preset.fullRangeY) {
      const marginY = vh * EDGE_MARGIN_RATIO;
      startY = marginY + Math.random() * Math.max(1, vh - marginY * 2 - size);
    } else {
      startY = vh - size - Math.random() * (vh * 0.25);
    }
    const sinkDistance = clampFallDistance(startY, 26 * riseFactor, vh, size);
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(1)',
      finalTransform: `translate(0, ${sinkDistance}px) rotate(${rotate}deg) scale(0.86)`,
      moveDuration: duration,
      easing: 'ease-out',
      endX: startX,
      endY: startY + sinkDistance,
    };
  }

  if (motion === 'hop') {
    // 🐸 カエル: ぴょんぴょん跳ねながら画面を左右どちらかへ横切りつつ、
    // 少しずつ上へ移動する専用モーション。waypointsで「跳ねる頂点」と
    // 「着地」を交互に3回刻み、pogoスティックのような動きにする。
    const goRight = Math.random() < 0.5;
    const dir = goRight ? 1 : -1;
    const startX = goRight
      ? size + Math.random() * (vw * 0.15)
      : vw - size - Math.random() * (vw * 0.15);
    const startY = vh - size - Math.random() * (vh * 0.15);
    const hopStepX = vw * 0.12 * (0.7 + Math.random() * 0.5);
    const netRisePerHop = vh * 0.045 * riseFactor;
    const hopCount = 3;
    // 跳躍を重ねるたびに正味では少しずつ上へ移動していくため、最後の跳躍の
    // ピーク時が最も高く上がる瞬間になる。そこが画面上端の余白を超えないよう、
    // 跳ねる高さ自体をstartYから逆算して制限する(clampHopHeight参照)。
    const hopHeight = clampHopHeight(
      startY,
      (vh * 0.14 + size * 0.25) * riseFactor,
      netRisePerHop,
      hopCount,
      vh
    );
    const legDuration = Math.round((duration * 0.85) / (hopCount * 2));
    const waypoints = [];
    let curX = 0;
    let curY = 0;
    for (let hop = 1; hop <= hopCount; hop++) {
      // 跳ねる頂点(高く上がりつつ、進行方向へ動く)
      curX += hopStepX * 0.55 * dir;
      curY -= hopHeight;
      waypoints.push({
        atMs: legDuration * (hop * 2 - 1),
        transform: `translate(${curX}px, ${curY}px) rotate(${dir * -8}deg) scale(1.05, 0.9)`,
      });
      // 着地(正味では少しだけ上に、横にはさらに進む)
      curX += hopStepX * 0.45 * dir;
      curY += hopHeight - netRisePerHop;
      waypoints.push({
        atMs: legDuration * hop * 2,
        transform: `translate(${curX}px, ${curY}px) rotate(0deg) scale(1.08, 0.85)`,
      });
    }
    waypoints.push({
      atMs: legDuration * hopCount * 2 + Math.round(legDuration * 0.4),
      transform: `translate(${curX}px, ${curY}px) rotate(0deg) scale(1)`,
    });
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.6)',
      moveDuration: legDuration,
      easing: 'cubic-bezier(.45,1.4,.4,1)',
      waypoints,
      endX: startX + curX,
      endY: startY + curY,
    };
  }

  if (motion === 'fallBounce') {
    // 💩: 上から落ちてきて、着地の瞬間に2回小さくバウンドしてから止まる。
    //
    // 元の実装では、最初のwaypoint(落下そのもの)がrAFで即座に適用される
    // (下のspawnReaction側の仕様: waypoints[0]はatMsを無視して生成直後に
    // 反映される)にもかかわらず、そのtransition時間には短いlegDuration
    // 1区間分しか使っていなかったため、意図(atMs: legDuration*3という
    // 記述)よりずっと速く落ちてしまっていた。「落ちるスピードが早すぎる」
    // という指摘の原因だったので、落下区間だけ専用の長いtransition時間
    // (fallLegMs)を持たせ、そのぶん後続のバウンド区間のatMsも後ろにずらす。
    // バウンド自体のテンポ(legMs)は従来通り短いまま(そこは速くていい)。
    const startX = Math.random() * (vw - size);
    const startY = Math.random() * (vh * 0.15);
    const groundY = clampFallDistance(startY, (vh * 0.62 + size) * riseFactor, vh, size);
    // 落下区間・バウンド区間それぞれのtransition時間を、全体の表示時間(duration)
    // に対する割合で直接指定する(bounce区間は従来通りテンポよく、落下区間
    // だけ長めに取ることで「半分くらいのスピード」の体感にする)。
    // 合計(fallLegMs + bounceLegMs*4)がフェードアウト開始(duration*0.75)より
    // 十分前に収まるよう按分してある。
    const fallLegMs = Math.round(duration * 0.38);
    const bounceLegMs = Math.round(duration * 0.07);
    const bounce1 = groundY * 0.22;
    const bounce2 = groundY * 0.09;
    const fallAt = fallLegMs;
    const bounce1At = fallAt + bounceLegMs;
    const bounce2At = bounce1At + bounceLegMs;
    const bounce3At = bounce2At + bounceLegMs;
    const settleAt = bounce3At + bounceLegMs;
    const waypoints = [
      { atMs: fallAt, legMs: fallLegMs, transform: `translate(0, ${groundY}px) rotate(${rotate * 0.3}deg) scale(1.12, 0.85)` },
      { atMs: bounce1At, legMs: bounceLegMs, transform: `translate(0, ${groundY - bounce1}px) rotate(${rotate * 0.2}deg) scale(0.94, 1.06)` },
      { atMs: bounce2At, legMs: bounceLegMs, transform: `translate(0, ${groundY}px) rotate(${rotate * 0.15}deg) scale(1.08, 0.92)` },
      { atMs: bounce3At, legMs: bounceLegMs, transform: `translate(0, ${groundY - bounce2}px) rotate(${rotate * 0.05}deg) scale(0.97, 1.03)` },
      { atMs: settleAt, legMs: bounceLegMs, transform: `translate(0, ${groundY}px) rotate(0deg) scale(1)` },
    ];
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.7)',
      moveDuration: fallLegMs,
      easing: 'cubic-bezier(.5,0,.85,1)',
      waypoints,
      endX: startX,
      endY: startY + groundY,
    };
  }

  if (motion === 'quickFall') {
    // 💣: すばやく落下して着地する(その後、しばらく待ってから爆発する演出は
    // spawnReaction側のpreset.explodeDelayRangeMs処理で行う)。
    // 着地する高さは、常に画面下部固定ではなく、開始位置より下・画面内の
    // 余白を除いた範囲からランダムに選ぶ(要望: 着地位置を全範囲ランダムに)。
    const startX = Math.random() * (vw - size);
    const startY = Math.random() * (vh * 0.15);
    const drift = (Math.random() - 0.5) * 60 * driftFactor;
    const marginY = vh * EDGE_MARGIN_RATIO;
    const minLandY = Math.max(startY + size * 0.5, marginY);
    const maxLandY = Math.max(minLandY, vh - marginY - size);
    const landY = minLandY + Math.random() * Math.max(0, maxLandY - minLandY);
    const fallDistance = Math.max(0, landY - startY);
    // 落下距離が短い(高い位置に着地する)場合は短く、長い(画面下の方まで
    // 落ちる)場合は長く、距離に応じてtransition時間を按分する。
    const baselineFall = vh * 0.85;
    const moveDuration = Math.round(
      duration * Math.min(0.34, Math.max(0.12, 0.3 * (fallDistance / baselineFall)))
    );
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.7)',
      finalTransform: `translate(${drift}px, ${fallDistance}px) rotate(${rotate * 0.4}deg) scale(1)`,
      moveDuration,
      easing: 'cubic-bezier(.4,0,.7,1)',
      endX: startX + drift,
      endY: startY + fallDistance,
    };
  }

  if (motion === 'popUp') {
    // 🧨 爆竹: 画面下の方に現れて、左右どちらかランダムな斜め上方向へ、
    // 跳ねるように2段階で急上昇する(爆竹が弾ける勢いのイメージ)。
    const dir = Math.random() < 0.5 ? -1 : 1;
    const startX = Math.min(Math.max(size, Math.random() * (vw - size)), vw - size);
    const startY = vh - size - Math.random() * (vh * 0.12);
    const jumpX = vw * (0.07 + Math.random() * 0.08) * dir;
    const jumpY = clampRiseDistance(startY, (vh * 0.3 + size) * riseFactor, vh);
    const legDuration = Math.round(duration * 0.22);
    const waypoints = [
      {
        atMs: legDuration,
        legMs: legDuration,
        transform: `translate(${jumpX * 0.55}px, -${jumpY * 0.6}px) rotate(${dir * -18}deg) scale(1.08, 0.9)`,
      },
      {
        atMs: legDuration * 2,
        legMs: legDuration,
        transform: `translate(${jumpX}px, -${jumpY}px) rotate(${dir * -8}deg) scale(1)`,
      },
    ];
    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.6)',
      moveDuration: legDuration,
      easing: 'cubic-bezier(.3,1.4,.4,1)',
      waypoints,
      endX: startX + jumpX,
      endY: startY - jumpY,
    };
  }

  if (motion === 'ballBounce') {
    // ⚽: 実際に重力・反発を刻み時間でシミュレートし、画面の上下左右いずれかの
    // 端から飛んできて、床・天井・左右の壁に当たるたびにバウンドしながら
    // 徐々に勢いが弱まって止まる(「もっと物理エンジンが乗っかってるみたいに」
    // という要望への対応。厳密な物理単位ではなく見た目重視の簡易シミュレーション)。
    const marginX = vw * EDGE_MARGIN_RATIO;
    const marginY = vh * EDGE_MARGIN_RATIO;
    const floorY = vh - marginY - size;
    const ceilY = marginY;
    const leftX = marginX;
    const rightX = Math.max(leftX, vw - marginX - size);

    const edge = ['top', 'bottom', 'left', 'right'][Math.floor(Math.random() * 4)];
    const speed = (vw + vh) * (0.32 + Math.random() * 0.22) * riseFactor;
    let x, y, vx, vy;
    if (edge === 'top') {
      x = marginX + Math.random() * Math.max(1, vw - marginX * 2 - size);
      y = ceilY;
      vx = (Math.random() - 0.5) * speed * 0.7;
      vy = speed * 0.3;
    } else if (edge === 'bottom') {
      x = marginX + Math.random() * Math.max(1, vw - marginX * 2 - size);
      y = floorY;
      vx = (Math.random() - 0.5) * speed * 0.7;
      vy = -speed * 0.85;
    } else if (edge === 'left') {
      x = leftX;
      y = marginY + Math.random() * Math.max(1, vh - marginY * 2 - size);
      vx = speed * 0.8;
      vy = -speed * 0.3;
    } else {
      x = rightX;
      y = marginY + Math.random() * Math.max(1, vh - marginY * 2 - size);
      vx = -speed * 0.8;
      vy = -speed * 0.3;
    }
    const startX = x;
    const startY = y;

    const gravity = speed * 1.6;
    const restitution = 0.56 + Math.random() * 0.12;
    const dtMs = 45;
    const dt = dtMs / 1000;
    const simBudgetMs = Math.round(duration * 0.6);

    const waypoints = [];
    let t = 0;
    let rotateAcc = 0;
    let bounces = 0;
    let settled = false;
    while (t < simBudgetMs && bounces < 10 && !settled) {
      vy += gravity * dt;
      x += vx * dt;
      y += vy * dt;

      if (x < leftX) {
        x = leftX;
        vx = -vx * restitution;
        bounces++;
      } else if (x > rightX) {
        x = rightX;
        vx = -vx * restitution;
        bounces++;
      }
      if (y < ceilY) {
        y = ceilY;
        vy = -vy * restitution;
        bounces++;
      } else if (y > floorY) {
        y = floorY;
        vy = -vy * restitution;
        bounces++;
        if (Math.abs(vy) < speed * 0.05) vy = 0;
      }

      rotateAcc += vx * dt * 0.6;
      t += dtMs;
      waypoints.push({
        atMs: t,
        legMs: dtMs,
        transform: `translate(${x - startX}px, ${y - startY}px) rotate(${rotateAcc}deg) scale(1)`,
      });

      // 床の上でほぼ静止したら、以降の刻みは省略して打ち切る
      if (y >= floorY - 1 && Math.abs(vx) < speed * 0.02 && vy === 0) settled = true;
    }
    if (waypoints.length === 0) {
      waypoints.push({ atMs: dtMs, legMs: dtMs, transform: 'translate(0, 0) rotate(0deg) scale(1)' });
    }

    return {
      startX,
      startY,
      initialTransform: 'translate(0, 0) rotate(0deg) scale(0.85)',
      moveDuration: dtMs,
      easing: 'linear',
      waypoints,
      endX: x,
      endY: y,
    };
  }

  // 'rise'(既定): 下から出てきて上に上がっていく
  const startX = Math.random() * (vw - size);
  const startY = vh - size - Math.random() * (vh * 0.3);
  const drift = (Math.random() - 0.5) * 240 * driftFactor;
  const riseDistance = clampRiseDistance(startY, (vh * 0.6 + size) * riseFactor, vh);
  return {
    startX,
    startY,
    initialTransform: 'translate(0, 0) rotate(0deg) scale(0.6)',
    finalTransform: `translate(${drift}px, -${riseDistance}px) rotate(${rotate}deg) scale(1)`,
    moveDuration: duration,
    easing: 'cubic-bezier(.17,.67,.35,1)',
    endX: startX + drift,
    endY: startY - riseDistance,
  };
}

function spawnReaction(emojiId) {
  const emoji = emojiById.get(emojiId);
  if (!emoji) return;

  enforceMaxConcurrent();

  const kind = emoji.anim || 'laugh';
  const preset = ANIM_PRESETS[kind] || {};
  const motion = preset.motion || 'rise';
  // グリフ(内側のspan)にかけるCSS自己アニメーションの名前。既定ではkindと
  // 同じ名前(anim-clap等)を使うが、複数のリアクションで同じ動き(例: ⬆⬇➡⬅の
  // 「ダッシュ」演出)を使い回したい場合はpreset.glyphAnimで上書きできる。
  const glyphAnimKey = preset.glyphAnim || kind;
  const comboLevel = getComboLevel(emojiId);
  // 「連打すると大きくなる」がOFFの場合は常にcomboLevel=1相当のサイズ固定にする
  // (comboLevel自体の計測・コンボ判定はコンボ表示以外に副作用が無いため続けたまま
  // でよく、サイズへの反映だけを止める)。
  const size = settings.comboGrowthEnabled
    ? settings.baseSizePx + settings.comboSizeStepPx * (comboLevel - 1)
    : settings.baseSizePx;
  const duration =
    settings.animationDurationMs * (0.85 + Math.random() * 0.3) * (preset.durationFactor ?? 1);

  const el = document.createElement('div');
  el.className = 'reaction-item';

  const glyph = document.createElement('span');
  glyph.className = `reaction-glyph anim-${glyphAnimKey}`;
  // 草(id:kusa)のように、絵文字ではなく複数の文字からランダムに1つを選んで
  // 表示したいリアクション用。無ければ従来通りchar固定。
  glyph.textContent = emoji.randomChars
    ? emoji.randomChars[Math.floor(Math.random() * emoji.randomChars.length)]
    : emoji.char;
  glyph.style.fontSize = `${size}px`;
  glyph.style.lineHeight = '1';
  if (emoji.color) {
    // 絵文字ではなく普通の文字として色指定して表示する項目(草・w等)。
    // 線が細く見づらいとの指摘があったため太字にする。
    glyph.style.color = emoji.color;
    glyph.style.fontWeight = '900';
  }
  // グリフ自身のアニメーションは、全体の表示時間の中で2〜3回ループさせて
  // 「動き続けている」印象にする(faint/stampは一度きりの動きなので1回)。
  // 重要: 以前はここでanimationDuration等の個別プロパティしか設定しておらず、
  // 肝心のanimation-name(どのCSS @keyframesを使うか)を一切指定していなかった
  // ため、拍手が弾む・炎がチラつく等の「絵文字ごとの自己アニメーション」が
  // 実際には一度も再生されていなかった(外側el側の移動だけが見えていた)。
  glyph.style.animationName = `anim-${glyphAnimKey}`;
  glyph.style.animationDuration = `${Math.max(400, duration * 0.42)}ms`;
  glyph.style.animationIterationCount =
    glyphAnimKey === 'faint' || glyphAnimKey === 'stamp' ? '1' : '3';
  glyph.style.animationTimingFunction = 'ease-in-out';
  glyph.style.animationFillMode = 'both';

  // 🎉のように絵文字自体のデザイン上いつも同じ向きになってしまうものは、
  // preset.randomFlipが立っていれば約半分の確率で左右反転させる。glyph本体の
  // transformはCSSの@keyframesが常に上書きしてしまうため、反転だけは
  // 別の(アニメーションしない)ラップ要素に持たせて、glyph自身の自己
  // アニメーションと干渉しないようにしている。
  let glyphHost = glyph;
  if (preset.randomFlip && Math.random() < 0.5) {
    const flipWrap = document.createElement('span');
    flipWrap.style.display = 'inline-block';
    flipWrap.style.transform = 'scaleX(-1)';
    flipWrap.appendChild(glyph);
    glyphHost = flipWrap;
  }
  el.appendChild(glyphHost);

  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const path = computeMotionPath(motion, preset, size, vw, vh, duration);

  el.style.left = `${path.startX}px`;
  el.style.top = `${path.startY}px`;
  el.style.transform = path.initialTransform;
  el.style.opacity = '0';
  el.style.transition = `transform ${path.moveDuration}ms ${path.easing}, opacity ${duration}ms ease-out`;

  stage.appendChild(el);
  activeItems.push(el);

  // sideIn(画面端から登場)は開始位置が画面外なので、パーティクル(効果音的な
  // 演出)は登場し終えた「命中/出現」の瞬間に、到着位置で発生させる。それ以外は
  // 最初から画面内にいるので、従来通り開始位置ですぐに発生させる。
  if (preset.particles) {
    // sideIn(画面端から登場)・quickFall(💣: 落下して着地)・popUp(🧨: 跳ねて
    // 上がる)は、開始位置ではなく「到着した瞬間」の位置でパーティクルを
    // 出したい動きなので、moveDurationだけ待ってから終了位置(endX/endY)を
    // 使う。それ以外は従来通り開始位置ですぐに発生させる。
    const particlesAtArrival = motion === 'sideIn' || motion === 'quickFall' || motion === 'popUp';
    const particleDelay = particlesAtArrival ? path.moveDuration : 0;
    // preset.particleOffsetX/Y: 絵文字の中心からずらしてパーティクルを出したい
    // 場合の倍率(サイズ基準)。例: 👺のビームを「鼻の先」あたりから出すため。
    const particleX =
      (particlesAtArrival ? path.endX : path.startX) + size / 2 + size * (preset.particleOffsetX || 0);
    const particleY =
      (particlesAtArrival ? path.endY : path.startY) + size / 2 + size * (preset.particleOffsetY || 0);
    setTimeout(() => {
      spawnParticles(preset.particles, particleX, particleY, size);
    }, particleDelay);
  }

  // 💣 爆弾専用: 着地してからしばらく待ち、ランダムなタイミングで爆発する。
  if (preset.explodeDelayRangeMs) {
    const [minDelay, maxDelay] = preset.explodeDelayRangeMs;
    const explodeDelay = path.moveDuration + minDelay + Math.random() * (maxDelay - minDelay);
    const explodeX = path.endX + size / 2;
    const explodeY = path.endY + size / 2;
    setTimeout(() => {
      if (!el.isConnected) return; // 爆発前に片付けられていたら何もしない(念のため)
      spawnParticles({ shape: 'confetti', count: 22, life: 750 }, explodeX, explodeY, size);
      spawnParticles({ shape: 'ring', life: 550 }, explodeX, explodeY, size * 1.3);
      // 爆発の瞬間、絵文字自体も一瞬弾けて消えるようにする
      el.style.transition = 'transform 260ms ease-out, opacity 260ms ease-out';
      el.style.transform += ' scale(1.6)';
      el.style.opacity = '0';
    }, explodeDelay);
  }

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      // waypointsがある場合(ジグザグ等)は、最初の経由地点をrAFで適用し、
      // 残りはsetTimeoutで順に適用していく(最後の要素が最終的な着地姿勢)。
      // waypointsが無い場合は、従来通り単純に開始→終了の2点間で動かす。
      el.style.transform = path.waypoints ? path.waypoints[0].transform : path.finalTransform;
      el.style.opacity = String(settings.glyphOpacity);
    });
  });

  if (path.waypoints) {
    for (let i = 1; i < path.waypoints.length; i++) {
      const wp = path.waypoints[i];
      setTimeout(() => {
        // wp.legMs: この区間だけ、transitionにかける時間(transform分)を
        // 差し替えたい場合に指定する(例: 💩は「落下」の区間だけ他の区間
        // (着地バウンド)よりゆっくり動かしたい、⚽は物理シミュレーションの
        // 刻み幅ごとに毎回差し替える、等)。transition自体は生成時に
        // `transform ${moveDuration}ms ..., opacity ${duration}ms ...` の
        // 2プロパティで設定してあるので、transitionDurationを同じ並び
        // (transform用, opacity用)で上書きすればよい。
        if (wp.legMs) {
          el.style.transitionDuration = `${wp.legMs}ms, ${duration}ms`;
        }
        el.style.transform = wp.transform;
      }, wp.atMs);
    }
  }

  setTimeout(() => {
    el.style.opacity = '0';
  }, duration * 0.75);

  setTimeout(() => {
    el.remove();
    const idx = activeItems.indexOf(el);
    if (idx !== -1) activeItems.splice(idx, 1);
  }, duration + 100);
}

function enforceMaxConcurrent() {
  while (activeItems.length >= settings.maxConcurrent) {
    const oldest = activeItems.shift();
    if (oldest) oldest.remove();
  }
}

/* ------------------------- パーティクル ------------------------- */

function spawnParticles(spec, cx, cy, baseSize) {
  if (spec.shape === 'ring') {
    spawnRing(cx, cy, baseSize, spec);
    return;
  }
  if (spec.shape === 'laser') {
    spawnLaserBeam(cx, cy, baseSize, spec);
    return;
  }
  const count = spec.count || 4;
  for (let i = 0; i < count; i++) {
    spawnOneParticle(spec, cx, cy, baseSize, i);
  }
}

// 👺天狗のビーム用: (cx, cy)を起点に右方向へ伸びるレーザー光線を1本出す
// (@keyframes particle-laserで、左端を軸にscaleXが0→1に伸びて見せる)。
function spawnLaserBeam(cx, cy, baseSize, spec) {
  const el = document.createElement('div');
  el.className = 'particle';
  const length = spec.length || baseSize * 2.4;
  const thickness = spec.thickness || 5;
  el.style.left = `${cx}px`;
  el.style.top = `${cy - thickness / 2}px`;
  el.style.width = `${length}px`;
  el.style.height = `${thickness}px`;
  el.style.borderRadius = `${thickness}px`;
  el.style.background = spec.color || 'rgba(255,70,70,0.95)';
  el.style.boxShadow = `0 0 ${thickness * 2.5}px ${spec.color || 'rgba(255,70,70,0.95)'}`;
  el.style.transformOrigin = 'left center';
  const life = spec.life || 420;
  el.style.animation = `particle-laser ${life}ms ease-out forwards`;
  stage.appendChild(el);
  setTimeout(() => el.remove(), life + 60);
}

function spawnRing(cx, cy, baseSize, spec) {
  const ring = document.createElement('div');
  ring.className = 'particle';
  const size = spec.small ? baseSize * 1.4 : baseSize * 2.2;
  ring.style.left = `${cx - size / 2}px`;
  ring.style.top = `${cy - size / 2}px`;
  ring.style.width = `${size}px`;
  ring.style.height = `${size}px`;
  ring.style.borderRadius = '50%';
  ring.style.border = `${spec.small ? 2 : 3}px solid rgba(255,255,255,0.85)`;
  ring.style.boxSizing = 'border-box';
  ring.style.animation = `particle-ring ${spec.life}ms ease-out forwards`;
  stage.appendChild(ring);
  setTimeout(() => ring.remove(), spec.life + 50);
}

function spawnOneParticle(spec, cx, cy, baseSize, i) {
  // 放射状(紙吹雪・効果線)は円周方向に、それ以外は上向き寄りの扇状にランダムに散らす
  const isRadial = spec.shape === 'confetti' || spec.shape === 'streak';
  const angle = isRadial ? Math.random() * Math.PI * 2 : (Math.random() - 0.5) * 1.6 - Math.PI / 2;
  const spread = spec.spread || 30;
  const dx = Math.cos(angle) * spread * (0.6 + Math.random() * 0.8);
  const rise = isRadial ? 50 + Math.random() * 70 : spread * (1.1 + Math.random() * 0.7);

  const el = document.createElement('div');
  el.className = 'particle';
  el.style.setProperty('--dx', `${dx}px`);
  el.style.setProperty('--rise', `${rise}px`);
  el.style.setProperty('--rot', `${(angle * 180) / Math.PI}deg`);

  const startOffsetX = (Math.random() - 0.5) * baseSize * 0.5;
  const startOffsetY = (Math.random() - 0.5) * baseSize * 0.3;
  el.style.left = `${cx + startOffsetX}px`;
  el.style.top = `${cy + startOffsetY}px`;

  const life = spec.life || 900;
  const driftKind = isRadial ? 'burst' : spec.drift;
  const animName = DRIFT_ANIM[driftKind] || 'particle-up';
  el.style.animation = `${animName} ${life}ms ease-out forwards`;

  if (spec.shape === 'glyph') {
    const glyphs = spec.glyphs || ['✨'];
    el.textContent = glyphs[i % glyphs.length];
    el.style.fontSize = `${Math.round(baseSize * 0.32)}px`;
  } else if (spec.shape === 'dot') {
    const dotSize = Math.max(4, Math.round(baseSize * 0.12));
    el.style.width = `${dotSize}px`;
    el.style.height = `${dotSize}px`;
    el.style.borderRadius = '50%';
    el.style.background = spec.color || '#fff';
  } else if (spec.shape === 'streak') {
    el.style.width = `${Math.round(baseSize * 0.38)}px`;
    el.style.height = '3px';
    el.style.borderRadius = '2px';
    el.style.background = spec.color || 'rgba(255,255,255,0.85)';
  } else if (spec.shape === 'confetti') {
    el.style.width = `${5 + Math.random() * 4}px`;
    el.style.height = `${8 + Math.random() * 6}px`;
    el.style.background = CONFETTI_COLORS[i % CONFETTI_COLORS.length];
    el.style.borderRadius = '1px';
  }

  stage.appendChild(el);
  setTimeout(() => el.remove(), life + 60);
}
