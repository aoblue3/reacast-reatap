# ReaCast / ReaTap

PeerCastなどの配信で使う、**リアクション**と**コメント読み上げ・字幕**のアプリです。

- **ReaCast**(配信者用): 視聴者から届いた絵文字リアクションを、配信画面にアニメーションで表示します。掲示板・YouTube・Twitchのコメントの字幕表示・読み上げ・OBS表示もできます
- **ReaTap**(視聴者用): pcwmp / PCRPlayerの画面のそばに絵文字のボタンを出して、配信者にリアクションを送れます
- **ReaTap Web**(視聴者用・ブラウザ版): exeを使わずに、ブラウザ(スマホでもOK)で合言葉を入れるだけでリアクションを送れます

使い方の詳しい説明は [使い方ガイド](./使い方ガイド.md)、更新内容は [CHANGELOG](./CHANGELOG.md) を見てください。

## ダウンロード

[Releases](https://github.com/aoblue3/reacast-reatap/releases/latest) から、使う方のexeをダウンロードしてください。

| ファイル | 使う人 |
|---|---|
| `ReaCast.exe` | 配信する人 |
| `ReaTap.exe` | 配信を見る人 |

インストールは不要です。好きな場所に置いて、ダブルクリックで起動します。使うのをやめる時はexeを削除するだけです。新しいバージョンが出ると、起動時に更新するか聞かれます。

**動作環境**: Windows 10(バージョン1803以降)/ Windows 11

## 初めてダウンロード・起動する時に出る警告について

このアプリはコード署名(有料の証明書)をしていないため、**初めてダウンロード・起動する時にWindowsやブラウザが警告を出します。** 新しいバージョンが出た直後は特に出やすいです。どれも「まだあまり知られていないファイル」という意味の警告で、正常です。

### ① Edgeで「一般的にダウンロードされていません」と出る

1. ダウンロード欄(右上の↓アイコン)を開く
2. ファイルの行にマウスを乗せて「…」→ **「保存」**
3. 「詳細表示」→ **「保持する」**

Chromeで「安全でない可能性があります」と出た時も、ダウンロード欄から「保存」(または「継続」)を選べばダウンロードできます。

### ② 起動すると青い画面で「Windows によって PC が保護されました」と出る

**「詳細情報」→「実行」** を押すと起動できます。「詳細情報」を押した時に「発行元: 不明な発行元」と出るのは正常です。

### こういう表示が出た時は使わずに連絡してください

- Windows セキュリティ(Defender)が **「脅威が見つかりました」** と通知してくる
- **「Trojan:〜」「Behavior:〜」** などの名前が表示される
- ダウンロードしたexeが勝手に消された(隔離された)

### 本物のファイルか確かめたい時

Releasesには、exeと一緒に `.sha256` ファイル(exeの指紋のような値)を置いています。PowerShellで次のコマンドを実行して、表示された値が `.sha256` の中身と同じなら、配布したものと同じファイルです。

```powershell
Get-FileHash $env:USERPROFILE\Downloads\ReaCast.exe
```

ソースコードはすべてこのリポジトリで公開していて、配布しているexeはGitHub Actionsでこのソースから自動で作っています。

## 使い始めるまで(ざっくり)

1. 配信者がReaCastを起動すると「合言葉」が決まります
2. 配信者が合言葉を視聴者に伝えます
3. 視聴者がReaTap(またはReaTap Web)で合言葉を入力します
4. 視聴者が絵文字を押すと、配信者の画面にリアクションが飛びます

OBSに出す場合は、OBSの「ブラウザ」ソースに `http://127.0.0.1:18772/`(リアクション)を追加してください。字幕・レス一覧などのURLは、ReaCastのコメント設定の「かんたん設定」タブにあります。

## できること

### ReaCast(配信者用)
- **リアクション表示**: 届いた絵文字を、絵文字ごとに違う動き(ねこが他のリアクションを追いかける、🌈の虹、🍙が転がる など)で表示。OBSのブラウザソースにも出せます
- **コメントの取得**: 2ちゃんねる互換の掲示板(jpnkn等)・したらば・YouTube Live・Twitchを同時に取得。次スレへの自動移動、落ちた時の続きから読み上げ、スパチャ・ビッツの強調表示
- **読み上げ**: VOICEVOX・COEIROINK・棒読みちゃん・任意の読み上げソフト。声のランダム、溜まったら速く読む(ターボ)、NGワード・読み方の登録、ホットキー
- **字幕・レス一覧・ニコ生風表示**: OBS用のページとデスクトップ字幕。たくさんのプリセット(P5風・FF風・DQ風・羽ペン・ネオン・オーロラなど)と出方のアニメーション、フォントの選択
- **話した字幕**: 配信者が話した言葉を字幕にして、自動で翻訳。話した言葉でリアクションを飛ばすこともできます
- **リアクション連動**: コメントや話した言葉に合わせて、自動でリアクションを飛ばします

### ReaTap(視聴者用)
- pcwmp / PCRPlayerのウィンドウを自動で見つけて、そのそばにリアクションバーを表示
- 複数の配信者を同時に登録・接続
- ホットキーでリアクションを送る、表示するリアクションの選択・並べ替え

> **ご注意**: YouTube LiveとTwitchのコメントは、公式のAPIではなく非公式の方法で取得しています。各サイトの仕様変更で、予告なく取得できなくなることがあります。

## 困った時は

[使い方ガイドの「困ったときは」](./使い方ガイド.md#困ったときは) を見てください。解決しない時は、何をしたらどうなったかと、ログファイル(アプリの「ログフォルダを開く」から開けます)を添えて連絡してください。

---

## 開発者向け

### 構成

```
broadcaster-app-tauri/  ReaCast(Tauri v2 + Rust)
viewer-app-tauri/       ReaTap(Tauri v2 + Rust)
web-viewer/             ReaTap Web(静的ファイル。relay-serverが配信する)
relay-server/           中継サーバー(Node.js、WebSocket)。配信者ごとに「部屋」を持つ
```

### ビルド

Windows上で、[Rust](https://www.rust-lang.org/ja) と Node.js を用意してください。詳しくは [WINDOWSでのテスト手順](./WINDOWSでのテスト手順.md) を参照してください。

```bash
cd broadcaster-app-tauri  # viewer-app-tauriも同様
npm install
npm run dev    # 開発中の動作確認
npm run build  # src-tauri/target/release/ReaCast.exe ができる
```

中継サーバーのアドレスは、ビルド時に環境変数 `REACTION_RELAY_HOST` / `REACTION_RELAY_PORT` で指定します(未指定だとダミーの `relay.example.invalid` になります)。自動アップデートは、CIでのビルド時に埋め込まれる `APP_VERSION` / `UPDATE_CHECK_REPO` がある時だけ有効です(手元のビルドでは無効)。

### 中継サーバー

```bash
cd relay-server
npm install
PORT=39200 node server.js
```

同じポートでReaTap Web(`web-viewer/`)も配信されます。中継サーバーとReaTap Webはリリースの自動配布の対象外なので、変更した時はサーバー側で手動で反映してください。

### リリース

`vX.Y.Z` のタグをpushすると、GitHub Actions(`.github/workflows/build-release.yml`)がReaCast.exe / ReaTap.exeと `.sha256` を作ってReleasesに添付します。自動アップデートはバージョン番号を比較するので、必ず前回より新しい番号にしてください。

### 絵文字の追加

`broadcaster-app-tauri/frontend/shared/emoji-set.js`、`viewer-app-tauri/frontend/shared/emoji-set.js`、`web-viewer/shared/emoji-set.js` の3つは同じ内容にしてください。`id` は英数字と `_-` のみです。

### テスト

```bash
(cd broadcaster-app-tauri/src-tauri && cargo test)
(cd viewer-app-tauri/src-tauri && cargo test)
(cd relay-server && npm install && node test/e2e.test.js)
```

## ライセンス

[MIT License](./LICENSE)
