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
 *   C->S disabledReactions {type:'disabledReactions', ids:[emojiId...]}  (配信者のみ。配信者がOFFにしているリアクション)
 *   S->C ok        {type:'registered', roomId} / {type:'joined', roomId}
 *   S->C passphrase_ok {type:'passphrase_ok', passphrase}   (配信者のみ。合言葉の登録/変更が成功した通知)
 *   S->C error     {type:'error', code, message}
 *   S->C reaction  {type:'reaction', emoji, viewerId, ts}  (配信者のみ受信)
 *   S->C muted     {type:'muted', untilMs}
 *   S->C viewerCount {type:'viewerCount', count}           (配信者のみ、参考情報)
 *   S->C disabledReactions {type:'disabledReactions', ids}  (視聴者のみ。join直後と、配信者が変更した時)
 *
 * disabledReactionsは後から追加したメッセージ。古い視聴者アプリは知らないtypeを
 * 無視するだけ、古い中継サーバーは配信者にunknown_typeエラーを返すだけ(配信者
 * アプリ側は無視する)なので、protocolVersionは上げていない。
 *
 * 合言葉について: 以前は「中継サーバーのアドレス+部屋ID+視聴者トークン」を
 * 暗号化して長い接続コードとして配布していたが、手入力しづらいという要望を受け、
 * 配信者が自分で決めた短い合言葉(passphrase)で視聴者を受け入れる方式に変更した。
 * 中継サーバーのアドレスは視聴者アプリのビルド時に埋め込む前提になったので、
 * 視聴者はこの合言葉だけを入力すればよい。合言葉は大文字小文字を区別せず
 * (内部で小文字化して比較・保存する)、他の部屋と重複しては使えない
 * (passphrases Mapで排他制御する)。
 *
 * ReaTap Web(web-viewer/)について: exeのインストールが敷居が高いという声を
 * 受け、ブラウザだけで合言葉入力→タップができる軽量版を追加した。この
 * 中継サーバー自身がweb-viewer/配下の静的ファイル(index.html等)も一緒に
 * 配信することで、ReaTap Web側は「このページを開いているサーバーに、
 * そのままWebSocketで繋ぎに行けばよい」だけで済み、利用者側でアドレスを
 * 設定する必要が一切無くなる(web-viewer/app.jsのresolveRelayUrl参照)。
 * WebSocketのアップグレード要求はwsライブラリが処理し、それ以外の通常の
 * HTTP GETリクエストだけをこの静的配信ロジックが処理する(同じポート・
 * 同じhttp.Serverを共有しているだけで、お互いに干渉しない)。
 */

const WebSocket = require('ws');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { URL } = require('url');

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
const MAX_DISABLED_REACTIONS = 200; // disabledReactionsで受け付けるIDの最大数
const ROOM_ID_RE = /^[0-9a-f]{10}$/; // 5byte hex
const BROADCASTER_TOKEN_RE = /^[0-9a-f]{48}$/; // 24byte hex
const PASSPHRASE_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{2,31}$/; // 英数字で開始、3〜32文字(- _ 可)
// 接続の生存確認(ping)の間隔。応答(pong)が次のpingまでに返ってこない接続は
// 切れているとみなして破棄する。スマホのブラウザを閉じた・電波が切れた等で
// closeが届かないまま残った接続を掃除するため(残ったままだと、後述の
// 「ブラウザ版は同一IPから1接続まで」の制限で本人が繋ぎ直せなくなる)。
const HEARTBEAT_INTERVAL_MS = 30_000;
// 配信者も視聴者も繋がっていない状態でこの時間が経った部屋は削除する
// (部屋が消えても、配信者アプリが同じroomId/トークンで再登録すれば
// 新しく作り直されるだけなので実害は無い)。
const ROOM_IDLE_TTL_MS = 24 * 60 * 60_000;
const ROOM_CLEANUP_INTERVAL_MS = 10 * 60_000;

// この中継サーバーが要求する最低プロトコルバージョン。これより小さい
// protocolVersionを送ってきた接続(=これより古いバージョンのアプリ)は、
// メッセージ形式の非互換による誤動作を避けるため一律で拒否する。
// 将来、register/joinのメッセージ形式などプロトコルに互換性を壊す変更を
// 加えた時は、ここと各アプリのrelay-client.js内のPROTOCOL_VERSIONを
// 同時に(新しい値に)上げること。
const MIN_PROTOCOL_VERSION = 2;

