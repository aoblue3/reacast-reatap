'use strict';
/**
 * 中継サーバー (仕様書 v0.4 第7章)
 *
 * 役割:
 *   - 配信者ごとの「部屋(room)」を管理する
 *   - 配信者アプリは自分から発信して部屋を登録し、以後は着信を待つのではなく
 *     この接続を維持してリアクションを受け取る(=ポート開放不要)
 *   - 視聴者アプリは配信者が決めた「合言葉」で参加し、リアクションイベントを送る
 *   - 送りつけられたイベントはこの部屋の配信者接続にだけ転送する
 *   - 連打・不正データはここで弾き、配信者PCまで届かないようにする
 *   - 古すぎるバージョンのアプリからの接続を拒否する(過去バージョン対策)
 *
 * プロトコル (WebSocket上でJSON1行=1メッセージ)。register/joinには必ず
 * protocolVersionを含める(古いバージョンのアプリを弾くため。下のPROTOCOL_VERSION
 * 定数を参照)。
 *   C->S register  {type:'register', roomId, broadcasterToken, passphrase, protocolVersion}
 *   C->S join      {type:'join', passphrase, protocolVersion}
 *   C->S reaction  {type:'reaction', emoji}                (join済み視聴者のみ)
 *   S->C ok        {type:'registered', roomId} / {type:'joined', roomId}
 *   S->C passphrase_ok {type:'passphrase_ok', passphrase}   (配信者のみ。合言葉の登録/変更が成功した通知)
 *   S->C error     {type:'error', code, message}
 *   S->C reaction  {type:'reaction', emoji, viewerId, ts}  (配信者のみ受信)
 *   S->C muted     {type:'muted', untilMs}
 *   S->C viewerCount {type:'viewerCount', count}           (配信者のみ、参考情報)
 *
 * 合言葉について: 以前は「中継サーバーのアドレス+部屋ID+視聴者トークン」を
 * 暗号化して長い接続コードとして配布していたが、手入力しづらいという要望を受け、
 * 配信者が自分で決めた短い合言葉(passphrase)で視聴者を受け入れる方式に変更した。
 * 中継サーバーのアドレスは視聴者アプリのビルド時に埋め込む前提になったので、
 * 視聴者はこの合言葉だけを入力すればよい。合言葉は大文字小文字を区別せず
 * (内部で小文字化して比較・保存する)、他の部屋と重複しては使えない
 * (passphrases Mapで排他制御する)。
 */

const WebSocket = require('ws');

// ---- 設定値 ----
const RATE_LIMIT_WINDOW_MS = 10_000; // 直近何ミリ秒を見るか
// その間に許容する最大リアクション数。クライアント側のdebounce(bar.js DEBOUNCE_MS=500ms)
// による理論上の最速値はちょうど20回/10秒なので、タイマーのずれ等で正規の
// 最速連打が誤ってミュートされないよう、あえてぴったりにはせず1回分の余裕を持たせる。
const RATE_LIMIT_MAX_EVENTS = 19;
const MUTE_DURATION_MS = 5 * 60_000; // 超過時のミュート時間 (5分)
const RATE_STATE_CLEANUP_INTERVAL_MS = 60_000; // IP別レート状態の掃除間隔
const RATE_STATE_MAX_IDLE_MS = 10 * 60_000; // このぶん操作が無いIP別レート状態は掃除してよい
const MAX_MESSAGE_BYTES = 2048; // 1メッセージの最大サイズ(不正・過大なデータを弾く)
const ALLOWED_EMOJI_ID_RE = /^[a-zA-Z0-9_-]{1,32}$/; // 絵文字IDの形式チェック
const ROOM_ID_RE = /^[0-9a-f]{10}$/; // 5byte hex
const BROADCASTER_TOKEN_RE = /^[0-9a-f]{48}$/; // 24byte hex
const PASSPHRASE_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{2,31}$/; // 英数字で開始、3〜32文字(- _ 可)

// この中継サーバーが要求する最低プロトコルバージョン。これより小さい
// protocolVersionを送ってきた接続(=これより古いバージョンのアプリ)は、
// メッセージ形式の非互換による誤動作を避けるため一律で拒否する。
// 将来、register/joinのメッセージ形式などプロトコルに互換性を壊す変更を
// 加えた時は、ここと各アプリのrelay-client.js内のPROTOCOL_VERSIONを
// 同時に(新しい値に)上げること。
const MIN_PROTOCOL_VERSION = 2;

/**
 * 部屋の状態:
 *   {
 *     broadcasterToken, passphrase (小文字化済み、未設定ならnull),
 *     broadcasterConn: ws|null,
 *     viewerConns: Set<ws>,
 *     createdAt, lastActiveAt,
 *   }
 */
