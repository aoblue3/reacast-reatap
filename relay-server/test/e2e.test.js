'use strict';
const assert = require('assert');
const WebSocket = require('ws');
const crypto = require('crypto');
const http = require('http');
const { RelayServer } = require('../server');

const PORT = 39299;
const PROTOCOL_VERSION = 2; // relay-client.js側のPROTOCOL_VERSIONと同じ値にしておくこと

// ReaTap Web(web-viewer/)の静的配信のテスト用。WebSocketと同じポートで
// 普通のHTTP GETがちゃんと処理されるかを見る(RelayServer#_handleHttpRequest参照)。
function httpGet(pathname) {
  return new Promise((resolve, reject) => {
    const req = http.get(`http://127.0.0.1:${PORT}${pathname}`, (res) => {
      let body = '';
      res.on('data', (chunk) => (body += chunk));
      res.on('end', () => resolve({ statusCode: res.statusCode, headers: res.headers, body }));
    });
    req.once('error', reject);
  });
}

// サーバーが複数メッセージを立て続けに送ってくることがあるため、
// once()を毎回付け直す方式だと取りこぼす。受信したメッセージは
// 全てキューに貯めておき、nextMessage()はキューから取り出す/なければ待つ、
// という方式にする。
function connect(options) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${PORT}`, options);
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

    // --- 8b. 接続を切ってすぐ繋ぎ直しても、ミュートはリセットされない
    //     (以前は接続ごとにレート状態を持っていたため、切断→再接続するだけで
    //     連打防止を無効化できてしまっていた不具合の再発防止テスト) ---
    viewer.close();
    await new Promise((resolve) => setTimeout(resolve, 100));
    const viewerReconnected = await connect();
    send(viewerReconnected, { type: 'join', passphrase, protocolVersion: PROTOCOL_VERSION });
    await nextMessage(viewerReconnected); // joined
    await nextMessage(broadcaster); // viewerCount通知
    send(viewerReconnected, { type: 'reaction', emoji: 'wwww' });
    const stillMutedAfterReconnect = await nextMessage(viewerReconnected);
    assert.strictEqual(stillMutedAfterReconnect.type, 'muted');
    console.log('OK: 切断して繋ぎ直しても連打防止のミュートは引き継がれる(同一IP・同一部屋)');

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

    viewerReconnected.close();

    // --- 12. ReaTap Web(web-viewer/)の静的ファイルが同じポートで配信される ---
    const indexResp = await httpGet('/');
    assert.strictEqual(indexResp.statusCode, 200);
    assert.ok(indexResp.headers['content-type'].startsWith('text/html'));
    assert.ok(indexResp.body.includes('ReaTap Web'));
    console.log('OK: ReaTap Webのindex.htmlが"/"で配信される');

    const appJsResp = await httpGet('/app.js');
    assert.strictEqual(appJsResp.statusCode, 200);
    assert.ok(appJsResp.headers['content-type'].startsWith('text/javascript'));
    console.log('OK: ReaTap Webのapp.jsが配信される');

    const notFoundResp = await httpGet('/no-such-file.html');
    assert.strictEqual(notFoundResp.statusCode, 404);
    console.log('OK: 存在しない静的ファイルは404になる');

    // パストラバーサル対策(web-viewer/の外にあるファイルを覗き見できないこと)。
    // 素の"../"はnew URL()のパス正規化で先に潰れてしまい、このサーバーの
    // ガード自体を通らないため、それをすり抜けるための"%2f"(エンコードした
    // "/")を使ったペイロードでテストする(_handleHttpRequestのコメント参照。
    // decodeURIComponent()が正規化"後"に効くため、"..%2f"は正規化をすり抜けて
    // 一旦"/../"に戻ってしまう。それを最終的なパス比較でブロックできているかの
    // テスト)。
    const traversalResp = await httpGet('/..%2f..%2fserver.js');
    assert.strictEqual(traversalResp.statusCode, 403);
    console.log('OK: エンコードされたパストラバーサル(..%2f)でweb-viewer/の外のファイルは読めない');

    // --- 13. NUL文字(%00)を含むパスでサーバーが落ちない ---
    const nulResp = await httpGet('/%00');
    assert.strictEqual(nulResp.statusCode, 400);
    const afterNulResp = await httpGet('/');
    assert.strictEqual(afterNulResp.statusCode, 200);
    console.log('OK: /%00 にアクセスしてもサーバーが落ちない(400を返す)');

    // --- 14. ブラウザ版(ReaTap Web)は同一IPから1接続まで(後から来た方が残る)。exe版は対象外 ---
    const webPassphrase = `web-${crypto.randomBytes(3).toString('hex')}`;
    const webBroadcaster = await connect();
    send(webBroadcaster, {
      type: 'register',
      roomId: crypto.randomBytes(5).toString('hex'),
      broadcasterToken: crypto.randomBytes(24).toString('hex'),
      passphrase: webPassphrase,
      protocolVersion: PROTOCOL_VERSION,
    });
    assert.strictEqual((await nextMessage(webBroadcaster)).type, 'registered');
    const webOrigin = { origin: `http://127.0.0.1:${PORT}` };
    const web1 = await connect(webOrigin);
    send(web1, { type: 'join', passphrase: webPassphrase, protocolVersion: PROTOCOL_VERSION });
    assert.strictEqual((await nextMessage(web1)).type, 'joined');
    const web1Closed = new Promise((r) => web1.once('close', r));
    const web2 = await connect(webOrigin);
    send(web2, { type: 'join', passphrase: webPassphrase, protocolVersion: PROTOCOL_VERSION });
    assert.strictEqual((await nextMessage(web2)).type, 'joined');
    const web1Replaced = await nextMessage(web1);
    assert.strictEqual(web1Replaced.type, 'error');
    assert.strictEqual(web1Replaced.code, 'replaced');
    await web1Closed;
    console.log('OK: ブラウザ版の2本目の接続が来ると、1本目は replaced で切断される(繋ぎ直しは断られない)');

    // 1本目が切れた後も、2本目のリアクションはちゃんと配信者に届く
    send(web2, { type: 'reaction', emoji: 'clap' });
    let fwd;
    do { fwd = await nextMessage(webBroadcaster); } while (fwd.type !== 'reaction');
    assert.strictEqual(fwd.emoji, 'clap');
    web2.close();
    console.log('OK: 残った2本目の接続からのリアクションは配信者に届く');

    const tauriOrigin = { origin: 'http://tauri.localhost' };
    const exe1 = await connect(tauriOrigin);
    const exe2 = await connect(tauriOrigin);
    for (const exe of [exe1, exe2]) {
      send(exe, { type: 'join', passphrase: webPassphrase, protocolVersion: PROTOCOL_VERSION });
      assert.strictEqual((await nextMessage(exe)).type, 'joined');
    }
    exe1.close();
    exe2.close();
    webBroadcaster.close();
    console.log('OK: exe版は同じIPから複数接続できる');

    // --- 15. 生存確認(ping)に応答する接続は切られない ---
    const alive = await connect();
    server._heartbeat();
    await new Promise((r) => setTimeout(r, 100));
    server._heartbeat();
    await new Promise((r) => setTimeout(r, 100));
    assert.strictEqual(alive.readyState, WebSocket.OPEN);
    alive.close();
    console.log('OK: pingに応答している接続は生存確認で切られない');

    // --- 16. 誰も繋がっていない部屋は一定時間後に削除される ---
    const idleRoomId = crypto.randomBytes(5).toString('hex');
    const idlePassphrase = `idle-${crypto.randomBytes(3).toString('hex')}`;
    const idleBroadcaster = await connect();
    send(idleBroadcaster, {
      type: 'register',
      roomId: idleRoomId,
      broadcasterToken: crypto.randomBytes(24).toString('hex'),
      passphrase: idlePassphrase,
      protocolVersion: PROTOCOL_VERSION,
    });
    assert.strictEqual((await nextMessage(idleBroadcaster)).type, 'registered');
    const DAY_PLUS = 25 * 60 * 60_000;
    server._cleanupIdleRooms(Date.now() + DAY_PLUS);
    assert.ok(server.rooms.has(idleRoomId), '配信者が繋がっている部屋は消えない');
    idleBroadcaster.close();
    await new Promise((r) => setTimeout(r, 100));
    server._cleanupIdleRooms(Date.now());
    assert.ok(server.rooms.has(idleRoomId), '切断直後の部屋はまだ消えない');
    server._cleanupIdleRooms(Date.now() + DAY_PLUS);
    assert.ok(!server.rooms.has(idleRoomId));
    assert.ok(!server.passphrases.has(idlePassphrase));
    console.log('OK: 誰も繋がっていない部屋は一定時間後に合言葉ごと削除される');

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