// ReaTap Web(web-viewer/)の静的ファイルの既定の場所。このファイル自身の
// 場所(__dirname)基準の相対パスにしておくことで、`cd relay-server &&
// node server.js`のようにどのディレクトリから起動しても正しく解決される
// (README.mdの起動手順を参照。process.cwd()基準にすると起動場所によって
// 壊れるため避けている)。
const DEFAULT_STATIC_DIR = path.join(__dirname, '..', 'web-viewer');

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

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
  constructor({
    port = 39200,
    logger = console,
    staticDir = DEFAULT_STATIC_DIR,
    heartbeatIntervalMs = HEARTBEAT_INTERVAL_MS,
    roomIdleTtlMs = ROOM_IDLE_TTL_MS,
  } = {}) {
    this.port = port;
    this.logger = logger;
    // staticDirが実際に存在しない環境(web-viewer/を配置していないデプロイ等)
    // でも中継サーバー自体は問題なく起動できるようにするため、存在確認だけ
    // ここでしておき、無ければ静的配信自体を諦める(HTTPリクエストには
    // 常に404を返す。WebSocketの中継機能には一切影響しない)。
    this.staticDir = staticDir && fs.existsSync(staticDir) ? staticDir : null;
    this.rooms = new Map(); // roomId -> room
    this.passphrases = new Map(); // 小文字化した合言葉 -> roomId
    // 連打防止のレート状態は、以前は接続(ws)ごとのWeakMapで管理していたが、
    // それだと「ミュートされたら一旦切断してすぐ繋ぎ直す」だけで簡単に
    // リセットできてしまう不具合があった(接続し直すたびに新しいwsオブジェクトが
    // 作られ、WeakMapのキーも新品になるため)。そこで「部屋ID+接続元IP」を
    // キーにした通常のMapで管理し、繋ぎ直してもレート状態が引き継がれるようにする。
    this.rateByRoomIp = new Map(); // "roomId:ip" -> {timestamps:[], mutedUntil:0, lastSeenAt}
    // ブラウザ版(ReaTap Web)の接続は同一IPから1本までに制限する(exe版は対象外)。
    this.webConnByIp = new Map(); // ip -> ws
    this.heartbeatIntervalMs = heartbeatIntervalMs;
    this.roomIdleTtlMs = roomIdleTtlMs;
    this.httpServer = null;
    this.wss = null;
    this._rateCleanupTimer = null;
    this._heartbeatTimer = null;
    this._roomCleanupTimer = null;
  }

  start() {
    // 以前はWebSocket.Serverに直接{port}を渡して単独でlistenさせていたが、
    // ReaTap Web(web-viewer/)の静的ファイルも同じポートで配信したいため、
    // 先に素のhttp.Serverを作り、それをWebSocket.Serverと共有する形に変更した。
    // 通常のHTTP GETリクエストはこのhttp.Server自身のrequestハンドラ
    // (_handleHttpRequest)が処理し、WebSocketのアップグレード要求はws
    // ライブラリが従来通り自動的に横取りして処理する(互いに干渉しない)。
    this.httpServer = http.createServer((req, res) => this._handleHttpRequest(req, res));
    this.wss = new WebSocket.Server({ server: this.httpServer, maxPayload: MAX_MESSAGE_BYTES });
    this.wss.on('connection', (ws, req) => this._handleConnection(ws, req));
    this.httpServer.listen(this.port, () => {
      this.logger.log(`[relay] listening on ws://0.0.0.0:${this.port}`);
      if (this.staticDir) {
        this.logger.log(`[relay] serving ReaTap Web from ${this.staticDir} (http://0.0.0.0:${this.port}/)`);
      }
    });

    // 使われなくなったIP別レート状態が溜まり続けないよう定期的に掃除する。
    this._rateCleanupTimer = setInterval(() => this._cleanupRateState(), RATE_STATE_CLEANUP_INTERVAL_MS);
    if (typeof this._rateCleanupTimer.unref === 'function') this._rateCleanupTimer.unref();

    this._heartbeatTimer = setInterval(() => this._heartbeat(), this.heartbeatIntervalMs);
    if (typeof this._heartbeatTimer.unref === 'function') this._heartbeatTimer.unref();

    this._roomCleanupTimer = setInterval(() => this._cleanupIdleRooms(), ROOM_CLEANUP_INTERVAL_MS);
    if (typeof this._roomCleanupTimer.unref === 'function') this._roomCleanupTimer.unref();

    return this;
  }

  /** 前回のpingに応答しなかった接続を破棄し、残りに次のpingを送る。
   * ブラウザもexe版(WebView2)もpingにはWebSocketの仕組みとして自動で
   * pongを返すので、クライアント側の対応は不要。 */
  _heartbeat() {
    for (const ws of this.wss.clients) {
      if (ws._isAlive === false) {
        ws.terminate();
        continue;
      }
      ws._isAlive = false;
      try { ws.ping(); } catch { /* noop */ }
    }
  }

  /** 配信者も視聴者も繋がっていないまま一定時間経った部屋を削除する。 */
  _cleanupIdleRooms(now = Date.now()) {
    for (const [roomId, room] of this.rooms) {
      if (room.broadcasterConn || room.viewerConns.size > 0) continue;
      if (now - room.lastActiveAt <= this.roomIdleTtlMs) continue;
      if (room.passphrase && this.passphrases.get(room.passphrase) === roomId) {
        this.passphrases.delete(room.passphrase);
      }
      this.rooms.delete(roomId);
      this.logger.log(`[relay] room ${roomId}: removed after being idle`);
    }
  }

  /** ブラウザ版(ReaTap Web)からの接続かどうか。ReaTap Webはこのサーバー
   * 自身が配信しているページなので、WebSocketのOriginのホストが接続先
   * (Hostヘッダー)と一致する。exe版(Tauri)のOriginはhttp://tauri.localhost
   * 等になるので一致しない。 */
  _isWebViewerRequest(req) {
    const origin = req && req.headers && req.headers.origin;
    const host = req && req.headers && req.headers.host;
    if (!origin || !host) return false;
    try {
      return new URL(origin).host === host;
    } catch {
      return false;
    }
  }

  /** ReaTap Web用の静的ファイル配信。WebSocketのアップグレード要求はここに
   * 来ない(wsライブラリがhttp.Serverの'upgrade'イベント側で横取りする
   * ため、この'request'イベントハンドラには通常のHTTPリクエストしか
   * 来ない)。 */
  _handleHttpRequest(req, res) {
    if (!this.staticDir) {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Not Found');
      return;
    }
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { 'Content-Type': 'text/plain; charset=utf-8', Allow: 'GET, HEAD' });
      res.end('Method Not Allowed');
      return;
    }

    let pathname;
    try {
      pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    } catch {
      res.writeHead(400, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Bad Request');
      return;
    }
    if (pathname === '/') pathname = '/index.html';

    // NUL文字(%00)を含むパスをfs.readFileに渡すと例外が同期的に投げられ、
    // 誰も受け止めないためプロセスごと落ちてしまう(GET /%00だけで中継
    // サーバーを止められる不具合があった)。ここで先に弾く。
    if (pathname.includes('\0')) {
      res.writeHead(400, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Bad Request');
      return;
    }

    // "../"等でstaticDirの外に出ようとするパストラバーサル対策。path.join
    // した後の絶対パスが必ずstaticDir配下に収まっていることを確認する
    // (path.normalizeだけでは"../"を残したままにするOSもあるため、最終的な
    // 絶対パスでの前方一致チェックが最も確実)。
    const filePath = path.join(this.staticDir, pathname);
    const staticDirWithSep = this.staticDir + path.sep;
    if (filePath !== this.staticDir && !filePath.startsWith(staticDirWithSep)) {
      res.writeHead(403, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Forbidden');
      return;
    }

    const notFound = () => {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Not Found');
    };
    // 上のNUL文字チェック以外の理由でfs.readFileが同期的に例外を投げても
    // プロセスが落ちないよう、念のため受け止めておく。
    try {
      fs.readFile(filePath, (err, data) => {
        if (err) return notFound();
        const ext = path.extname(filePath).toLowerCase();
        const contentType = MIME_TYPES[ext] || 'application/octet-stream';
        res.writeHead(200, { 'Content-Type': contentType, 'Cache-Control': 'no-cache' });
        res.end(req.method === 'HEAD' ? undefined : data);
      });
    } catch {
      notFound();
    }
  }

  stop() {
    return new Promise((resolve) => {
      for (const key of ['_rateCleanupTimer', '_heartbeatTimer', '_roomCleanupTimer']) {
        if (this[key]) {
          clearInterval(this[key]);
          this[key] = null;
        }
      }
      if (!this.wss) return resolve();
      // ws の Server#close() は「新規接続の受付を止める」だけで、既存の接続が
      // 自然にcloseするまで待ち続けてしまう。サーバーを止める操作としては
      // 接続中のクライアントも強制的に切断すべきなので、ここで能動的に閉じる。
      for (const client of this.wss.clients) {
        try { client.terminate(); } catch { /* noop */ }
      }
      this.wss.close(() => {
        // wsはserverオプションで渡されたhttp.Serverの所有権を持たない
        // (自分で作ったものではないため、close()しても自動では閉じない)。
        // このRelayServerがstart()で自分で作ったhttp.Serverなので、
        // ここで明示的に閉じてやる必要がある(閉じないとNodeプロセスが
        // 終了できずテストがハングする)。
        if (this.httpServer) {
          this.httpServer.close(() => resolve());
        } else {
          resolve();
        }
      });
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
    ws._isAlive = true;
    ws.on('pong', () => {
      ws._isAlive = true;
    });

    // ブラウザ版は同一IPから1接続まで。既に同じIPからの接続があれば、
    // 古い方を切って新しい方を残す。以前は新しい方を断っていたが、スマホで
    // アプリを切り替えて戻った時など、サーバーがまだ気づいていない切れた
    // 接続が残っていると本人の繋ぎ直しまで断られ、リアクション画面のまま
    // 黙って送信できなくなる不具合があった(2026-10-07の配信中に報告)。
    // 切られた側には'replaced'を送り、ReaTap Web側はこれを受けたら自動の
    // 再接続をやめる(2つのタブがお互いを切り合い続けないようにするため)。
    if (this._isWebViewerRequest(req)) {
      const existing = this.webConnByIp.get(ws._ip);
      if (existing && existing !== ws) {
        this._send(existing, {
          type: 'error',
          code: 'replaced',
          message:
            '同じネットワークの別のタブ・端末からReaTap Webで接続されたため、こちらは切断しました',
        });
        try { existing.close(); } catch { /* noop */ }
        this.logger.log('[relay] web viewer: replaced an older connection from the same IP');
      }
      this.webConnByIp.set(ws._ip, ws);
      ws.on('close', () => {
        if (this.webConnByIp.get(ws._ip) === ws) this.webConnByIp.delete(ws._ip);
      });
    }

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
      case 'disabledReactions':
        return this._handleDisabledReactions(ws, msg);
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
        // 配信者がOFFにしているリアクションのID一覧(視聴者側でボタンを灰色に
        // するために中継する。中継サーバー自身はこれで転送を止めたりはしない)
        disabledReactions: [],
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
    room.lastActiveAt = Date.now();
    ws._role = 'viewer';
    ws._roomId = roomId;
    ws._viewerId = `v_${Math.random().toString(36).slice(2, 10)}`;

    this._send(ws, { type: 'joined', roomId });
    this._send(ws, { type: 'disabledReactions', ids: room.disabledReactions });
    if (room.broadcasterConn) {
      this._send(room.broadcasterConn, { type: 'viewerCount', count: room.viewerConns.size });
    }
    this.logger.log(`[relay] room ${roomId}: viewer joined (${room.viewerConns.size} total)`);
  }

  _handleDisabledReactions(ws, msg) {
    if (ws._role !== 'broadcaster' || !ws._roomId) {
      return this._send(ws, { type: 'error', code: 'not_registered', message: '先にregisterしてください' });
    }
    const ids = msg.ids;
    if (
      !Array.isArray(ids) ||
      ids.length > MAX_DISABLED_REACTIONS ||
      !ids.every((id) => typeof id === 'string' && ALLOWED_EMOJI_ID_RE.test(id))
    ) {
      return this._send(ws, { type: 'error', code: 'invalid_params', message: 'idsが不正です' });
    }
    const room = this.rooms.get(ws._roomId);
    if (!room) return;
    room.disabledReactions = [...new Set(ids)];
    for (const viewer of room.viewerConns) {
      this._send(viewer, { type: 'disabledReactions', ids: room.disabledReactions });
    }
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
    room.lastActiveAt = now;

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
    room.lastActiveAt = Date.now();

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
