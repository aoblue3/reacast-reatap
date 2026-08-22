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
    applyGlyphScale(event.payload.glyphScale);
  });
  window.__TAURI__.core
    .invoke('get_overlay_settings')
    .then((s) => applyGlyphScale(s.glyphScale))
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
      if (data.type === 'settings' && typeof data.glyphScale === 'number') {
        applyGlyphScale(data.glyphScale);
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
    // 横に倒れる動きをするので、外側はあまり動かさず沈む距離だけ小さく持たせる)
    const startX = Math.random() * (vw - size);
    const startY = vh - size - Math.random() * (vh * 0.25);
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
  const size = settings.baseSizePx + settings.comboSizeStepPx * (comboLevel - 1);
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
    const isOffscreenStart = motion === 'sideIn';
    const particleDelay = isOffscreenStart ? path.moveDuration : 0;
    const particleX = (isOffscreenStart ? path.endX : path.startX) + size / 2;
    const particleY = (isOffscreenStart ? path.endY : path.startY) + size / 2;
    setTimeout(() => {
      spawnParticles(preset.particles, particleX, particleY, size);
    }, particleDelay);
  }

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      // waypointsがある場合(ジグザグ等)は、最初の経由地点をrAFで適用し、
      // 残りはsetTimeoutで順に適用していく(最後の要素が最終的な着地姿勢)。
      // waypointsが無い場合は、従来通り単純に開始→終了の2点間で動かす。
      el.style.transform = path.waypoints ? path.waypoints[0].transform : path.finalTransform;
      el.style.opacity = '1';
    });
  });

  if (path.waypoints) {
    for (let i = 1; i < path.waypoints.length; i++) {
      const wp = path.waypoints[i];
      setTimeout(() => {
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
  const count = spec.count || 4;
  for (let i = 0; i < count; i++) {
    spawnOneParticle(spec, cx, cy, baseSize, i);
  }
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
