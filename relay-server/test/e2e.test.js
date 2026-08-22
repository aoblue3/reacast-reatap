'use strict';
const assert = require('assert');
const WebSocket = require('ws');
const crypto = require('crypto');
const { RelayServer } = require('../server');

const PORT = 39299;
const PROTOCOL_VERSION = 2; // relay-client.js側のPROTOCOL_VERSIONと同じ値にしておくこと

// サーバーが複数メッセージを立て続けに送ってくることがあるため、
// once()を毎回付け直す方式だと取りこぼす。受信したメッセージは
// 全てキューに貯めておき、nextMessage()はキューから取り出す/なければ待つ、
// という方式にする。
function connect() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${PORT}`);
    ws._queue = [];
    ws._waiters = [];
    ws.on('message', (raw) => {
      const msg = JSON.parse(raw.toString('utf8'));
      if (ws._waiters.length) {
        ws._waiters.shift()(msg);
      } else {
        ws._queue.push(msg);
      }
    });
    ws.once('open', () => resolve(ws));
    ws.once('error', reject);
  });
}

function nextMessage(ws) {
  if (ws._queue.length) {
    return Promise.resolve(ws._queue.shift());
  }
  return new Promise((resolve) => ws._waiters.push(resolve));
}

function send(ws, obj) {
  ws.send(JSON.stringify(obj));
}

async function run() {
  const watchdog = setTimeout(() => {
    console.error('!! watchdog: 10秒経過してもテストが終わらないため強制終了します');
    process.exit(2);
  }, 10_000);

  const server = new RelayServer({ port: PORT, logger: { log: (...a) => console.log(...a) } }).start();
  await new Promise((r) => setTimeout(r, 100));

  const roomId = crypto.randomBytes(5).toString('hex');
  const broadcasterToken = crypto.randomBytes(24).toString('hex');
  const passphrase = `test-${crypto.randomBytes(3).toString('hex')}`;

  try {
    // --- 1. 配信者が部屋を登録し、合言葉も同時に登録する ---
    const broadcaster = await connect();
    send(broadcaster, {
      type: 'register',
      roomId,
      broadcasterToken,
      passphrase,
      protocolVersion: PROTOCOL_VERSION,
    });
    const regAck = await nextMessage(broadcaster);
    assert.strictEqual(regAck.type, 'registered');
    assert.strictEqual(regAck.roomId, roomId);
    await nextMessage(broadcaster); // viewerCount:0
    const passAck = await nextMessage(broadcaster);
    assert.strictEqual(passAck.type, 'passphrase_ok');
    assert.strictEqual(passAck.passphrase, passphrase.toLowerCase());
    console.log('OK: 配信者の部屋登録・合言葉登録が成功');

    // --- 2. 古いバージョン(protocolVersion不足)からの接続は拒否される ---
    const outdated = await connect();
    send(outdated, { type: 'join', passphrase });
    const outdatedResp = await nextMessage(outdated);
    assert.strictEqual(outdatedResp.type, 'error');
    assert.strictEqual(outdatedResp.code, 'outdated_app');
    outdated.close();
    console.log('OK: protocolVersion未指定(=過去バージョン)の接続を拒否');

    // --- 3. 存在しない合言葉での参加は拒否される ---
    const badViewer = await connect();
    send(badViewer, { type: 'join', passphrase: 'no-such-passphrase', protocolVersion: PROTOCOL_VERSION });
    const badJoin = await nextMessage(badViewer);
    assert.strictEqual(badJoin.type, 'error');
    assert.strictEqual(badJoin.code, 'room_not_found');
    badViewer.close();
    console.log('OK: 存在しない合言葉での参加を拒否');

    // --- 4. 正しい合言葉で視聴者が参加できる(大文字小文字は区別しない) ---
    const viewer = await connect();
    send(viewer, { type: 'join', passphrase: passphrase.toUpperCase(), protocolVersion: PROTOCOL_VERSION });
    const joinAck = await nextMessage(viewer);
    assert.strictEqual(joinAck.type, 'joined');
    await nextMessage(broadcaster); // viewerCount:1 通知
    console.log('OK: 正しい合言葉(大文字小文字を無視)で参加成功');

    // --- 5. 視聴者のリアクションが配信者に届く ---
    send(viewer, { type: 'reaction', emoji: 'wwww' });
    const reactionMsg = await nextMessage(broadcaster);
    assert.strictEqual(reactionMsg.type, 'reaction');
    assert.strictEqual(reactionMsg.emoji, 'wwww');
    assert.ok(reactionMsg.viewerId);
    console.log('OK: リアクションが配信者に転送される');

    // --- 6. 不正な絵文字IDは弾かれる ---
    send(viewer, { type: 'reaction', emoji: '<script>alert(1)</script>' });
    const badEmoji = await nextMessage(viewer);
    assert.strictEqual(badEmoji.type, 'error');
    assert.strictEqual(badEmoji.code, 'invalid_emoji');
    console.log('OK: 不正な形式の絵文字IDを拒否');

    // --- 7. 連打を繰り返すとミュートされる ---
    const mutedPromise = nextMessage(viewer);
    for (let i = 0; i < 25; i++) {
      send(viewer, { type: 'reaction', emoji: 'wwww' });
    }
    const muted = await mutedPromise;
    assert.strictEqual(muted.type, 'muted');
    assert.ok(muted.untilMs > Date.now());
    console.log('OK: 連打を検知してミュートされる');

    // --- 8. ミュート中はリアクションが配信者に転送されない ---
    send(viewer, { type: 'reaction', emoji: 'zzzz' });
    const stillMuted = await nextMessage(viewer);
    assert.strictEqual(stillMuted.type, 'muted');
    console.log('OK: ミュート中は再度muted通知が返り、配信者には転送されない');

    // --- 9. 他人の部屋に、broadcasterTokenが不一致な状態で登録し直そうとすると拒否される ---
    const impostor = await connect();
    send(impostor, {
      type: 'register',
      roomId,
      broadcasterToken: crypto.randomBytes(24).toString('hex'),
      protocolVersion: PROTOCOL_VERSION,
    });
    const impostorResp = await nextMessage(impostor);
    assert.strictEqual(impostorResp.type, 'error');
    assert.strictEqual(impostorResp.code, 'token_mismatch');
    impostor.close();
    console.log('OK: broadcasterToken不一致での部屋の乗っ取りを拒否');

    // --- 10. 既に使われている合言葉は、別の部屋からは登録できない ---
    const otherRoomId = crypto.randomBytes(5).toString('hex');
    const otherToken = crypto.randomBytes(24).toString('hex');
    const otherBroadcaster = await connect();
    send(otherBroadcaster, {
      type: 'register',
      roomId: otherRoomId,
      broadcasterToken: otherToken,
      passphrase,
      protocolVersion: PROTOCOL_VERSION,
    });
    await nextMessage(otherBroadcaster); // registered
    await nextMessage(otherBroadcaster); // viewerCount:0
    const dupResp = await nextMessage(otherBroadcaster);
    assert.strictEqual(dupResp.type, 'error');
    assert.strictEqual(dupResp.code, 'passphrase_taken');
    otherBroadcaster.close();
    console.log('OK: 他の部屋が既に使っている合言葉の重複登録を拒否');

    // --- 11. 合言葉を持つ部屋の配信者接続が切れている(放置された部屋)場合は、
    //     別の部屋がその合言葉を横取りできる(配信者アプリの再インストール等で
    //     roomIdが変わっても、以前から使っていた合言葉をそのまま使い続けられる
    //     ようにするための仕様変更)。
    broadcaster.close();
    await new Promise((resolve) => setTimeout(resolve, 200)); // サーバー側のclose処理の反映を待つ
    const reclaimRoomId = crypto.randomBytes(5).toString('hex');
    const reclaimToken = crypto.randomBytes(24).toString('hex');
    const reclaimBroadcaster = await connect();
    send(reclaimBroadcaster, {
      type: 'register',
      roomId: reclaimRoomId,
      broadcasterToken: reclaimToken,
      passphrase,
      protocolVersion: PROTOCOL_VERSION,
    });
    await nextMessage(reclaimBroadcaster); // registered
    await nextMessage(reclaimBroadcaster); // viewerCount:0
    const reclaimResp = await nextMessage(reclaimBroadcaster);
    assert.strictEqual(reclaimResp.type, 'passphrase_ok');
    assert.strictEqual(reclaimResp.passphrase, passphrase.toLowerCase());
    reclaimBroadcaster.close();
    console.log('OK: 配信者接続が切れている(放置された)部屋が持つ合言葉は、別の部屋が横取りできる');

    viewer.close();

    console.log('\nすべての中継サーバーE2Eテストに成功しました。');
    clearTimeout(watchdog);
  } finally {
    await server.stop();
  }
}

run().catch((err) => {
  console.error('テスト失敗:', err);
  process.exit(1);
});