class RelayServer {
  constructor({ port = 39200, logger = console } = {}) {
    this.port = port;
    this.logger = logger;
    this.rooms = new Map(); // roomId -> room
    this.passphrases = new Map(); // 小文字化した合言葉 -> roomId
    // 連打防止のレート状態は、以前は接続(ws)ごとのWeakMapで管理していたが、
    // それだと「ミュートされたら一旦切断してすぐ繋ぎ直す」だけで簡単に
    // リセットできてしまう不具合があった(接続し直すたびに新しいwsオブジェクトが
    // 作られ、WeakMapのキーも新品になるため)。そこで「部屋ID+接続元IP」を
    // キーにした通常のMapで管理し、繋ぎ直してもレート状態が引き継がれるようにする。
    this.rateByRoomIp = new Map(); // "roomId:ip" -> {timestamps:[], mutedUntil:0, lastSeenAt}
    this.wss = null;
    this._rateCleanupTimer = null;
  }

  start() {
    this.wss = new WebSocket.Server({ port: this.port, maxPayload: MAX_MESSAGE_BYTES });
    this.wss.on('connection', (ws, req) => this._handleConnection(ws, req));
    this.logger.log(`[relay] listening on ws://0.0.0.0:${this.port}`);

    // 使われなくなったIP別レート状態が溜まり続けないよう定期的に掃除する。
    this._rateCleanupTimer = setInterval(() => this._cleanupRateState(), RATE_STATE_CLEANUP_INTERVAL_MS);
    if (typeof this._rateCleanupTimer.unref === 'function') this._rateCleanupTimer.unref();

    return this;
  }

  stop() {
    return new Promise((resolve) => {
      if (this._rateCleanupTimer) {
        clearInterval(this._rateCleanupTimer);
        this._rateCleanupTimer = null;
      }
      if (!this.wss) return resolve();
      // ws の Server#close() は「新規接続の受付を止める」だけで、既存の接続が
      // 自然にcloseするまで待ち続けてしまう。サーバーを止める操作としては
      // 接続中のクライアントも強制的に切断すべきなので、ここで能動的に閉じる。
      for (const client of this.wss.clients) {
        try { client.terminate(); } catch { /* noop */ }
      }
      this.wss.close(() => resolve());
    });
  }

  // 接続元のIPアドレスを取り出す("::ffff:1.2.3.4"のようなIPv4-mapped IPv6
  // 表記は素のIPv4に正規化する)。取得できない場合は全員まとめて1つの
  // 仮想IP扱いになる(レート制限が多少厳しめになるだけで、安全側に倒れる)。
  _extractIp(req) {
    let ip = (req && req.socket && req.socket.remoteAddress) || 'unknown';
    if (ip.startsWith('::ffff:')) ip = ip.slice(7);
    return ip;
  }

  _cleanupRateState() {
    const now = Date.now();
    for (const [key, rate] of this.rateByRoomIp) {
      if (now - rate.lastSeenAt > RATE_STATE_MAX_IDLE_MS && rate.mutedUntil <= now) {
        this.rateByRoomIp.delete(key);
      }
    }
  }

  _handleConnection(ws, req) {
    ws._role = null; // 'broadcaster' | 'viewer'
    ws._roomId = null;
    ws._viewerId = null;
    ws._ip = this._extractIp(req);

    ws.on('message', (raw) => this._handleMessage(ws, raw));
    ws.on('close', () => this._handleClose(ws));
    ws.on('error', () => {
      /* 個別接続のエラーはcloseで後始末するので握りつぶす */
    });
  }

