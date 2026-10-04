'use strict';
/**
 * 中継サーバーへの接続クライアント(Electron版 shared/relay-client.js のブラウザ移植版)。
 * Node.jsの'ws'パッケージの代わりに、webview標準のWebSocket APIを直接使う。
 * イベント購読のインターフェース(on/emit)はElectron版と同じ形にして、
 * 呼び出し側のコード(bar.js等)をほぼそのまま使い回せるようにしている。
 */

// 過去バージョンのアプリが新しい中継サーバーに(逆もまた然り)接続してしまい、
// 想定と違うメッセージ形式で誤動作するのを防ぐための番号。register/joinの
// メッセージ形式など、互換性を壊す変更をした時だけ上げる(関連: relay-server/server.js
// のMIN_PROTOCOL_VERSION)。呼び出し側が個別に付与する必要が無いよう、
// ここでhelloメッセージに自動的に混ぜ込む。
const PROTOCOL_VERSION = 2;

class RelayClient {
  constructor({ url, hello, reconnectDelayMs = 3000, maxReconnectDelayMs = 30000 }) {
    this.url = url;
    this.hello = hello;
    this.reconnectDelayMs = reconnectDelayMs;
    this.maxReconnectDelayMs = maxReconnectDelayMs;
    this._currentDelay = reconnectDelayMs;
    this.ws = null;
    this._closedByUser = false;
    this._reconnectTimer = null;
    this._listeners = new Map(); // eventName -> Set<fn>
  }

  on(event, fn) {
    if (!this._listeners.has(event)) this._listeners.set(event, new Set());
    this._listeners.get(event).add(fn);
    return this;
  }

  off(event, fn) {
    this._listeners.get(event)?.delete(fn);
    return this;
  }

  emit(event, payload) {
    for (const fn of this._listeners.get(event) || []) {
      try {
        fn(payload);
      } catch (err) {
        console.error(`[RelayClient] リスナーでエラー(${event}):`, err);
      }
    }
  }

  connect() {
    this._closedByUser = false;
    this._open();
    return this;
  }

  _open() {
    const ws = new WebSocket(this.url);
    this.ws = ws;

    ws.addEventListener('open', () => {
      this._currentDelay = this.reconnectDelayMs;
      this.emit('open');
      if (this.hello) this.send({ ...this.hello, protocolVersion: PROTOCOL_VERSION });
    });

    ws.addEventListener('message', (evt) => {
      let msg;
      try {
        msg = JSON.parse(evt.data);
      } catch {
        this.emit('protocol-error', new Error('サーバーから不正なJSONを受信しました'));
        return;
      }
      this.emit('message', msg);
      if (msg && typeof msg.type === 'string') {
        this.emit(`type:${msg.type}`, msg);
      }
    });

    ws.addEventListener('close', () => {
      this.emit('close');
      if (!this._closedByUser) this._scheduleReconnect();
    });

    ws.addEventListener('error', (err) => {
      this.emit('error', err);
    });
  }

  _scheduleReconnect() {
    this.emit('reconnecting', this._currentDelay);
    clearTimeout(this._reconnectTimer);
    this._reconnectTimer = setTimeout(() => {
      if (!this._closedByUser) this._open();
    }, this._currentDelay);
    this._currentDelay = Math.min(this._currentDelay * 2, this.maxReconnectDelayMs);
  }

  send(obj) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(obj));
      return true;
    }
    return false;
  }

  close() {
    this._closedByUser = true;
    clearTimeout(this._reconnectTimer);
    if (this.ws) {
      try {
        this.ws.close();
      } catch {
        /* noop */
      }
    }
  }
}

// send()を直接呼んでregisterを送り直す(合言葉の変更等)場合など、呼び出し側が
// 自分でprotocolVersionを付与したい時のために公開しておく。
RelayClient.PROTOCOL_VERSION = PROTOCOL_VERSION;

window.RelayClient = RelayClient;