  _send(ws, obj) {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(obj));
    }
  }

  _handleMessage(ws, raw) {
    if (Buffer.byteLength(raw) > MAX_MESSAGE_BYTES) {
      return this._send(ws, { type: 'error', code: 'too_large', message: 'メッセージが大きすぎます' });
    }

    let msg;
    try {
      msg = JSON.parse(raw.toString('utf8'));
    } catch {
      return this._send(ws, { type: 'error', code: 'bad_json', message: 'JSONとして解釈できません' });
    }

    if (!msg || typeof msg.type !== 'string') {
      return this._send(ws, { type: 'error', code: 'bad_message', message: 'typeが必要です' });
    }

    // 過去バージョン対策: register/joinはアプリ間の通信仕様そのものに関わる
    // ため、ここでprotocolVersionを検査する(reactionは接続確立後のメッセージ
    // なので、その時点で既にversion確認済み=対象外)。
    if (msg.type === 'register' || msg.type === 'join') {
      if (!Number.isInteger(msg.protocolVersion) || msg.protocolVersion < MIN_PROTOCOL_VERSION) {
        return this._send(ws, {
          type: 'error',
          code: 'outdated_app',
          message:
            'アプリのバージョンが古いため接続できません。最新版のアプリに更新してください。',
        });
      }
    }

    switch (msg.type) {
      case 'register':
        return this._handleRegister(ws, msg);
      case 'join':
        return this._handleJoin(ws, msg);
      case 'reaction':
        return this._handleReaction(ws, msg);
      default:
        return this._send(ws, { type: 'error', code: 'unknown_type', message: `未知のtype: ${msg.type}` });
    }
  }

  _handleRegister(ws, msg) {
    const { roomId, broadcasterToken } = msg;
    if (!ROOM_ID_RE.test(roomId) || !BROADCASTER_TOKEN_RE.test(broadcasterToken)) {
      return this._send(ws, { type: 'error', code: 'invalid_params', message: 'roomId/broadcasterTokenが不正です' });
    }

    let room = this.rooms.get(roomId);
    if (room) {
      // 既存の部屋: トークンが一致する場合のみ再登録(再接続)を許可する。
      // 一致しない場合は他人による部屋乗っ取りの可能性があるため拒否する。
      if (room.broadcasterToken !== broadcasterToken) {
        return this._send(ws, { type: 'error', code: 'token_mismatch', message: 'broadcasterTokenが一致しません' });
      }
      if (room.broadcasterConn && room.broadcasterConn !== ws) {
        try { room.broadcasterConn.close(); } catch { /* noop */ }
      }
    } else {
      room = {
        broadcasterToken,
        passphrase: null,
        broadcasterConn: null,
        viewerConns: new Set(),
        createdAt: Date.now(),
      };
      this.rooms.set(roomId, room);
    }

    room.broadcasterConn = ws;
    room.lastActiveAt = Date.now();
    ws._role = 'broadcaster';
    ws._roomId = roomId;

    this._send(ws, { type: 'registered', roomId });
    this._send(ws, { type: 'viewerCount', count: room.viewerConns.size });
    this.logger.log(`[relay] room ${roomId}: broadcaster registered`);

    // 合言葉の登録・変更(register時に指定されていれば)。部屋の登録自体は
    // 上で既に完了させてあるので、合言葉が重複していて弾かれても再接続自体は
    // 成功する(合言葉だけ別途エラーを返し、配信者アプリ側で分かるようにする)。
    if (typeof msg.passphrase === 'string') {
      this._handleSetPassphrase(ws, room, roomId, msg.passphrase);
    }
  }

  _handleSetPassphrase(ws, room, roomId, passphrase) {
    if (!PASSPHRASE_RE.test(passphrase)) {
      return this._send(ws, {
        type: 'error',
        code: 'invalid_passphrase',
        message: '合言葉は英数字で始まる3〜32文字(英数字・ハイフン・アンダースコアのみ)で指定してください',
      });
    }
    const normalized = passphrase.toLowerCase();
    if (room.passphrase === normalized) {
      // 変更なし(再接続時に前回と同じ合言葉を送ってきた場合等)。そのまま成功扱い。
      return this._send(ws, { type: 'passphrase_ok', passphrase: normalized });
    }
    const owner = this.passphrases.get(normalized);
    if (owner && owner !== roomId) {
      const ownerRoom = this.rooms.get(owner);
      // 「配信者アプリを再ビルド/再インストールした後は前回と別のroomIdになる
      // ことがあり(アプリ識別子の変更等)、以前使っていた合言葉が古い(今はもう
      // 誰も繋がっていない)部屋に握られたままになって二度と使えなくなる」という
      // 報告を受けての対応。持ち主の部屋に配信者接続が実際に無い(=放置された
      // 部屋)場合に限り、新しい部屋がその合言葉を横取りしてよいことにする。
      // 現に配信中の部屋から奪うことはできない(そちらは従来通り拒否する)ので、
      // 稼働中の配信の合言葉が他人に奪われる心配は無い。
      const ownerActive =
        ownerRoom && ownerRoom.broadcasterConn && ownerRoom.broadcasterConn.readyState === WebSocket.OPEN;
      if (ownerActive) {
        return this._send(ws, {
          type: 'error',
          code: 'passphrase_taken',
          message: 'その合言葉は既に他の配信で使われています。別の合言葉を試してください',
        });
      }
      // 放置された部屋からマッピングを解放する(その部屋自身の状態もついでに
      // 整合させておく。誰も見ていない部屋なので実害は無い)。
      this.passphrases.delete(normalized);
      if (ownerRoom) ownerRoom.passphrase = null;
      this.logger.log(`[relay] room ${roomId}: passphrase "${normalized}" was held by inactive room ${owner}; reclaiming`);
    }
    // 古い合言葉が別の値だった場合は、そのマッピングを解放してから新しい方を登録する
    if (room.passphrase) {
      this.passphrases.delete(room.passphrase);
    }
    room.passphrase = normalized;
    this.passphrases.set(normalized, roomId);
    this._send(ws, { type: 'passphrase_ok', passphrase: normalized });
    this.logger.log(`[relay] room ${roomId}: passphrase set`);
  }

  _handleJoin(ws, msg) {
    const { passphrase } = msg;
    if (typeof passphrase !== 'string' || !PASSPHRASE_RE.test(passphrase)) {
      return this._send(ws, { type: 'error', code: 'invalid_params', message: '合言葉の形式が不正です' });
    }

    const roomId = this.passphrases.get(passphrase.toLowerCase());
    const room = roomId ? this.rooms.get(roomId) : null;
    if (!room) {
      return this._send(ws, {
        type: 'error',
        code: 'room_not_found',
        message: 'その合言葉に該当する配信が見つかりません(合言葉を確認するか、配信者に最新のものを聞いてください)',
      });
    }

    room.viewerConns.add(ws);
    ws._role = 'viewer';
    ws._roomId = roomId;
    ws._viewerId = `v_${Math.random().toString(36).slice(2, 10)}`;

    this._send(ws, { type: 'joined', roomId });
    if (room.broadcasterConn) {
      this._send(room.broadcasterConn, { type: 'viewerCount', count: room.viewerConns.size });
    }
    this.logger.log(`[relay] room ${roomId}: viewer joined (${room.viewerConns.size} total)`);
  }

  _handleReaction(ws, msg) {
    if (ws._role !== 'viewer' || !ws._roomId) {
      return this._send(ws, { type: 'error', code: 'not_joined', message: '先にjoinしてください' });
    }

    const rateKey = `${ws._roomId}:${ws._ip}`;
    let rate = this.rateByRoomIp.get(rateKey);
    if (!rate) {
      rate = { timestamps: [], mutedUntil: 0, lastSeenAt: 0 };
      this.rateByRoomIp.set(rateKey, rate);
    }
    const now = Date.now();
    rate.lastSeenAt = now;

    if (rate.mutedUntil > now) {
      return this._send(ws, { type: 'muted', untilMs: rate.mutedUntil });
    }

    rate.timestamps.push(now);
    // ウィンドウ外の古い記録を捨てる
    while (rate.timestamps.length && now - rate.timestamps[0] > RATE_LIMIT_WINDOW_MS) {
      rate.timestamps.shift();
    }
    if (rate.timestamps.length > RATE_LIMIT_MAX_EVENTS) {
      rate.mutedUntil = now + MUTE_DURATION_MS;
      rate.timestamps = [];
      this.logger.log(`[relay] room ${ws._roomId}: viewer ${ws._viewerId} を連打検知でミュート`);
      return this._send(ws, { type: 'muted', untilMs: rate.mutedUntil });
    }

    if (typeof msg.emoji !== 'string' || !ALLOWED_EMOJI_ID_RE.test(msg.emoji)) {
      return this._send(ws, { type: 'error', code: 'invalid_emoji', message: 'emojiの形式が不正です' });
    }

    const room = this.rooms.get(ws._roomId);
    if (!room) return; // 部屋が消えている(異常系)

    if (room.broadcasterConn) {
      this._send(room.broadcasterConn, {
        type: 'reaction',
        emoji: msg.emoji,
        viewerId: ws._viewerId,
        ts: now,
      });
    }
    // 配信者が今オフラインでも、視聴者へはエラーを返さない
    // (配信者アプリが再接続すれば以降のイベントから復帰する。取りこぼしは許容する設計)
  }

  _handleClose(ws) {
    const roomId = ws._roomId;
    if (!roomId) return;
    const room = this.rooms.get(roomId);
    if (!room) return;

    if (ws._role === 'broadcaster' && room.broadcasterConn === ws) {
      room.broadcasterConn = null;
      this.logger.log(`[relay] room ${roomId}: broadcaster disconnected`);
    } else if (ws._role === 'viewer') {
      room.viewerConns.delete(ws);
      if (room.broadcasterConn) {
        this._send(room.broadcasterConn, { type: 'viewerCount', count: room.viewerConns.size });
      }
    }
  }
}

module.exports = { RelayServer };

if (require.main === module) {
  const port = Number(process.env.PORT) || 39200;
  new RelayServer({ port }).start();
}
