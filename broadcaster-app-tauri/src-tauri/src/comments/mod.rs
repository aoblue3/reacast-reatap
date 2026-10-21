//! コメント読み上げ・字幕機能(SpeechCastMe + unacastの置き換え)。
//!
//! 掲示板(jpnkn等の2ch互換・したらば)の新着レスを取得し、1本の順番待ち列に
//! 並べて、1件ずつ「字幕を表示 → 読み上げコマンド(vrx.exe等)に渡す →
//! 読み上げ間隔だけ待つ」を繰り返す。字幕(下)・レス一覧(右)・デスクトップ
//! 字幕は、どれもobs_bridgeのWebSocketから同じメッセージを受け取って描画する
//! ので、表示のタイミングがずれない。
//!
//! 取得・順番待ち・コマンド実行はすべてRust側(このモジュール)で行う。
//! コントロールパネル等の画面を隠していてもWebViewのタイマー抑制の影響を
//! 受けずに動き続けるようにするため。

pub mod hotkeys;
pub mod jpnkn_fast;
pub mod source;
pub mod twitch;
pub mod youtube;
pub mod text;

use crate::config_store::ConfigStore;
use crate::obs_bridge::ObsBridge;
use serde::{Deserialize, Serialize};
use source::{Board, ThreadReader, ThreadRef};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

pub const SETTINGS_KEY: &str = "commentSettings";

/// 読み上げソフトごとの話速・音高などの調整(ランダムで別々のソフトの声を混ぜた時に、
/// ソフトごとに音量や速さを揃えられるように)
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TtsParams {
    pub speed: f64,
    pub pitch: f64,
    pub intonation: f64,
    pub volume: f64,
    pub pre_silence: f64,
    pub post_silence: f64,
    pub pause_scale: f64,
}

impl Default for TtsParams {
    fn default() -> Self {
        Self { speed: 1.0, pitch: 0.0, intonation: 1.0, volume: 1.0, pre_silence: 0.1, post_silence: 0.1, pause_scale: 1.0 }
    }
}

/// ランダムに使う声の1つ(読み上げソフト・声・割合)。別々のソフトの声を混ぜられる
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct RandomVoice {
    /// CommentSettings::speech_engineと同じ値(VOICEVOX互換・COEIROINK v2だけ)
    pub engine: String,
    pub speaker: String,
    /// 割合(重み。合計が100でなくてもよい)
    pub weight: u32,
}


#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentSettings {
    // ---- 取得 ----
    pub thread_url: String,
    pub poll_interval_ms: u64,
    pub auto_next_thread: bool,
    pub next_thread_keyword: String,
    /// このレス番号に達したら一度だけ「次スレを立ててください」と知らせる(0で知らせない)
    pub thread_warn_count: u32,
    pub start_on_launch: bool,
    /// jpnknの板では即時通知(Fastインターフェース)も使って、書き込まれた瞬間に拾う
    pub jpnkn_fast: bool,
    /// スレッドの>>1(テンプレート)は字幕・読み上げ・右のレス一覧に出さない
    /// (長いと字幕の文字が小さくなり、読み終わるまで次のレスが待たされるため)
    pub skip_first_post: bool,
    /// 「スレッドのレス」タブから書き込む時の名前・メール欄
    pub post_name: String,
    pub post_mail: String,
    /// 開始時に右のレス一覧へ載せるレスの開始番号(0なら載せない。読み上げはしない)
    pub start_res_no: u32,
    /// 読み上げの途中でPCやReaCastが落ちた時、次に同じスレッドを開始したら、
    /// 前回読み終わったレスの次から読み上げる(「停止」を押して止めた時と、
    /// 1時間以上たった時は続きから読まない)
    pub resume_reading: bool,
    /// 開始時に右のレス一覧へ出すお知らせ(空なら出さない)
    pub initial_text: String,
    /// 右のレス一覧に出すアイコン画像のフォルダ(掲示板のレス用。中の画像からランダムに選ぶ)
    pub icon_folder_bbs: String,
    pub icon_folder_youtube: String,
    pub icon_folder_twitch: String,
    // ---- 配信サイトのコメント(空欄なら取得しない) ----
    /// YouTubeの配信URL・動画ID・チャンネルURL・@ハンドル
    pub youtube_target: String,
    /// Twitchのチャンネル名(またはURL)
    pub twitch_channel: String,
    // ---- 次スレの自動作成 ----
    pub auto_create_thread: bool,
    /// このレス番号に達したら次スレを建てる
    pub create_thread_count: u32,
    /// {next}=今のタイトルの最後の数字を1増やしたもの、{n}=その数字だけ
    pub create_title_template: String,
    pub create_name: String,
    pub create_mail: String,
    /// {prevUrl}=今のスレッドのURL、{prevTitle}=今のタイトル、{title}=新しいタイトル
    pub create_body_template: String,
    // ---- 読み上げ ----
    pub speech_enabled: bool,
    pub command_path: String,
    pub command_args: String,
    pub read_interval_ms: u64,
    pub turbo_interval_ms: u64,
    /// 順番待ちがこの件数以上溜まったらターボ(短い間隔)にする
    pub turbo_threshold: usize,
    /// ターボ中の話速の倍率(VOICEVOX等ReaCastが鳴らすソフトだけ)。ターボ中も声は
    /// 最後まで読み、話す速さを上げて追いつく
    pub turbo_speed_scale: f64,
    pub read_res_number: bool,
    pub max_chars: usize,
    /// 読み上げにかかる時間の目安(1文字あたりのミリ秒、0で使わない)。読み上げが
    /// 終わるまで次のレスに進まず、字幕もその間は表示しておく(読み上げソフトの
    /// 「ただちに再生」をOFFにしている時に、字幕と声がずれないようにするため)
    pub speech_ms_per_char: u64,
    /// 1件のレスで待つ最大の時間(ミリ秒、0で無制限)。長いレスは読み上げをこの時間に
    /// 収まる所で打ち切って(「以下略」)、字幕もこの時間で次のレスへ進む
    pub max_wait_ms: u64,
    /// 読み上げソフト(tts.rs)。"command"=コマンドで渡す(民安★TALK・SofTalkなど)、
    /// "voicevox"/"coeiroinkv1"/"sharevox"/"aivisspeech"/"voicevoxcompat"=VOICEVOX互換、
    /// "coeiroink"=COEIROINK v2、"bouyomi"=棒読みちゃん
    pub speech_engine: String,
    /// 読み上げソフトの接続先(空欄ならそのソフトの既定のアドレス)
    pub tts_url: String,
    /// 読み上げソフト(VOICEVOX等・棒読みちゃん)が起動していなければ自動で起動する
    pub tts_auto_launch: bool,
    /// (v1.5.12の設定。読み込み時にtts_exe_pathsへ移す)
    #[serde(skip_serializing)]
    pub tts_exe_path: String,
    /// 自動で起動するexeの場所(読み上げソフトごと。無ければ起動中のプロセス・
    /// 標準のインストール先から探す)
    pub tts_exe_paths: std::collections::BTreeMap<String, String>,
    /// 声をランダムにする(民安★TALKのランダム再生と同じ。割合はtts_random_voicesのweight)
    pub tts_random: bool,
    pub tts_random_voices: Vec<RandomVoice>,
    /// ランダムでも、同じ人(掲示板はID、配信サイトは名前)には毎回同じ声を使う
    pub tts_random_by_person: bool,
    /// 声(VOICEVOX互換はスタイルの番号、COEIROINK v2は「UUID:スタイル番号」)。
    /// YouTube・Twitchのコメントは別の声にもできる(空欄なら共通の声)
    pub tts_speaker: String,
    pub tts_speaker_youtube: String,
    pub tts_speaker_twitch: String,
    /// 読み上げソフトごとの調整(無いソフトは下のtts_speed等を使う)
    pub tts_engine_params: std::collections::BTreeMap<String, TtsParams>,
    /// 話速・音高・抑揚・音量・開始無音(秒)・終了無音(秒)・間の長さ(v1.5.13までの共通の調整)
    pub tts_speed: f64,
    pub tts_pitch: f64,
    pub tts_intonation: f64,
    pub tts_volume: f64,
    pub tts_pre_silence: f64,
    pub tts_post_silence: f64,
    pub tts_pause_scale: f64,
    /// 音の出力先(空欄ならWindowsの既定。仮想オーディオデバイスを選べばOBSで別に取り込める)
    pub tts_device: String,
    /// 棒読みちゃんの声(0=棒読みちゃんの設定どおり)・速度・音程・音量(-1=棒読みちゃんの設定どおり)
    pub bouyomi_voice: i32,
    pub bouyomi_speed: i32,
    pub bouyomi_tone: i32,
    pub bouyomi_volume: i32,
    /// 読み上げの「スキップ」「一時停止/再開」のホットキー(空なら使わない。hotkeys.rs)
    pub hotkey_skip: String,
    pub hotkey_pause: String,
    /// 話した字幕のマイクを止める / 再開するホットキー(別々のキー。voice.rs)
    pub hotkey_voice_mute: String,
    pub hotkey_voice_unmute: String,
    pub reading_rules: String,
    /// v1.5.6で増やした既定の読み方を足し終えたか。保存済みの設定に無い時は
    /// false(=これから足す)、新しく作った設定ではtrue(load_settings参照)
    #[serde(default)]
    pub reading_presets_v2: bool,
    /// v1.5.20で増やした既定のリアクション連動(全部の絵文字に1つ以上)を足し終えたか
    /// (reading_presets_v2と同じ仕組み)
    #[serde(default)]
    pub reaction_presets_v2: bool,
    /// v1.6.3で🌈を足した時の、既定のルールの追加(reaction_presets_v2と同じ仕組み)
    #[serde(default)]
    pub reaction_presets_v3: bool,
    pub ng_words: String,
    // ---- コメントとリアクションの連動 ----
    /// 字幕に出したレスの本文がルールに当てはまったら、そのリアクションを飛ばす
    pub reaction_link: bool,
    /// 1行に「正規表現/リアクションのID」(text::parse_reaction_rules)
    pub reaction_rules: String,
    /// 1件のレスで飛ばすリアクションの最大数
    pub reaction_link_max: usize,
    /// 下の字幕に出したレスを翻訳して下に添える(日本語→英語、それ以外→日本語。読み上げない)
    pub subtitle_translate: bool,
    /// 日本語のレスの翻訳先(Google翻訳の言語コード。既定は英語)。日本語以外のレスは日本語に訳す
    pub subtitle_translate_target: String,
    // ---- 話した字幕(voice.rs) ----
    /// 話した字幕を使う(ONの間、Edgeで音声認識の窓を開いておく)
    pub voice_enabled: bool,
    /// 聞き取る言葉(例: "ja-JP")
    pub voice_lang: String,
    /// 音声認識に使うブラウザ: "auto"(Chromeがあれば優先) / "chrome" / "edge"。
    /// Edge(Microsoftの音声認識)は精度が高めだが、約10時間で使えなくなることがある
    pub voice_browser: String,
    /// 話した言葉を翻訳する / 翻訳先の言語(Google翻訳の言語コード。例: "en")
    pub voice_translate: bool,
    pub voice_target_lang: String,
    /// 話した言葉(確定した1文)が「リアクション連動」のルールに当てはまったら、リアクションを飛ばす
    pub voice_reaction_link: bool,
    /// 話した言葉で飛んだリアクションに印(水色の光)を付ける(リスナーのリアクションと見分ける)
    pub voice_reaction_mark: bool,
    // ---- 字幕 ----
    pub display_ms: u64,
    /// 長いレスほど長く表示する: 1文字あたりに足す時間(0で長さに関係なく一定)
    pub display_per_char_ms: u64,
    /// 文字数で延ばした場合の表示時間の上限
    pub display_max_ms: u64,
    /// 字幕を出し終わるまで次のレスを待つ(ターボ中は待たない)
    pub wait_for_display: bool,
    /// 下の字幕で、>>番号のアンカーが付いたレスの前に、アンカー先のレスを先に表示する
    pub anchor_preview: bool,
    pub anchor_preview_ms: u64,
    pub aa_display_ms: u64,
    // ---- AAモード ----
    pub aa_enabled: bool,
    pub aa_threshold_chars: usize,
    pub aa_patterns: String,
    pub aa_read_quoted_only: bool,
    pub aa_read_res_number: bool,
    pub aa_fixed_reading: String,
    // ---- デスクトップ字幕 ----
    pub desktop_enabled: bool,
    pub desktop_monitor_id: Option<String>,
    pub desktop_x: f64,
    pub desktop_y: f64,
    pub desktop_width: f64,
    pub desktop_height: f64,
    /// マウスで動かして決めた位置・大きさ(物理ピクセル)。あればモニター・割合の
    /// 指定より優先する。
    pub desktop_rect: Option<DesktopRect>,
    /// 見た目の設定(フォント・色・縁など)。Rust側では中身を解釈せず、そのまま
    /// 字幕・レス一覧の画面に渡す(画面側が既定値を補う)。
    pub style: serde_json::Value,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 既定の読み方(上から順に置き換える)。先頭2つはURLで、以前の既定(OLD_URL_READINGS)と
/// 同じ並び。行を足したら、既存の人にはadd_reading_presetsで一度だけ足される。
const DEFAULT_READING_RULES: &[&str] = &[
    r"(https|ttps)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/ユーアールエル省略",
    r"(http|ttp)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/ユーアールエル省略",
    // アンカー(>>12 や >>12-15)は読まない
    r"[>＞≫]{1,2}[0-9０-９]+([-ー][0-9０-９]+)?/",
    // 6桁以上の数字
    r"[0-9０-９]{6,}/数字省略",
    "ww+/ワラワラ",
    "ｗｗ+/ワラワラ",
    r"^[wｗ]$/ワラ",
    r"^[8８]{3,}$/パチパチ",
    // 記号や伸ばし棒の連続は1つに
    r"[!！]{2,}/！",
    r"[?？]{2,}/？",
    r"[ー〜~]{3,}/ー",
    // 絵文字は読まない(読み上げソフトが名前を読み上げてしまうため)
    r"[\x{1F300}-\x{1FAFF}\x{2600}-\x{27BF}\x{FE0F}]/",
    // ネットスラング
    "(?i)orz/がっくり",
    "(?i)kwsk/くわしく",
    "(?i)wktk/わくてか",
    "(?i)ktkr/きたこれ",
    "(?i)gdgd/ぐだぐだ",
    "(?i)ggrks/ぐぐれかす",
];

/// v1.5.5までの既定のURLの読み方(DEFAULT_READING_RULESの先頭2つに置き換える)
const OLD_URL_READINGS: &[&str] = &[
    r"(https|ttps)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/リンク",
    r"(http|ttp)(:¥/¥/[-_.!~*¥'()a-zA-Z0-9;¥/?:¥@&=+¥$,%#]+)/リンク",
];

/// コメントとリアクションの連動の既定のルール(上にあるものほど優先)。
/// IDはshared/emoji-set.jsのもの。誤爆しやすい短い言葉は「^…$」(レス全体がその言葉)にしている。
const DEFAULT_REACTION_RULES: &[&str] = &[
    // 笑い
    "[wｗWＷ]{3,}|^草+$|大草原/kusa",
    "ワロタ|ﾜﾛﾀ|わろた|爆笑|腹痛い|^笑$|ｗｗ笑/lol",
    // 拍手・お祝い
    "^[8８]{3,}$|パチパチ|ぱちぱち|拍手/clap",
    "おめでと|^おめ$|祝|ハッピーバースデー|誕生日/cake",
    "優勝|勝った|勝利|クリア|^GG$|^gg$|やったー/gg",
    "花火|たまや|かぎや/fireworks",
    // 気持ち
    "かわいい|可愛い|かわよ|カワイイ|好き|♡|❤/heart",
    "^えっ|^まじ[!！?？]|^マジ[!！?？]|^うそ[!！?？]|^嘘[!！?？]|^[!！][?？]$/shock",
    "胸熱|激アツ|激熱|アツい|熱い|神回|燃える|🔥/fire",
    "泣いた|泣ける|涙|うるっ|;;|；；|( ;∀;)/cry",
    "こわ[いっ]|怖い|ひえ|ヒエ|ぎゃあ|ギャー|ぎゃー/scary",
    "お願い|おねがい|頼む|たのむ/please",
    "むずい|難しい|うーん|考え中|^[?？]{2,}$/think",
    "眠い|ねむい|おやすみ|寝る|zzz|ｚｚｚ/sleepy",
    "死んだ|しんだ|ちーん|チーン|昇天|成仏/skull",
    // ほめる・応援
    "^ナイス|ないす|ﾅｲｽ|nice|ナイスー/nice",
    "がんば|頑張|ファイト|ふぁいと|ガンバ/ganbare",
    "^それな|それな[ｗw]/sorena",
    "^わかる|わかる[ｗw]|わかりみ/wakaru",
    "^うまい|上手い|うますぎ|うめえ/umai",
    "^神$|神[ｗw]|ゴッド|^GOD$/kami",
    "^OK$|^おけ$|^おk$|^りょ|^了解/ok",
    "的中|ヘッショ|ヘッドショット|ドンピシャ/headshot",
    // 挨拶
    "^乙|^おつ|お疲れ|おつかれ|^おつ[ー〜~]?$/otsu",
    // 食べ物・飲み物
    "乾杯|かんぱい|カンパイ|🍻|🍺/cheers",
    "ラーメン|らーめん|拉麺/ramen",
    "寿司|すし|スシ|🍣/sushi",
    "バナナ|ばなな/banana",
    // 動物など
    "わんわん|ワンワン|わんこ|ワンコ|🐶/dog",
    "ねこ|ネコ|猫|にゃー|ニャー|にゃん|ニャン|🐈/cat",
    "さくらんぼ|サクランボ|チェリー|🍒/cherry",
    "ぶーぶー|ブーブー|🐷/pig",
    "ペンギン|ぺんぎん|🐧/penguin",
    "カエル|ケロケロ|けろけろ|🐸/frog",
    "赤ちゃん|ばぶ|バブー|👶/baby",
    "うんこ|うんち|💩/poop",
    // 果物(v1.5.20で追加)
    "マンゴー|まんごー|🥭/mango",
    "^もも$|^桃$|ピーチ|桃食|🍑/peach",
    // 気持ち・ノリ(v1.5.20で追加)
    "怒った|怒る|怒って|激おこ|^おこ$|ぷんぷん|プンプン|むかつ|ムカつ|💢|😠|😡/angry",
    "ブーイング|ひっこめ|引っ込め|^帰れ|^かえれ|^ぶー$|^ブー$|👎/boo",
    "挑発|煽り|煽って|あおり|かかってこい|ざぁこ|ざーこ|^雑魚|^ざこ|^ザコ|^m9|😏/provoke",
    "天狗|てんぐ|イキり|イキって|イキる|調子に乗|調子乗|ドヤ顔|どや顔|ドヤァ|👺/tengu",
    "おふざけ|ふざけ|てへぺろ|テヘペロ|^てへ|^テヘ|🤪/zany",
    "パンチ|ゲンコツ|げんこつ|鉄拳|拳で|殴|👊/fist",
    // 時間・方向(v1.5.20で追加)
    "ストップウォッチ|タイム計|計測|RTA|タイマー|新記録|⏱/stopwatch",
    "こんな時間|もう朝|時報|遅刻|寝坊|間に合わ|🕔/clock5",
    "^上$|^うえ$|^上上|上だ[!！]|↑|⬆/arrowup",
    "^下$|^した$|下だ[!！]|↓|⬇/arrowdown",
    "^右$|^みぎ$|右だ[!！]|→|➡/arrowright",
    "^左$|^ひだり$|左だ[!！]|←|⬅/arrowleft",
    // その他
    "ドカン|どかーん|爆発|💣/bomb",
    "ゴール|サッカー|⚽/soccer",
    "爆竹|クラッカー|パンパン|ぱんぱん|🧨/firecracker",
    "虹|にじ|レインボー|🌈/rainbow",
    "^[ｷキ]ﾀ━|キタ━|きたあ|キター|キタ[ー━]+/fire",
];

impl Default for CommentSettings {
    fn default() -> Self {
        Self {
            thread_url: String::new(),
            poll_interval_ms: 7000,
            auto_next_thread: true,
            next_thread_keyword: String::new(),
            thread_warn_count: 975,
            start_on_launch: false,
            jpnkn_fast: true,
            skip_first_post: true,
            post_name: String::new(),
            post_mail: String::new(),
            start_res_no: 0,
            resume_reading: true,
            initial_text: "スレッド読み込みを開始しました".into(),
            icon_folder_bbs: String::new(),
            icon_folder_youtube: String::new(),
            icon_folder_twitch: String::new(),
            youtube_target: String::new(),
            twitch_channel: String::new(),
            auto_create_thread: false,
            create_thread_count: 975,
            create_title_template: "{next}".into(),
            create_name: String::new(),
            create_mail: String::new(),
            create_body_template: "前スレ\n{prevUrl}".into(),
            speech_enabled: true,
            command_path: r"C:\tamiyasuex\vrx.exe".into(),
            command_args: "#Res#".into(),
            read_interval_ms: 3000,
            turbo_interval_ms: 400,
            turbo_threshold: 3,
            turbo_speed_scale: 1.3,
            // 既定は本文だけを読み上げる(レス番号・名前は読まない)
            read_res_number: false,
            max_chars: 270,
            speech_ms_per_char: 160,
            max_wait_ms: 20000,
            speech_engine: "command".into(),
            tts_url: String::new(),
            tts_auto_launch: true,
            tts_exe_path: String::new(),
            tts_exe_paths: Default::default(),
            tts_random: false,
            tts_random_voices: Vec::new(),
            tts_random_by_person: false,
            tts_speaker: String::new(),
            tts_speaker_youtube: String::new(),
            tts_speaker_twitch: String::new(),
            tts_engine_params: Default::default(),
            tts_speed: 1.0,
            tts_pitch: 0.0,
            tts_intonation: 1.0,
            tts_volume: 1.0,
            tts_pre_silence: 0.1,
            tts_post_silence: 0.1,
            tts_pause_scale: 1.0,
            tts_device: String::new(),
            bouyomi_voice: 0,
            bouyomi_speed: -1,
            bouyomi_tone: -1,
            bouyomi_volume: -1,
            hotkey_skip: String::new(),
            hotkey_pause: String::new(),
            hotkey_voice_mute: String::new(),
            hotkey_voice_unmute: String::new(),
            reading_rules: DEFAULT_READING_RULES.join("\n"),
            reading_presets_v2: true,
            reaction_presets_v2: true,
            reaction_presets_v3: true,
            ng_words: String::new(),
            reaction_link: true,
            reaction_rules: DEFAULT_REACTION_RULES.join("\n"),
            reaction_link_max: 2,
            subtitle_translate: false,
            subtitle_translate_target: "en".into(),
            voice_enabled: false,
            voice_lang: "ja-JP".into(),
            voice_browser: "auto".into(),
            voice_translate: true,
            voice_target_lang: "en".into(),
            voice_reaction_link: false,
            voice_reaction_mark: true,
            display_ms: 3000,
            display_per_char_ms: 80,
            display_max_ms: 12000,
            wait_for_display: true,
            anchor_preview: true,
            anchor_preview_ms: 2500,
            aa_display_ms: 6000,
            aa_enabled: true,
            aa_threshold_chars: 300,
            aa_patterns: ["д", "（●）", "从", "）)ﾉヽ", "∀", "<●>", "（__人__）"].join("\n"),
            aa_read_quoted_only: true,
            aa_read_res_number: false,
            aa_fixed_reading: "アスキーアート".into(),
            desktop_enabled: true,
            desktop_monitor_id: None,
            // 画面の一番下だとタスクバーに重なるので、少し上(70%〜92%)を既定にする
            desktop_x: 0.0,
            desktop_y: 0.70,
            desktop_width: 1.0,
            desktop_height: 0.22,
            desktop_rect: None,
            style: serde_json::json!({}),
        }
    }
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommentStatus {
    pub running: bool,
    pub thread_title: Option<String>,
    pub thread_url: Option<String>,
    pub last_no: u32,
    pub queue_len: usize,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// 配信サイト(YouTube・Twitch)ごとの状態
    pub sources: Vec<SourceStatus>,
    /// 読み上げを一時停止中か(取得は続け、順番待ちに溜めておく)
    pub paused: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    /// "youtube" | "twitch"
    pub id: String,
    pub label: String,
    /// "ok"(取得中) | "wait"(配信待ち・再接続中) | "error"
    pub state: String,
    pub message: String,
}

/// エラー・警告・配信サイトの状態が変わった時だけログに残す(定期取得のたびに同じ行を書かない)
fn log_status_change(before: &CommentStatus, after: &CommentStatus) {
    if after.error != before.error {
        if let Some(e) = &after.error {
            log::warn!("コメント取得のエラー: {e}");
        }
    }
    if after.warning != before.warning {
        if let Some(w) = &after.warning {
            log::info!("コメント取得の警告: {w}");
        }
    }
    for src in &after.sources {
        let prev = before.sources.iter().find(|x| x.id == src.id);
        if prev.map_or(true, |p| p.state != src.state || p.message != src.message) {
            log::info!("{}: {} {}", src.label, src.state, src.message);
        }
    }
}

#[derive(Clone)]
struct Post {
    no: u32,
    name: String,
    date: String,
    body: String,
    /// ReaCast自身からのお知らせ(次スレ移動・スレ建て警告・テスト表示)
    system: bool,
    /// 右のレス一覧に出すアイコン画像のURL(obs_bridgeが配信する)
    icon: Option<String>,
    /// 取得元: "bbs" | "youtube" | "twitch" | "system"
    source: &'static str,
    /// 配信サイトのコメントを書いた人のアイコン画像のURL(レス番号の位置に出す)
    avatar: Option<String>,
    /// 「前回の続き」「ここから読み始める」で入れ直したレス。溜まっていてもターボに
    /// しない(追いつくために声を途中で切らず、普通の速さで読む)
    catch_up: bool,
    /// スーパーチャット・ビッツ等の応援(右のレス一覧で目立たせる)
    paid: Option<Paid>,
}

/// 配信サイトの応援(YouTubeのスーパーチャット・スーパーステッカー・メンバーシップ、
/// Twitchのビッツ・サブスク・ギフト・レイド)
#[derive(Clone, Debug)]
struct Paid {
    kind: &'static str,
    /// 表示する金額・種類(例: "¥500" "100ビッツ" "サブスク")
    amount: String,
    /// 色(#rrggbb)
    color: String,
    /// 応援に添えられたメッセージ(金額を除いた本文)
    message: String,
}

impl Post {
    /// 同じ人を見分ける文字列(ランダムの声を「同じ人には同じ声」にする時に使う)。
    /// 掲示板は日付欄の「ID:xxxx」(無ければ名前)、配信サイトは名前。
    fn person(&self) -> String {
        if self.source == "bbs" {
            if let Some(i) = self.date.find("ID:") {
                return self.date[i..].split_whitespace().next().unwrap_or_default().to_string();
            }
        }
        format!("{}:{}", self.source, self.name)
    }

    fn from_raw(p: &source::RawPost, icon: Option<String>) -> Self {
        Self {
            no: p.no,
            name: text::html_to_text(&p.name),
            date: p.date.clone(),
            body: text::html_to_text(&p.body_html),
            system: false,
            icon,
            source: "bbs",
            avatar: None,
            catch_up: false,
            paid: None,
        }
    }

    fn system(body: impl Into<String>) -> Self {
        Self {
            no: 0,
            name: "ReaCast".into(),
            date: String::new(),
            body: body.into(),
            system: true,
            icon: None,
            source: "system",
            avatar: None,
            catch_up: false,
            paid: None,
        }
    }

    /// 配信サイトのチャット(レス番号は無い)
    fn chat(source: &'static str, name: String, body: String, icon: Option<String>, avatar: Option<String>) -> Self {
        Self { no: 0, name, date: String::new(), body, system: false, icon, source, avatar, catch_up: false, paid: None }
    }

    /// NGワードが名前・本文・日時の行(「ID:xxxx」を含む)のどれかに含まれるか
    fn is_ng(&self, c: &Compiled) -> bool {
        text::contains_ng(&format!("{}\n{}", self.name, self.date), &self.body, &c.ng_words)
    }

    fn to_json(&self, aa: bool) -> serde_json::Value {
        serde_json::json!({
            "no": self.no,
            "name": self.name,
            "date": self.date,
            "body": self.body,
            "aa": aa,
            "system": self.system,
            "icon": self.icon,
            "source": self.source,
            "avatar": self.avatar,
            "paid": self.paid.as_ref().map(|p| serde_json::json!({
                "kind": p.kind, "amount": p.amount, "color": p.color, "message": p.message,
            })),
        })
    }
}

/// 設定から作る、処理用にコンパイル済みの値(レスごとに作り直さないため)。
struct Compiled {
    ng_words: Vec<String>,
    aa_patterns: Vec<String>,
    reading_rules: Vec<text::ReadingRule>,
    reaction_rules: Vec<text::ReactionRule>,
}

impl Compiled {
    fn from(s: &CommentSettings) -> Self {
        Self {
            ng_words: text::non_empty_lines(&s.ng_words),
            aa_patterns: text::non_empty_lines(&s.aa_patterns),
            reading_rules: text::parse_reading_rules(&s.reading_rules),
            reaction_rules: text::parse_reaction_rules(&s.reaction_rules),
        }
    }
}

fn detect_aa(s: &CommentSettings, c: &Compiled, post: &Post) -> bool {
    s.aa_enabled && !post.system && text::is_aa(&post.body, s.aa_threshold_chars, &c.aa_patterns)
}

/// 既に順番待ちに入れた(または開始時点で既にあった)掲示板のレス。即時通知と定期取得の
/// 両方から同じレスが届いても1回しか読まないようにする。
///
/// 番号だけで見分けていたが、同じ時刻の2つの書き込み(2026-10-21、ao33の926と927)で、
/// 即時通知の番号がずれていたらしく、926の内容が927として読まれ、本物の927は
/// 「もう読んだ」扱いで出なかった。そこで内容(日時・名前・本文)でも見分ける。
/// 番号が食い違った時は、定期取得(datの行の順番)を正とする。
#[derive(Default)]
struct SeenPosts {
    /// 内容 → 番号
    by_sig: std::collections::HashMap<String, u32>,
    /// 番号 → 内容
    by_no: std::collections::HashMap<u32, String>,
}

fn post_sig(p: &source::RawPost) -> String {
    let squash = |s: &str| text::html_to_text(s).chars().filter(|c| !c.is_whitespace()).collect::<String>();
    format!("{}\u{1}{}\u{1}{}", p.date.trim(), squash(&p.name), squash(&p.body_html))
}

impl SeenPosts {
    /// 定期取得(dat)で届いたレス。まだ読んでいなければtrue
    fn from_dat(&mut self, p: &source::RawPost) -> bool {
        let sig = post_sig(p);
        if self.by_sig.contains_key(&sig) {
            return false;
        }
        self.by_sig.insert(sig.clone(), p.no);
        self.by_no.insert(p.no, sig);
        true
    }

    /// 即時通知で届いたレス。番号か内容のどちらかが既にあれば読まない(番号がずれていても、
    /// あとの定期取得で内容が違うと分かったものは、そちらで読む)
    fn from_fast(&mut self, p: &source::RawPost) -> bool {
        let sig = post_sig(p);
        if self.by_sig.contains_key(&sig) || self.by_no.contains_key(&p.no) {
            return false;
        }
        self.by_sig.insert(sig.clone(), p.no);
        self.by_no.insert(p.no, sig);
        true
    }
}

/// 読み上げた位置(落ちた時に続きから読むため。ConfigStoreのPROGRESS_KEYに保存)
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct ReadProgress {
    /// スレッドのURL(ThreadRef::read_url)
    thread: String,
    /// 最後に読み上げた(字幕に出した)レス番号
    no: u32,
    /// 「停止」で止めた(続きから読まない)
    clean: bool,
    /// 記録した時刻(UNIX時間のミリ秒)
    at_ms: u64,
}

const PROGRESS_KEY: &str = "readProgress";
/// これより前に記録した位置からは続けない(翌日に開始し直した時に大量に読まないように)
const RESUME_WITHIN_MS: u64 = 60 * 60 * 1000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 前回の続きから読むなら、その直前のレス番号(=最後に読んだレス)。
fn resume_point(p: &ReadProgress, thread: &str, last_no: u32, now: u64) -> Option<u32> {
    (p.thread == thread && !p.clean && p.no > 0 && last_no > p.no && now.saturating_sub(p.at_ms) < RESUME_WITHIN_MS)
        .then_some(p.no)
}

struct State {
    settings: CommentSettings,
    compiled: Arc<Compiled>,
    /// start/stopのたびに増やす。古い取得タスクはこれが変わったら終了する。
    run_id: u64,
    /// 掲示板の読み込み先を手動で切り替えるたびに増やす(古い掲示板の取得タスクを止める)
    bbs_gen: u64,
    queue: VecDeque<Post>,
    /// 今読んでいるスレッドの全レス(設定ウィンドウで過去のレスを遡る用)
    thread_posts: Vec<Post>,
    status: CommentStatus,
    /// デスクトップ字幕をマウスで動かしている最中か
    adjusting: bool,
    /// テスト表示のために、取得していなくてもデスクトップ字幕を出しておく期限
    desktop_hold_until: Option<std::time::Instant>,
    /// 読み上げの一時停止中(開始・停止をまたいで保つ)
    paused: bool,
    /// 最後に記録した読み上げ位置(同じ値を何度も書かないように)
    progress: Option<(String, u32)>,
}

pub struct CommentEngine {
    app: AppHandle,
    state: Mutex<State>,
    wake: Notify,
    /// 「スキップ」で、今のレスの待ち時間を打ち切って次のレスへ進む
    skip: Notify,
}

impl CommentEngine {
    pub fn new(app: AppHandle, settings: CommentSettings) -> Arc<Self> {
        let engine = Arc::new(Self {
            app,
            state: Mutex::new(State {
                compiled: Arc::new(Compiled::from(&settings)),
                settings,
                run_id: 0,
                bbs_gen: 0,
                queue: VecDeque::new(),
                thread_posts: Vec::new(),
                status: CommentStatus::default(),
                adjusting: false,
                desktop_hold_until: None,
                paused: false,
                progress: None,
            }),
            wake: Notify::new(),
            skip: Notify::new(),
        });
        let worker = engine.clone();
        tauri::async_runtime::spawn(async move { worker.run_queue().await });
        engine
    }

    pub fn settings(&self) -> CommentSettings {
        self.state.lock().unwrap().settings.clone()
    }

    pub fn status(&self) -> CommentStatus {
        let st = self.state.lock().unwrap();
        CommentStatus { paused: st.paused, ..st.status.clone() }
    }

    /// 今のレスの字幕を消し、待ち時間を打ち切って次のレスへ進む。ReaCastが鳴らしている
    /// 声(VOICEVOX等)と棒読みちゃんは止める。コマンドで渡し済みの声は止められない
    /// (民安★TALK側で止める)。
    pub fn skip(&self) {
        log::info!("読み上げをスキップ");
        crate::tts::skip(&self.settings());
        self.bridge().clear_subtitle();
        self.skip.notify_waiters();
    }

    /// 読み上げの一時停止と再開を切り替える。一時停止中も取得は続け、順番待ちに溜める。
    pub fn toggle_pause(&self) -> bool {
        let paused = {
            let mut st = self.state.lock().unwrap();
            st.paused = !st.paused;
            st.paused
        };
        log::info!("読み上げを{}", if paused { "一時停止" } else { "再開" });
        if !paused {
            self.wake.notify_one();
        }
        self.emit_status();
        paused
    }

    /// 指定した時間待つ。途中で「スキップ」されたらすぐ戻る。
    async fn wait_or_skip(&self, ms: u64) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
            _ = self.skip.notified() => {}
        }
    }

    pub fn apply_settings(&self, settings: CommentSettings) {
        let compiled = Arc::new(Compiled::from(&settings));
        let style = settings.style.clone();
        let icon_folders = [
            ("bbs", settings.icon_folder_bbs.clone()),
            ("youtube", settings.icon_folder_youtube.clone()),
            ("twitch", settings.icon_folder_twitch.clone()),
        ];
        // VOICEVOX等を使う設定なら、起動していなければ裏で起動しておく(ReaCastの起動時も通る)
        crate::tts::launch_in_background(&settings);
        {
            let mut st = self.state.lock().unwrap();
            st.settings = settings;
            st.compiled = compiled;
        }
        let bridge = self.bridge();
        bridge.set_comment_style(style);
        for (source, folder) in &icon_folders {
            bridge.set_icon_folder(source, folder);
        }
    }

    /// 文章が「リアクション連動」のルールに当てはまったリアクションのID(話した字幕用。最大数は設定どおり)
    pub fn matched_reactions(&self, body: &str) -> Vec<String> {
        let st = self.state.lock().unwrap();
        text::matched_reactions(body, &st.compiled.reaction_rules, st.settings.reaction_link_max)
    }

    /// デスクトップ字幕のウィンドウを今出しておくべきか(ONで、かつ取得中・
    /// 位置調整中・テスト表示の直後のいずれか)。
    pub fn desktop_wanted(&self) -> bool {
        let st = self.state.lock().unwrap();
        let held = st.desktop_hold_until.map(|t| t > std::time::Instant::now()).unwrap_or(false);
        st.settings.desktop_enabled && (st.status.running || st.adjusting || held)
    }

    pub fn adjusting(&self) -> bool {
        self.state.lock().unwrap().adjusting
    }

    pub fn set_adjusting(&self, on: bool) {
        self.state.lock().unwrap().adjusting = on;
    }

    pub fn hold_desktop(&self, dur: Duration) {
        self.state.lock().unwrap().desktop_hold_until = Some(std::time::Instant::now() + dur);
    }

    fn pick_icon(&self, source: &str) -> Option<String> {
        use rand::Rng;
        let n = self.bridge().icon_count(source);
        (n > 0).then(|| format!("/icons/{source}/{}", rand::thread_rng().gen_range(0..n)))
    }

    /// 次スレのタイトルと>>1の本文を、設定のテンプレートから作る。
    pub fn next_thread_draft(&self) -> Result<(String, String), String> {
        let (s, status) = {
            let st = self.state.lock().unwrap();
            (st.settings.clone(), st.status.clone())
        };
        let cur_title = status.thread_title.ok_or("先に取得を開始してください(今のスレッドのタイトルが必要です)")?;
        build_draft(&s, &cur_title, status.thread_url.as_deref().unwrap_or(""))
    }

    /// 今読んでいるスレッドの>>1の本文(テンプレートに取り込む用)。
    pub fn first_post(&self) -> Option<String> {
        self.state.lock().unwrap().thread_posts.iter().find(|p| p.no == 1).map(|p| p.body.clone())
    }

    /// 取得を開始する。掲示板(urlが空なら取得しない)と、設定にあるYouTube・Twitchを
    /// それぞれ別のタスクで同時に取得し、すべて同じ順番待ちの列に並べる。
    pub fn start(self: &Arc<Self>, url: &str) -> Result<(), String> {
        let settings = self.settings();
        let bbs = if url.trim().is_empty() { None } else { Some(source::parse_url(url)?) };
        let youtube = if settings.youtube_target.trim().is_empty() {
            None
        } else {
            Some(youtube::parse_target(&settings.youtube_target)?)
        };
        let twitch = if settings.twitch_channel.trim().is_empty() {
            None
        } else {
            Some(twitch::parse_channel(&settings.twitch_channel)?)
        };
        if bbs.is_none() && youtube.is_none() && twitch.is_none() {
            return Err("掲示板のURL、YouTube、Twitchのどれかを指定してください".into());
        }
        let mut sources = Vec::new();
        if youtube.is_some() {
            sources.push(SourceStatus { id: "youtube".into(), label: "YouTube".into(), state: "wait".into(), message: "接続中…".into() });
        }
        if twitch.is_some() {
            sources.push(SourceStatus { id: "twitch".into(), label: "Twitch".into(), state: "wait".into(), message: "接続中…".into() });
        }
        let run_id = {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.thread_posts.clear();
            st.settings.thread_url = url.trim().to_string();
            st.status = CommentStatus { running: true, sources, ..Default::default() };
            st.run_id
        };
        log::info!(
            "コメント取得を開始: 掲示板={} YouTube={} Twitch={}",
            url.trim(),
            settings.youtube_target.trim(),
            settings.twitch_channel.trim()
        );
        self.emit_status();
        self.emit_thread_reset();
        // 開始(再開始)のたびに右のレス一覧を空にしてから始める
        self.bridge().clear_comments();
        let initial = self.settings().initial_text;
        if !initial.trim().is_empty() {
            self.bridge().push_comment(Post::system(initial).to_json(false));
        }
        if let Some((board, key)) = bbs {
            let me = self.clone();
            let gen = self.state.lock().unwrap().bbs_gen;
            tauri::async_runtime::spawn(async move { me.run_fetch(run_id, gen, board, key).await });
        }
        if let Some(target) = youtube {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.run_youtube(run_id, target).await });
        }
        if let Some(channel) = twitch {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.run_twitch(run_id, channel).await });
        }
        Ok(())
    }

    fn update_source(&self, run_id: u64, id: &str, state: &str, message: impl Into<String>) {
        let message = message.into();
        self.update_status(run_id, |s| {
            if let Some(src) = s.sources.iter_mut().find(|x| x.id == id) {
                src.state = state.into();
                src.message = message;
            }
        });
    }

    /// 停止されるまで待つ(1秒ごとに止められていないか確認する)。止められたらfalse。
    async fn sleep_while_current(&self, run_id: u64, dur: Duration) -> bool {
        let end = tokio::time::Instant::now() + dur;
        while tokio::time::Instant::now() < end {
            if !self.is_current(run_id) {
                return false;
            }
            let left = end - tokio::time::Instant::now();
            tokio::time::sleep(left.min(Duration::from_secs(1))).await;
        }
        self.is_current(run_id)
    }

    async fn run_youtube(self: Arc<Self>, run_id: u64, target: youtube::Target) {
        let client = youtube::client();
        'outer: while self.is_current(run_id) {
            // 1. 動画IDを決める(チャンネルなら、配信が始まるまで待つ)
            let video_id = match &target {
                youtube::Target::Video(id) => id.clone(),
                youtube::Target::Channel(url) => match youtube::get_text(&client, &format!("{url}/live")).await {
                    Ok(html) => match youtube::extract_live_video_id(&html) {
                        Some(id) => id,
                        None => {
                            self.update_source(run_id, "youtube", "wait", "配信が始まるのを待っています");
                            if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                                return;
                            }
                            continue;
                        }
                    },
                    Err(e) => {
                        self.update_source(run_id, "youtube", "error", e);
                        if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                            return;
                        }
                        continue;
                    }
                },
            };
            // 2. チャット欄のページから、続きを取得するためのトークンを取り出す
            let params = match youtube::get_text(&client, &format!("https://www.youtube.com/live_chat?is_popout=1&v={video_id}")).await {
                Ok(html) => youtube::extract_chat_params(&html),
                Err(e) => {
                    self.update_source(run_id, "youtube", "error", e);
                    None
                }
            };
            let Some(params) = params else {
                self.update_source(run_id, "youtube", "wait", "チャットを読み込めません(配信前・終了後、またはチャットが無効)");
                if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                    return;
                }
                continue;
            };
            self.update_source(run_id, "youtube", "ok", format!("取得中(動画 {video_id})"));
            // 3. 新着コメントを取り続ける(最初のページにある過去のコメントは読まない)
            let mut continuation = params.continuation.clone();
            let mut failures = 0;
            loop {
                if !self.is_current(run_id) {
                    return;
                }
                match youtube::fetch_chat(&client, &params, &continuation).await {
                    Ok(page) => {
                        failures = 0;
                        let posts: Vec<Post> = page
                            .messages
                            .into_iter()
                            .map(|m| {
                                let body = match &m.amount {
                                    Some(a) if m.text.is_empty() => a.clone(),
                                    Some(a) => format!("{a} {}", m.text),
                                    None => m.text.clone(),
                                };
                                let mut post = Post::chat("youtube", m.author, body, self.pick_icon("youtube"), m.photo);
                                if let (Some(kind), Some(amount)) = (m.kind, m.amount) {
                                    post.paid = Some(Paid {
                                        kind,
                                        amount,
                                        color: m.color.unwrap_or_else(|| "#ffca28".into()),
                                        message: m.text,
                                    });
                                }
                                post
                            })
                            .collect();
                        self.enqueue(run_id, posts);
                        match page.next {
                            Some((c, timeout)) => {
                                continuation = c;
                                let wait = Duration::from_millis(timeout.clamp(1500, 5000));
                                if !self.sleep_while_current(run_id, wait).await {
                                    return;
                                }
                            }
                            None => {
                                self.update_source(run_id, "youtube", "wait", "配信が終了しました。次の配信を待っています");
                                if !self.sleep_while_current(run_id, Duration::from_secs(30)).await {
                                    return;
                                }
                                continue 'outer;
                            }
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        self.update_source(run_id, "youtube", "error", e);
                        if !self.sleep_while_current(run_id, Duration::from_secs(5)).await {
                            return;
                        }
                        if failures >= 3 {
                            continue 'outer; // 最初からやり直す
                        }
                    }
                }
            }
        }
    }

    async fn run_twitch(self: Arc<Self>, run_id: u64, channel: String) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        // ログイン名→アイコンのURL(調べられなかった人はNone)。同じ人を何度も調べない
        let mut avatars: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
        let client = source::http_client();
        while self.is_current(run_id) {
            let conn = tokio_tungstenite::connect_async("wss://irc-ws.chat.twitch.tv:443").await;
            let (mut write, mut read) = match conn {
                Ok((ws, _)) => ws.split(),
                Err(e) => {
                    self.update_source(run_id, "twitch", "error", format!("Twitchへの接続に失敗しました: {e}"));
                    if !self.sleep_while_current(run_id, Duration::from_secs(10)).await {
                        return;
                    }
                    continue;
                }
            };
            let nick = format!("justinfan{}", rand::random::<u32>() % 90000 + 10000);
            for line in [
                "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
                "PASS SCHMOOPIIE".to_string(),
                format!("NICK {nick}"),
                format!("JOIN #{channel}"),
            ] {
                let _ = write.send(Message::Text(line)).await;
            }
            self.update_source(run_id, "twitch", "ok", format!("取得中(#{channel})"));
            loop {
                if !self.is_current(run_id) {
                    let _ = write.close().await;
                    return;
                }
                // 1秒ごとに止められていないか確認しつつ読む
                let msg = match tokio::time::timeout(Duration::from_secs(1), read.next()).await {
                    Err(_) => continue,
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(m))) => m,
                };
                let Message::Text(text) = msg else { continue };
                let mut posts = Vec::new();
                let mut reconnect = false;
                for line in text.split("\r\n").filter(|l| !l.is_empty()) {
                    if let Some(rest) = line.strip_prefix("PING") {
                        let _ = write.send(Message::Text(format!("PONG{rest}"))).await;
                    } else if line.contains(" RECONNECT") {
                        // Twitch側からの「繋ぎ直してください」の合図
                        reconnect = true;
                    } else if let Some(m) = twitch::parse_privmsg(line).or_else(|| twitch::parse_usernotice(line)) {
                        if !avatars.contains_key(&m.login) {
                            if avatars.len() > 2000 {
                                avatars.clear();
                            }
                            avatars.insert(m.login.clone(), twitch::fetch_avatar(&client, &m.login).await);
                        }
                        let avatar = avatars.get(&m.login).cloned().flatten();
                        let body = match &m.paid {
                            Some(p) if m.text.is_empty() => p.label.clone(),
                            Some(p) => format!("{} {}", p.label, m.text),
                            None => m.text.clone(),
                        };
                        let mut post = Post::chat("twitch", m.author, body, self.pick_icon("twitch"), avatar);
                        if let Some(p) = m.paid {
                            post.paid = Some(Paid { kind: p.kind, amount: p.label, color: p.color, message: m.text });
                        }
                        posts.push(post);
                    }
                }
                self.enqueue(run_id, posts);
                if reconnect {
                    break;
                }
            }
            self.update_source(run_id, "twitch", "wait", "接続が切れました。再接続しています");
            if !self.sleep_while_current(run_id, Duration::from_secs(3)).await {
                return;
            }
        }
    }

    fn load_progress(&self) -> Option<ReadProgress> {
        self.app
            .try_state::<ConfigStore>()?
            .get(PROGRESS_KEY)
            .and_then(|v| serde_json::from_value(v).ok())
    }

    fn save_progress(&self, p: &ReadProgress) {
        if let (Some(store), Ok(v)) = (self.app.try_state::<ConfigStore>(), serde_json::to_value(p)) {
            if let Err(e) = store.set(PROGRESS_KEY.to_string(), v) {
                log::warn!("読み上げ位置を保存できませんでした: {e}");
            }
        }
    }

    /// 掲示板のレスを読み上げた(字幕に出した)ので、位置を記録する。
    fn record_progress(&self, post: &Post) {
        if post.source != "bbs" || post.system || post.no == 0 {
            return;
        }
        let thread = {
            let mut st = self.state.lock().unwrap();
            let Some(thread) = st.status.thread_url.clone() else { return };
            // 同じスレッドで前に戻る(「読み上げ」ボタンでの読み直し等)時は記録しない
            if st.progress.as_ref().is_some_and(|(t, n)| *t == thread && *n >= post.no) {
                return;
            }
            st.progress = Some((thread.clone(), post.no));
            thread
        };
        self.save_progress(&ReadProgress { thread, no: post.no, clean: false, at_ms: now_ms() });
    }

    /// 指定したレスから読み上げ直す(「スレッドのレス」タブの右クリック「ここから読み始める」)。
    /// 順番待ちの掲示板のレスは入れ直す(YouTube・Twitchのコメントは残す)。
    pub fn read_from(&self, no: u32) -> Result<usize, String> {
        let n = {
            let mut st = self.state.lock().unwrap();
            if !st.status.running {
                return Err("取得を開始してから使ってください".into());
            }
            let posts: Vec<Post> = st
                .thread_posts
                .iter()
                .filter(|p| p.no >= no && !p.system)
                .cloned()
                .map(|mut p| {
                    p.catch_up = true;
                    p
                })
                .collect();
            if posts.is_empty() {
                return Err("そのレス以降のレスがありません".into());
            }
            st.queue.retain(|p| p.source != "bbs");
            let n = posts.len();
            for p in posts.into_iter().rev() {
                st.queue.push_front(p);
            }
            // 前に戻って読む時も位置を記録し直す
            st.progress = None;
            st.status.queue_len = st.queue.len();
            n
        };
        log::info!("レス{no}から読み上げ直します({n}件)");
        // 今読んでいるレスを止めて、すぐに始める
        self.skip();
        self.wake.notify_one();
        self.emit_status();
        Ok(n)
    }

    pub fn stop(&self) {
        // 「停止」で止めた時は、次に開始しても続きから読まない
        if let Some(mut p) = self.load_progress() {
            if !p.clean {
                p.clean = true;
                self.save_progress(&p);
            }
        }
        // 取得と、まだ読んでいない順番待ちは捨てる。「スレッドのレス」の一覧は残す
        // (止めた後も見返したり、右クリックでNGワード登録・コピーができるように。
        // 次に開始した時に読み込み直す)
        {
            let mut st = self.state.lock().unwrap();
            st.run_id += 1;
            st.queue.clear();
            st.status = CommentStatus::default();
        }
        log::info!("コメント取得を停止");
        self.bridge().clear_subtitle();
        crate::tts::stop();
        self.emit_status();
    }

    fn emit_thread_reset(&self) {
        let _ = self.app.emit("comments:posts", serde_json::json!({ "reset": true, "posts": [] }));
    }

    /// 今のスレッドの全レス(設定ウィンドウの「スレッドのレス」タブ用)
    pub fn thread_posts_json(&self) -> serde_json::Value {
        let st = self.state.lock().unwrap();
        let posts: Vec<serde_json::Value> = st
            .thread_posts
            .iter()
            .map(|p| p.to_json(detect_aa(&st.settings, &st.compiled, p)))
            .collect();
        serde_json::json!({ "posts": posts })
    }

    /// 指定したレス番号のレスを、順番待ちの先頭に入れてもう一度表示・読み上げる。
    pub fn replay(&self, no: u32) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let post = st.thread_posts.iter().find(|p| p.no == no).cloned().ok_or("そのレスが見つかりません")?;
        st.queue.push_front(post);
        drop(st);
        self.wake.notify_one();
        Ok(())
    }

    /// 取得したレスをスレッドの全レス一覧に追加し、設定ウィンドウにも知らせる。
    fn record_posts(&self, run_id: u64, posts: &[Post], reset: bool) {
        let json = {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            if reset {
                st.thread_posts.clear();
            }
            st.thread_posts.extend(posts.iter().cloned());
            let (settings, compiled) = (st.settings.clone(), st.compiled.clone());
            posts
                .iter()
                .map(|p| p.to_json(detect_aa(&settings, &compiled, p)))
                .collect::<Vec<_>>()
        };
        if reset || !json.is_empty() {
            let _ = self.app.emit("comments:posts", serde_json::json!({ "reset": reset, "posts": json }));
        }
    }

    /// 設定画面の「テスト表示」用。順番待ちの先頭に割り込ませる。
    pub fn test(&self, body: &str) {
        let mut post = Post::system(body);
        post.no = 1;
        post.name = "テスト".into();
        self.state.lock().unwrap().queue.push_front(post);
        self.wake.notify_one();
    }

    fn bridge(&self) -> ObsBridge {
        self.app.state::<ObsBridge>().inner().clone()
    }

    fn is_current(&self, run_id: u64) -> bool {
        self.state.lock().unwrap().run_id == run_id
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app.emit("comments:status", status);
    }

    fn update_status(&self, run_id: u64, f: impl FnOnce(&mut CommentStatus)) {
        {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            let before = st.status.clone();
            f(&mut st.status);
            st.status.queue_len = st.queue.len();
            log_status_change(&before, &st.status);
        }
        self.emit_status();
    }

    fn enqueue(&self, run_id: u64, posts: Vec<Post>) {
        if posts.is_empty() {
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            if st.run_id != run_id {
                return;
            }
            // ニコ生風の「届いたらすぐ流す」: 読み上げの順番待ちを通さず、届いた時点で流す
            // (字幕・読み上げ・右のレス一覧はこれまでどおり順番待ちから1件ずつ)
            if st.settings.style.pointer("/nico/instant").and_then(|v| v.as_bool()).unwrap_or(false) {
                let bridge = self.bridge();
                for p in posts.iter().filter(|p| !p.system && !p.is_ng(&st.compiled)) {
                    bridge.push_nico(p.to_json(detect_aa(&st.settings, &st.compiled, p)));
                }
            }
            st.queue.extend(posts);
            st.status.queue_len = st.queue.len();
        }
        self.wake.notify_one();
    }

    /// 掲示板の読み込み先を、指定したスレッドに切り替える(取得中のみ。YouTube・
    /// Twitchの取得や右のレス一覧はそのまま続ける)。保存してある取得先のURLは
    /// 変えないので、次に開始し直すとまた自動で現行スレを選ぶ。
    pub fn switch_thread(self: &Arc<Self>, url: &str) -> Result<(), String> {
        let (board, key) = source::parse_url(url)?;
        let key = key.ok_or("スレッドのURLを指定してください")?;
        let (run_id, gen) = {
            let mut st = self.state.lock().unwrap();
            if !st.status.running {
                return Err("取得を開始してから切り替えてください".into());
            }
            st.bbs_gen += 1;
            st.thread_posts.clear();
            st.status.thread_title = None;
            st.status.thread_url = None;
            st.status.last_no = 0;
            st.status.warning = None;
            (st.run_id, st.bbs_gen)
        };
        self.emit_thread_reset();
        self.emit_status();
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.run_fetch(run_id, gen, board, Some(key)).await });
        Ok(())
    }

    fn is_bbs_current(&self, run_id: u64, gen: u64) -> bool {
        let st = self.state.lock().unwrap();
        st.run_id == run_id && st.bbs_gen == gen
    }

    async fn run_fetch(self: Arc<Self>, run_id: u64, gen: u64, board: Board, key: Option<String>) {
        let client = source::http_client();
        let poll_interval = |me: &Self| Duration::from_millis(me.settings().poll_interval_ms.max(1000));

        // 板のURLが入力された場合は、今使っている(現行の)スレッドを選ぶ
        let key = match key {
            Some(k) => k,
            None => loop {
                if !self.is_bbs_current(run_id, gen) {
                    return;
                }
                match source::list_threads(&client, &board).await {
                    Ok(list) => match source::pick_current(&list, &self.settings().next_thread_keyword) {
                        Some(t) => break t.key.clone(),
                        None => self.update_status(run_id, |s| s.error = Some("板にスレッドが見つかりません".into())),
                    },
                    Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
                }
                tokio::time::sleep(poll_interval(&self)).await;
            },
        };

        let mut reader = ThreadReader::new(ThreadRef { board: board.clone(), key });
        let mut first = true;
        let mut warned = false;
        let mut created = false;
        // 既に順番待ちに入れた(または開始時点で既にあった)レス番号。即時通知と
        // 定期取得の両方から同じレスが届いても、1回しか読まないようにする
        let mut seen = SeenPosts::default();

        // jpnknの板なら、即時通知(MQTT)も使う。通知で届いたレスはチャンネル経由で
        // このループに渡す(定期取得は取りこぼし対策としてそのまま続ける)
        let mut fast_rx = match (&board, self.settings().jpnkn_fast) {
            (Board::Nch { origin, board: name }, true) if origin.contains("jpnkn.com") => {
                let (tx, rx) = tokio::sync::mpsc::channel::<(String, source::RawPost)>(256);
                self.update_status(run_id, |s| {
                    if !s.sources.iter().any(|x| x.id == "jpnkn") {
                        s.sources.insert(0, SourceStatus {
                            id: "jpnkn".into(),
                            label: "jpnkn即時通知".into(),
                            state: "wait".into(),
                            message: "接続中…".into(),
                        });
                    }
                });
                let me = self.clone();
                let name = name.clone();
                tauri::async_runtime::spawn(async move { me.run_jpnkn_fast(run_id, gen, name, tx).await });
                Some(rx)
            }
            _ => None,
        };

        loop {
            if !self.is_bbs_current(run_id, gen) {
                return;
            }
            let polled = reader.poll(&client).await;
            // 取得している間に手動で別のスレッドへ切り替えられていたら、結果を捨てる
            if !self.is_bbs_current(run_id, gen) {
                return;
            }
            match polled {
                Ok(raw) => {
                    let posts: Vec<Post> = raw
                        .iter()
                        .filter(|p| seen.from_dat(p)) // 即時通知で既に読んだレスは除く
                        .map(|p| Post::from_raw(p, self.pick_icon("bbs")))
                        .collect();
                    self.record_posts(run_id, &posts, false);
                    if first {
                        // 起動時点で既にあったレスは読み上げない(右のレス一覧にだけ載せる)
                        first = false;
                        let compiled = self.state.lock().unwrap().compiled.clone();
                        let settings = self.settings();
                        // 開始レス番号が指定されている時だけ、それ以降を右のレス一覧に載せる
                        // (既定では載せず、「スレッド読み込みを開始しました」だけを出す)
                        let shown: Vec<&Post> = if settings.start_res_no > 0 {
                            posts.iter().filter(|p| p.no >= settings.start_res_no).collect()
                        } else {
                            Vec::new()
                        };
                        for p in shown {
                            if !p.is_ng(&compiled) {
                                self.bridge().push_comment(p.to_json(detect_aa(&settings, &compiled, p)));
                            }
                        }
                        // 前回、読み上げの途中で落ちていたら、その続きから読む
                        let resume = if settings.resume_reading {
                            self.load_progress()
                                .and_then(|p| resume_point(&p, &reader.thread.read_url(), reader.last_no, now_ms()))
                        } else {
                            None
                        };
                        if let Some(last) = resume {
                            let unread: Vec<Post> = posts
                                .iter()
                                .filter(|p| p.no > last && !(settings.skip_first_post && p.no == 1))
                                .cloned()
                                .map(|mut p| {
                                    p.catch_up = true;
                                    p
                                })
                                .collect();
                            log::info!("前回の続き(レス{})から読み上げます({}件)", last + 1, unread.len());
                            self.bridge().push_comment(
                                Post::system(format!("前回の続き(レス{}から)を読み上げます", last + 1)).to_json(false),
                            );
                            self.enqueue(run_id, unread);
                        }
                    } else {
                        let skip_first = self.settings().skip_first_post;
                        self.enqueue(run_id, posts.into_iter().filter(|p| !(skip_first && p.no == 1)).collect());
                    }
                    let (title, url, last_no) = (reader.title.clone(), reader.thread.read_url(), reader.last_no);
                    self.update_status(run_id, |s| {
                        s.error = None;
                        s.thread_title = title;
                        s.thread_url = Some(url);
                        s.last_no = last_no;
                    });

                    let settings = self.settings();
                    if settings.auto_create_thread
                        && !created
                        && settings.create_thread_count > 0
                        && reader.last_no >= settings.create_thread_count
                        && reader.last_no < 1000
                    {
                        // 次スレを1回だけ建てる(既に誰かが建てていれば建てない)
                        created = true;
                        warned = true;
                        let msg = match source::list_threads(&client, &board).await {
                            Ok(list) => match source::pick_next(&list, &reader.thread.key, &settings.next_thread_keyword) {
                                Some(t) => format!("次スレ「{}」は既に建っています", t.title),
                                None => match self.next_thread_draft() {
                                    Ok((title, body)) => match source::create_thread(
                                        &client,
                                        &board,
                                        &title,
                                        &settings.create_name,
                                        &settings.create_mail,
                                        &body,
                                    )
                                    .await
                                    {
                                        Ok(()) => format!("次スレ「{title}」を建てました"),
                                        Err(e) => e,
                                    },
                                    Err(e) => e,
                                },
                            },
                            Err(e) => e,
                        };
                        self.update_status(run_id, |s| s.warning = Some(msg.clone()));
                        self.enqueue(run_id, vec![Post::system(msg)]);
                    }

                    if !settings.auto_create_thread
                        && !warned
                        && settings.thread_warn_count > 0
                        && reader.last_no >= settings.thread_warn_count
                        && reader.last_no < 1000
                    {
                        warned = true;
                        let msg = format!("レスが{}を超えました。次スレを立ててください", settings.thread_warn_count);
                        self.update_status(run_id, |s| s.warning = Some(msg.clone()));
                        self.enqueue(run_id, vec![Post::system(msg)]);
                    }

                    if settings.auto_next_thread && reader.last_no >= 1000 {
                        match source::list_threads(&client, &board).await {
                            Ok(list) => {
                                if let Some(next) = source::pick_next(&list, &reader.thread.key, &settings.next_thread_keyword) {
                                    // 次スレのレスは1番から全部新着として扱う
                                    reader = ThreadReader::new(ThreadRef { board: board.clone(), key: next.key.clone() });
                                    self.record_posts(run_id, &[], true);
                                    seen = SeenPosts::default();
                                    warned = false;
                                    created = false;
                                    self.update_status(run_id, |s| s.warning = None);
                                    self.enqueue(run_id, vec![Post::system(format!("次スレ「{}」に移動しました", next.title))]);
                                    continue;
                                } else {
                                    self.update_status(run_id, |s| s.warning = Some("スレッドが1000を超えました。次スレを探しています".into()));
                                }
                            }
                            Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
                        }
                    }
                }
                Err(e) => self.update_status(run_id, |s| s.error = Some(e)),
            }

            // 次の定期取得まで待つ。その間に即時通知で届いたレスは、すぐ順番待ちに入れる
            let deadline = tokio::time::Instant::now() + poll_interval(&self);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    msg = recv_or_pending(&mut fast_rx) => {
                        let Some((thread_key, raw)) = msg else {
                            fast_rx = None; // 通知の接続タスクが終わった
                            continue;
                        };
                        if !self.is_bbs_current(run_id, gen) {
                            return;
                        }
                        // 1001番(「このスレッドは1000を超えました」)は即時通知では使わない。
                        // 1000番より先に読まれてしまったことがあるため(定期取得なら
                        // 1000番→1001番の順に届く)
                        if raw.no > 1000 {
                            continue;
                        }
                        // 今読んでいるスレッドへの、まだ読んでいない書き込みだけ
                        if first || thread_key != reader.thread.key || !seen.from_fast(&raw) {
                            continue;
                        }
                        let post = Post::from_raw(&raw, self.pick_icon("bbs"));
                        self.record_posts(run_id, std::slice::from_ref(&post), false);
                        if !(raw.no == 1 && self.settings().skip_first_post) {
                            self.enqueue(run_id, vec![post]);
                        }
                    }
                }
            }
        }
    }

    /// jpnknの即時通知(MQTT over WebSocket)に繋ぎ、板への書き込みをtxに送り続ける。
    /// 切れたら繋ぎ直す。取得が止められる(または掲示板が切り替えられる)と終わる。
    async fn run_jpnkn_fast(
        self: Arc<Self>,
        run_id: u64,
        gen: u64,
        board: String,
        tx: tokio::sync::mpsc::Sender<(String, source::RawPost)>,
    ) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;
        let client = source::http_client();
        while self.is_bbs_current(run_id, gen) {
            // 接続先のサーバーを一覧から選ぶ(取れなければ既定のサーバー)
            let servers = match client.get(jpnkn_fast::SERVER_LIST_URL).send().await {
                Ok(r) => r.json::<serde_json::Value>().await.map(|v| jpnkn_fast::secure_servers(&v)).unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let url = if servers.is_empty() {
                "wss://a.mq.jpnkn.com:9091/mqtt".to_string()
            } else {
                servers[rand::random::<usize>() % servers.len()].clone()
            };
            let request = url.as_str().into_client_request().map(|mut r| {
                r.headers_mut().insert("Sec-WebSocket-Protocol", "mqtt".parse().unwrap());
                r
            });
            let conn = match request {
                Ok(req) => tokio_tungstenite::connect_async(req).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let (mut write, mut read) = match conn {
                Ok((ws, _)) => ws.split(),
                Err(e) => {
                    self.update_source(run_id, "jpnkn", "error", format!("即時通知に接続できません({e})。定期取得で続けます"));
                    if !self.sleep_while_bbs_current(run_id, gen, Duration::from_secs(15)).await {
                        return;
                    }
                    continue;
                }
            };
            let client_id = format!("reacast-{}", rand::random::<u32>());
            let _ = write.send(Message::Binary(jpnkn_fast::connect_packet(&client_id))).await;
            let mut buf: Vec<u8> = Vec::new();
            let mut last_ping = tokio::time::Instant::now();
            loop {
                if !self.is_bbs_current(run_id, gen) {
                    let _ = write.close().await;
                    return;
                }
                if last_ping.elapsed() >= Duration::from_secs((jpnkn_fast::KEEPALIVE_SECS / 2) as u64) {
                    last_ping = tokio::time::Instant::now();
                    if write.send(Message::Binary(jpnkn_fast::pingreq_packet())).await.is_err() {
                        break;
                    }
                }
                let msg = match tokio::time::timeout(Duration::from_secs(1), read.next()).await {
                    Err(_) => continue,
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(m))) => m,
                };
                let data = match msg {
                    Message::Binary(d) => d,
                    Message::Close(_) => break,
                    _ => continue,
                };
                buf.extend_from_slice(&data);
                for p in jpnkn_fast::take_packets(&mut buf) {
                    match p {
                        jpnkn_fast::Incoming::ConnAck(0) => {
                            let _ = write.send(Message::Binary(jpnkn_fast::subscribe_packet(&format!("bbs/{board}")))).await;
                        }
                        jpnkn_fast::Incoming::ConnAck(rc) => {
                            self.update_source(run_id, "jpnkn", "error", format!("即時通知への接続が拒否されました(コード{rc})"));
                        }
                        jpnkn_fast::Incoming::SubAck => {
                            self.update_source(run_id, "jpnkn", "ok", "接続中(書き込まれた瞬間に拾います)");
                        }
                        jpnkn_fast::Incoming::Publish { payload, .. } => {
                            if let Some(item) = jpnkn_fast::parse_payload(&payload) {
                                if tx.send(item).await.is_err() {
                                    return; // 掲示板の取得ループが終わった
                                }
                            }
                        }
                        jpnkn_fast::Incoming::Other => {}
                    }
                }
            }
            self.update_source(run_id, "jpnkn", "wait", "即時通知が切れました。再接続しています(その間も定期取得で拾います)");
            if !self.sleep_while_bbs_current(run_id, gen, Duration::from_secs(5)).await {
                return;
            }
        }
    }

    async fn sleep_while_bbs_current(&self, run_id: u64, gen: u64, dur: Duration) -> bool {
        let end = tokio::time::Instant::now() + dur;
        while tokio::time::Instant::now() < end {
            if !self.is_bbs_current(run_id, gen) {
                return false;
            }
            tokio::time::sleep((end - tokio::time::Instant::now()).min(Duration::from_secs(1))).await;
        }
        self.is_bbs_current(run_id, gen)
    }

    async fn run_queue(self: Arc<Self>) {
        loop {
            if self.state.lock().unwrap().paused {
                // 再開(またはレスの追加)で起こされるまで待つ
                self.wake.notified().await;
                continue;
            }
            let next = {
                let mut st = self.state.lock().unwrap();
                // 溜まっている数(ターボの判定用)には、続きから読むために入れ直したレスは数えない
                st.queue
                    .pop_front()
                    .map(|p| (p, st.settings.clone(), st.compiled.clone(), st.queue.iter().filter(|q| !q.catch_up).count()))
            };
            let Some((post, settings, compiled, backlog)) = next else {
                self.wake.notified().await;
                continue;
            };
            {
                let mut st = self.state.lock().unwrap();
                st.status.queue_len = st.queue.len();
            }
            self.emit_status();

            // 読み上げた位置を記録(NGで飛ばすレスも、読み終わった扱い)
            self.record_progress(&post);
            if !post.system && post.is_ng(&compiled) {
                continue;
            }
            let aa = detect_aa(&settings, &compiled, &post);
            let bridge = self.bridge();

            // >>番号が付いたレスなら、アンカー先のレスを先に(斜体で)下の字幕に出す。
            // 溜まっている時(ターボ中)は追いつくのを優先して省く。
            if settings.anchor_preview && post.source == "bbs" && backlog < settings.turbo_threshold.max(1) {
                let quoted = text::first_anchor(&post.body).and_then(|n| {
                    let st = self.state.lock().unwrap();
                    st.thread_posts.iter().find(|p| p.no == n && n != post.no).cloned()
                });
                if let Some(q) = quoted {
                    let q_aa = detect_aa(&settings, &compiled, &q);
                    let mut json = q.to_json(q_aa);
                    json["quoted"] = serde_json::json!(true);
                    let ms = settings.anchor_preview_ms.max(300);
                    bridge.show_subtitle(json, ms);
                    self.wait_or_skip(ms).await;
                }
            }

            let engine = crate::tts::engine_kind(&settings);
            let speaking = settings.speech_enabled
                && (engine != crate::tts::EngineKind::Command || !settings.command_path.trim().is_empty());
            let spoken = if speaking { speech_text(&post, aa, &settings, &compiled) } else { String::new() };
            let mut spoken = fit_speech_to_max_wait(spoken, &settings);
            let mut speech_ms = speech_duration(&spoken, &settings);
            // ReaCastが鳴らす読み上げソフト(VOICEVOX等)は、字幕を出す前に音声を作っておき、
            // 本当の長さだけ字幕を出して待つ(目安の時間ではなく)
            let mut wav = None;
            if crate::tts::plays_itself(&settings) && !spoken.trim().is_empty() {
                let mut voice = crate::tts::pick_voice(&settings, post.source, &post.person());
                // ターボ中は話す速さを上げて追いつく(声は途中で切らない。next_interval参照)
                if backlog >= settings.turbo_threshold.max(1) {
                    voice.tts_speed = (voice.tts_speed * settings.turbo_speed_scale.clamp(1.0, 2.0)).min(2.5);
                }
                let mut result = crate::tts::synthesize_or_launch(&voice, &spoken).await;
                // 最大待ち時間を超える音声なら、その声の実際の速さで削り直して作り直す
                // (途中で声を止めずに「以下略」まで読ませるため)
                let target = settings.max_wait_ms.saturating_sub(150);
                for _ in 0..3 {
                    let ms = match &result {
                        Ok(Some((_, ms))) if settings.max_wait_ms > 0 && *ms > target => *ms,
                        _ => break,
                    };
                    let Some(shorter) = shorten_speech_to_duration(&spoken, ms, target) else { break };
                    spoken = shorter;
                    result = crate::tts::synthesize_or_launch(&voice, &spoken).await;
                }
                match result {
                    Ok(Some((w, ms))) => {
                        speech_ms = ms + 150;
                        wav = Some(w);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        // 読み上げソフトが起動していない等。声は出ないので待たない
                        log::warn!("読み上げ: {e}");
                        speech_ms = 0;
                    }
                }
            }
            // 長すぎる時は最大待ち時間で次へ(声もそこで止める)
            let cut_speech = settings.max_wait_ms > 0 && speech_ms > settings.max_wait_ms;
            if cut_speech {
                speech_ms = settings.max_wait_ms;
            }
            // 読み上げている間は字幕も出しておく(長いレスでも最大待ち時間まで)
            let shown_ms = display_duration(&post, aa, &settings).max(speech_ms);
            let shown_ms = if settings.max_wait_ms > 0 { shown_ms.min(settings.max_wait_ms) } else { shown_ms };
            // お知らせには反応しないが、「テスト表示」(test()のレス)では試せるようにする
            let linkable = !post.system || post.name == "テスト";
            // 下の字幕のレスの翻訳: 字幕には番号(tid)を付けて出し、翻訳が返ってきたら
            // 同じ番号で送る(字幕の画面が、今出ている字幕の翻訳だけを下に添える)。
            // 読み上げはしない。右のレス一覧・ニコ生風には出さない。
            static SUBTITLE_TID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let tid = (settings.subtitle_translate && linkable && !aa && !post.body.trim().is_empty())
                .then(|| SUBTITLE_TID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
            let mut subtitle_json = post.to_json(aa);
            if let Some(tid) = tid {
                subtitle_json["tid"] = serde_json::json!(tid);
                let body = post.body.clone();
                let bridge = bridge.clone();
                let ja_target = match settings.subtitle_translate_target.trim() {
                    "" | "ja" => "en".to_string(),
                    t => t.to_string(),
                };
                tauri::async_runtime::spawn(async move {
                    // 日本語なら設定の言語(既定は英語)に、それ以外なら日本語に
                    let target = if text::has_japanese(&body) { ja_target.as_str() } else { "ja" };
                    match crate::voice::translate_detect(&source::http_client(), &body, target).await {
                        // 翻訳先と同じ言語だった・訳しても同じだった時は出さない
                        Ok((t, src)) if src != target && t.trim() != body.trim() => bridge.push_subtitle_translation(tid, &t),
                        Ok(_) => {}
                        Err(e) => log::warn!("字幕の翻訳: {e}"),
                    }
                });
            }
            bridge.show_subtitle(subtitle_json, shown_ms);
            bridge.push_comment(post.to_json(aa));
            // 本文がルールに当てはまったら、字幕と同時にリアクションも飛ばす(AAは除く)。
            if settings.reaction_link && linkable && !aa {
                for id in text::matched_reactions(&post.body, &compiled.reaction_rules, settings.reaction_link_max) {
                    if let Err(e) = crate::show_reaction(&self.app, &id, "comment") {
                        log::warn!("コメントからのリアクションを表示できませんでした: {e}");
                    }
                }
            }

            if !spoken.trim().is_empty() {
                match engine {
                    crate::tts::EngineKind::Command => run_speech_command(&settings, &post, &spoken),
                    crate::tts::EngineKind::Bouyomi => crate::tts::speak_bouyomi(&settings, &spoken),
                    _ => {
                        if let Some(w) = wav {
                            if let Err(e) = crate::tts::play_wav(w, &settings) {
                                log::warn!("読み上げ: {e}");
                            }
                        }
                    }
                }
            }

            let interval = next_interval(backlog, shown_ms, speech_ms, crate::tts::plays_itself(&settings), &settings);
            self.wait_or_skip(interval).await;
            if cut_speech && crate::tts::plays_itself(&settings) {
                crate::tts::stop();
            }
        }
    }
}

/// 受信側があればそこから受け取り、無ければずっと待つ(tokio::select!用)。
async fn recv_or_pending<T>(rx: &mut Option<tokio::sync::mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// 字幕を表示しておく時間。通常のレスは文字数に応じて延ばす(上限あり)。
/// AAは「AAモードで字幕が消える時間」で固定。
fn display_duration(post: &Post, aa: bool, s: &CommentSettings) -> u64 {
    if aa {
        return s.aa_display_ms;
    }
    let chars = post.body.chars().filter(|c| !c.is_whitespace()).count() as u64;
    let extended = s.display_ms.saturating_add(chars.saturating_mul(s.display_per_char_ms));
    if s.display_per_char_ms == 0 {
        s.display_ms
    } else {
        extended.min(s.display_max_ms.max(s.display_ms))
    }
}

/// 読み上げにかかる時間の目安(読み上げる文字数 × 1文字あたりの時間)。
fn speech_duration(spoken: &str, s: &CommentSettings) -> u64 {
    let chars = spoken.chars().filter(|c| !c.is_whitespace()).count() as u64;
    chars.saturating_mul(s.speech_ms_per_char)
}

/// 読み上げ時間の目安が最大待ち時間を超える長いレスは、収まる所までで打ち切って
/// 「以下略」を付ける(読み上げが終わる前に次のレスへ進んで、声と字幕がずれないように)。
fn fit_speech_to_max_wait(spoken: String, s: &CommentSettings) -> String {
    if s.max_wait_ms == 0 || s.speech_ms_per_char == 0 || speech_duration(&spoken, s) <= s.max_wait_ms {
        return spoken;
    }
    let limit = (s.max_wait_ms / s.speech_ms_per_char) as usize;
    let limit = limit.saturating_sub(SPEECH_CUT_SUFFIX.chars().count()); // 「、以下略」を読む分
    cut_speech_with_suffix(&spoken, limit)
}

const SPEECH_CUT_SUFFIX: &str = "、以下略";

/// 空白以外を`limit`文字まで残して「、以下略」を付ける。
fn cut_speech_with_suffix(spoken: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in spoken.chars() {
        if !c.is_whitespace() {
            if count == limit {
                break;
            }
            count += 1;
        }
        out.push(c);
    }
    out.truncate(out.trim_end().len());
    out + SPEECH_CUT_SUFFIX
}

/// 実際に作った音声(`actual_ms`)が`target_ms`を超えた時に、その声の実際の速さで
/// 収まるように削り直した読み上げの文字列(削れない時はNone)。声によって読む速さが
/// 違うので、文字数の目安(fit_speech_to_max_wait)だけでは収まらないことがある。
fn shorten_speech_to_duration(spoken: &str, actual_ms: u64, target_ms: u64) -> Option<String> {
    let suffix_chars = SPEECH_CUT_SUFFIX.chars().count();
    let base = spoken.strip_suffix(SPEECH_CUT_SUFFIX).unwrap_or(spoken);
    let had_suffix = base.len() != spoken.len();
    let n = base.chars().filter(|c| !c.is_whitespace()).count();
    let total = n + if had_suffix { suffix_chars } else { 0 };
    if n == 0 || total == 0 || actual_ms == 0 {
        return None;
    }
    // 開始・終了の無音などで文字数に比例しない分もあるので、少し余裕を見て削る
    let per_char = actual_ms as f64 / total as f64;
    let fit = ((target_ms as f64 * 0.95) / per_char) as usize;
    let limit = fit.saturating_sub(suffix_chars).min(n * 9 / 10);
    (limit > 0).then(|| cut_speech_with_suffix(base, limit))
}

/// 次のレスを出すまでの間隔。溜まっている時(ターボ)は短い間隔で追いつくことを
/// 優先し、そうでなければ読み上げ間隔・読み上げ時間の目安・(設定がONなら)字幕の
/// 表示時間のうち一番長いもの。
/// `exact_speech`はReaCastが鳴らすソフト(VOICEVOX等。speech_msが本当の長さ)の時。
/// その場合はターボ中も読み終わるまで待つ(次の声で前の声を止めると途中で途切れるため。
/// 代わりに話速を上げて追いつく)。民安★TALK・棒読みちゃんはソフト側が順番に読むので、
/// これまでどおりターボ間隔で渡す。
fn next_interval(backlog: usize, shown_ms: u64, speech_ms: u64, exact_speech: bool, s: &CommentSettings) -> u64 {
    if backlog >= s.turbo_threshold.max(1) {
        if exact_speech {
            s.turbo_interval_ms.max(speech_ms)
        } else {
            s.turbo_interval_ms
        }
    } else if s.wait_for_display {
        s.read_interval_ms.max(shown_ms).max(speech_ms)
    } else {
        s.read_interval_ms.max(speech_ms)
    }
}

/// 読み上げ用の文字列を作る(AAなら固定文字列か「」の中だけ、読み方ルール、
/// レス番号、文字数制限)。
fn speech_text(post: &Post, aa: bool, s: &CommentSettings, c: &Compiled) -> String {
    let body = if aa {
        let quoted = if s.aa_read_quoted_only { text::quoted_only(&post.body) } else { String::new() };
        if quoted.is_empty() { s.aa_fixed_reading.clone() } else { quoted }
    } else {
        post.body.clone()
    };
    let body = text::apply_reading_rules(&body.replace('\n', " "), &c.reading_rules);
    let with_no = if !post.system && post.no > 0 && (if aa { s.aa_read_res_number } else { s.read_res_number }) {
        format!("{}、{}", post.no, body)
    } else {
        body
    };
    text::truncate_chars(&with_no, s.max_chars)
}

/// 読み上げコマンドを起動する。シェルを経由せず引数の配列で直接起動するので、
/// レス本文の内容がコマンドとして解釈されることはない。終了は待たない
/// (vrx.exeは民安★TALKに文字列を渡したらすぐ終わる。順番の管理は読み上げ
/// 間隔で行う=SpeechCastMeと同じ方式)。
fn run_speech_command(s: &CommentSettings, post: &Post, spoken: &str) {
    let values = text::TokenValues { res: spoken, no: post.no, name: &post.name, time: &post.date };
    let args: Vec<String> = text::split_args(&s.command_args).iter().map(|a| text::fill_tokens(a, &values)).collect();
    let mut cmd = std::process::Command::new(s.command_path.trim());
    cmd.args(&args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        // 終了コードの回収だけ別スレッドで行う(プロセスハンドルを残さないため)
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("読み上げコマンドの起動に失敗しました: {e}"),
    }
}

/// 初めて使う時(と「全部の設定を最初の状態に戻す」)の設定。既定値に加えて、
/// VOICEVOXが入っていれば(または起動中なら)読み上げソフトをVOICEVOXにしておく
/// (無ければ今までどおり「コマンドで渡す(民安★TALK)」)。
pub fn initial_settings() -> CommentSettings {
    let mut s = CommentSettings::default();
    let mut probe = s.clone();
    probe.speech_engine = "voicevox".into();
    if crate::tts::find_exe(&probe).is_some() {
        s.speech_engine = "voicevox".into();
    }
    s
}

pub fn load_settings(store: &ConfigStore) -> CommentSettings {
    let Some(saved) = store.get(SETTINGS_KEY) else {
        // 初めて使う時
        let s = initial_settings();
        log::info!("コメント読み上げ・字幕: 初めての起動なので、最初の設定を作りました(読み上げソフト: {})", s.speech_engine);
        let _ = save_settings(store, &s);
        return s;
    };
    let mut s: CommentSettings = serde_json::from_value(saved).unwrap_or_default();
    if !s.reading_presets_v2 {
        add_reading_presets(&mut s);
        let _ = save_settings(store, &s);
    }
    if !s.reaction_presets_v2 || !s.reaction_presets_v3 {
        add_reaction_presets(&mut s);
        let _ = save_settings(store, &s);
    }
    // v1.5.12はexeの場所が1つだけだった
    let legacy = std::mem::take(&mut s.tts_exe_path);
    if !legacy.trim().is_empty() {
        s.tts_exe_paths.entry(s.speech_engine.clone()).or_insert(legacy);
    }
    s
}

/// 以前から使っている人の読み方に、v1.5.6で増やした既定の読み方を一度だけ足す
/// (同じ行が既にあれば足さない)。以前の既定のURLの読み(リンク)は新しい読みにする。
fn add_reading_presets(s: &mut CommentSettings) {
    let mut lines: Vec<String> = s
        .reading_rules
        .lines()
        .map(|l| match OLD_URL_READINGS.iter().position(|old| *old == l) {
            Some(i) => DEFAULT_READING_RULES[i].to_string(),
            None => l.to_string(),
        })
        .collect();
    for rule in DEFAULT_READING_RULES {
        if !lines.iter().any(|l| l == rule) {
            lines.push(rule.to_string());
        }
    }
    s.reading_rules = lines.join("\n");
    s.reading_presets_v2 = true;
}

/// 以前から使っている人のリアクション連動に、まだルールが1つも無いリアクションの
/// 既定のルールを一度だけ足す(自分で作ったルール・消したルールはそのまま)。
fn add_reaction_presets(s: &mut CommentSettings) {
    let rule_id = |l: &str| l.rsplit_once('/').map(|(_, id)| id.trim().to_string());
    let have: std::collections::HashSet<String> = s.reaction_rules.lines().filter_map(rule_id).collect();
    let mut lines: Vec<String> = s.reaction_rules.lines().map(str::to_string).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    for rule in DEFAULT_REACTION_RULES {
        if rule_id(rule).is_some_and(|id| !have.contains(&id)) {
            lines.push(rule.to_string());
        }
    }
    s.reaction_rules = lines.join("\n");
    s.reaction_presets_v2 = true;
    s.reaction_presets_v3 = true;
}

fn save_settings(store: &ConfigStore, settings: &CommentSettings) -> Result<(), String> {
    let v = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    store.set(SETTINGS_KEY.to_string(), v).map_err(|e| e.to_string())
}

// ---------------- Tauriコマンド ----------------

type Engine<'a> = tauri::State<'a, Arc<CommentEngine>>;

#[tauri::command]
pub fn comments_get_settings(engine: Engine) -> CommentSettings {
    engine.settings()
}

#[tauri::command]
pub fn comments_status(engine: Engine) -> CommentStatus {
    engine.status()
}

/// ウィンドウの作成・破棄を伴うことがあるので、メインスレッドを塞がないよう
/// async指定にしている(lib.rsのopen_region_pickerのコメント参照)。
#[tauri::command(async)]
pub fn comments_save_settings(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    settings: CommentSettings,
) -> Result<(), String> {
    save_settings(&store, &settings)?;
    if let Err(e) = hotkeys::apply(&app, &settings) {
        log::warn!("{e}");
    }
    engine.apply_settings(settings);
    crate::sync_subtitle_window(&app);
    // 話した字幕のON/OFF・聞き取る言葉が変わっていれば、Edgeの窓を開く・閉じる・開き直す
    crate::voice::sync(&app);
    Ok(())
}

/// 「全部の設定を最初の状態に戻す」: コメント読み上げ・字幕の設定を全部、初めて使う時の
/// 状態に戻して、戻した設定を返す(設定画面が表示し直す)。
#[tauri::command(async)]
pub fn comments_reset_settings(app: AppHandle, engine: Engine, store: tauri::State<ConfigStore>) -> Result<CommentSettings, String> {
    let settings = initial_settings();
    log::info!("コメント読み上げ・字幕: 全部の設定を最初の状態に戻しました");
    comments_save_settings(app, engine, store, settings.clone())?;
    Ok(settings)
}

/// 次スレのタイトルと>>1の本文を、設定のテンプレートと今のスレッドから作る。
fn build_draft(s: &CommentSettings, cur_title: &str, cur_url: &str) -> Result<(String, String), String> {
    let tmpl = if s.create_title_template.trim().is_empty() { "{next}" } else { s.create_title_template.as_str() };
    let next = text::increment_last_number(cur_title);
    if (tmpl.contains("{next}") || tmpl.contains("{n}")) && next.is_none() {
        return Err(format!(
            "今のタイトル「{cur_title}」に数字が無いため、次のタイトルを自動で決められません(タイトルの設定を変えてください)"
        ));
    }
    let next = next.unwrap_or_default();
    let title = tmpl.replace("{next}", &next).replace("{n}", &text::last_number(&next).unwrap_or_default());
    let body = s
        .create_body_template
        .replace("{prevUrl}", cur_url)
        .replace("{prevTitle}", cur_title)
        .replace("{title}", &title);
    if title.trim().is_empty() {
        return Err("次スレのタイトルが空です".into());
    }
    if body.trim().is_empty() {
        return Err(">>1の本文が空です".into());
    }
    Ok((title, body))
}

/// テンプレートの編集中に、入力途中の設定でタイトルと>>1を作って見せる。取得して
/// いない時は見本のスレッド(sample: true)で作る。
#[tauri::command]
pub fn comments_preview_draft(engine: Engine, settings: CommentSettings) -> serde_json::Value {
    let status = engine.status();
    let (cur_title, cur_url, sample) = match status.thread_title {
        Some(t) => (t, status.thread_url.unwrap_or_default(), false),
        None => ("配信スレ★539".to_string(), "https://bbs.jpnkn.com/test/read.cgi/板の名前/1700000000/".to_string(), true),
    };
    match build_draft(&settings, &cur_title, &cur_url) {
        Ok((title, body)) => serde_json::json!({ "title": title, "body": body, "sample": sample }),
        Err(e) => serde_json::json!({ "error": e, "sample": sample }),
    }
}

/// 今読んでいるスレッドの>>1の本文を、テンプレートとして使える形にして返す
/// (同じ掲示板のスレッドのURLは{prevUrl}にする)。
#[tauri::command]
pub fn comments_first_post_template(engine: Engine) -> Result<String, String> {
    let body = engine.first_post().ok_or("今読んでいるスレッドの>>1がありません(取得を開始してから押してください)")?;
    Ok(thread_urls_to_placeholder(&body))
}

/// 本文中のスレッドのURL(…/read.cgi/…)を{prevUrl}にする。
fn thread_urls_to_placeholder(body: &str) -> String {
    let re = regex::Regex::new(r"https?://[^\s]+/read\.cgi/[^\s]+").unwrap();
    re.replace_all(body, "{prevUrl}").into_owned()
}

/// コメントとリアクションの連動の既定のルール(「既定のルールに戻す」用)。
#[tauri::command]
pub fn comments_default_reaction_rules() -> String {
    DEFAULT_REACTION_RULES.join("\n")
}

/// 今のレスを飛ばして次のレスへ進む(設定画面のボタン・ホットキー)。
#[tauri::command]
pub fn comments_skip(engine: Engine) {
    engine.skip();
}

/// 読み上げの一時停止/再開を切り替える。戻り値は切り替えた後に一時停止中か。
#[tauri::command]
pub fn comments_toggle_pause(engine: Engine) -> bool {
    engine.toggle_pause()
}

/// ホットキーの割り当て中は、登録済みのホットキーを一旦外す。
/// ReaCastの画面で押されたキー(shared/hotkey-forward.js)をホットキーと照合する
#[tauri::command]
pub fn comments_hotkey_from_page(app: AppHandle, code: String, ctrl: bool, alt: bool, shift: bool, meta: bool) {
    hotkeys::page_key(&app, &code, ctrl, alt, shift, meta);
}

#[tauri::command]
pub fn comments_suspend_hotkeys(app: AppHandle) {
    hotkeys::suspend(&app);
}

/// 設定のホットキーを登録し直す(登録できなかったキーがあればその説明を返す)。
#[tauri::command]
pub fn comments_apply_hotkeys(app: AppHandle, engine: Engine) -> Result<(), String> {
    hotkeys::apply(&app, &engine.settings())
}

#[tauri::command(async)]
pub fn comments_start(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    url: String,
) -> Result<(), String> {
    engine.start(&url)?;
    save_settings(&store, &engine.settings())?;
    crate::sync_subtitle_window(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn comments_stop(app: AppHandle, engine: Engine) {
    engine.stop();
    crate::sync_subtitle_window(&app);
}

#[tauri::command]
pub fn comments_get_thread(engine: Engine) -> serde_json::Value {
    engine.thread_posts_json()
}

#[tauri::command]
pub fn comments_replay(engine: Engine, no: u32) -> Result<(), String> {
    engine.replay(no)
}

/// 「スレッドのレス」タブの右クリック「ここから読み始める」
#[tauri::command]
pub fn comments_read_from(engine: Engine, no: u32) -> Result<usize, String> {
    engine.read_from(no)
}

/// 右クリック「この言葉で検索」: いつものブラウザでGoogle検索を開く
#[tauri::command]
pub fn comments_search_web(text: String) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let url = reqwest::Url::parse_with_params("https://www.google.com/search", &[("q", text)]).map_err(|e| e.to_string())?;
    // explorerにURLを渡すと「ドキュメント」フォルダが開いてしまう(本人のPCで発生)ので、
    // URLを既定のブラウザで開く標準の方法(url.dllのFileProtocolHandler)を使う
    #[cfg(target_os = "windows")]
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url.as_str()])
        .spawn()
        .map_err(|e| format!("ブラウザを開けませんでした: {e}"))?;
    #[cfg(not(target_os = "windows"))]
    let _ = url;
    Ok(())
}

/// テスト表示。取得していなくてもデスクトップ字幕で確認できるよう、表示が
/// 終わるまでの間だけ字幕ウィンドウを作っておく。
#[tauri::command(async)]
pub fn comments_test(app: AppHandle, engine: Engine, text: String) {
    let hold = Duration::from_millis(engine.settings().display_ms.max(1000) + 4000);
    let created = app.get_webview_window("subtitle").is_none();
    engine.hold_desktop(hold);
    crate::sync_subtitle_window(&app);
    if created && app.get_webview_window("subtitle").is_some() {
        // 作った直後は画面の読み込みが終わっていないので、少し待ってから表示する
        // (読み込みが遅れても、繋いできた時点で表示中の字幕は送り直される)
        std::thread::sleep(Duration::from_millis(600));
    }
    engine.test(&text);
    std::thread::spawn(move || {
        std::thread::sleep(hold + Duration::from_millis(200));
        crate::sync_subtitle_window(&app);
    });
}

/// 右のレス一覧・ニコ生風の表示だけに、見た目の確認用のレスを出す
/// (字幕・読み上げはしない)。
#[tauri::command]
pub fn comments_test_list(engine: Engine, text: String, name: String) {
    let mut post = Post::system(text);
    post.system = false;
    post.no = 1;
    post.name = name;
    post.date = "2026/10/08(木) 21:00:00.00 ID:test".into();
    post.icon = engine.pick_icon("bbs");
    // AAとして判定されるかも本番と同じにする(ニコ生風のAAの流れ方を試せるように)
    let aa = {
        let st = engine.state.lock().unwrap();
        detect_aa(&st.settings, &st.compiled, &post)
    };
    let json = post.to_json(aa);
    let bridge = engine.bridge();
    bridge.push_comment(json.clone());
    // ニコ生風の「届いたらすぐ流す」がONの時は、ニコ生風はこちらを流す(OFFの時は無視される)
    bridge.push_nico(json);
}

/// YouTubeのURLで、配信とチャットに接続できるかを確かめる(取得は開始しない)。
#[tauri::command]
pub async fn comments_check_youtube(target: String) -> youtube::CheckResult {
    youtube::check(&target).await
}

/// Twitchのチャンネルに接続できるかを確かめる(数秒だけ繋いでコメントを受け取る)。
#[tauri::command]
pub async fn comments_check_twitch(channel: String) -> serde_json::Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let channel = match twitch::parse_channel(&channel) {
        Ok(c) => c,
        Err(e) => return serde_json::json!({ "ok": false, "message": e, "samples": [] }),
    };
    let (mut write, mut read) = match tokio_tungstenite::connect_async("wss://irc-ws.chat.twitch.tv:443").await {
        Ok((ws, _)) => ws.split(),
        Err(e) => return serde_json::json!({ "ok": false, "message": format!("Twitchへの接続に失敗しました: {e}"), "samples": [] }),
    };
    for line in [
        "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
        "PASS SCHMOOPIIE".to_string(),
        format!("NICK justinfan{}", rand::random::<u32>() % 90000 + 10000),
        format!("JOIN #{channel}"),
    ] {
        let _ = write.send(Message::Text(line)).await;
    }
    let mut joined = false;
    let mut samples: Vec<String> = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < end {
        let left = end - tokio::time::Instant::now();
        let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(left, read.next()).await else {
            break;
        };
        for line in text.split("\r\n") {
            if line.contains(&format!(" JOIN #{channel}")) {
                joined = true;
            } else if let Some(m) = twitch::parse_privmsg(line) {
                if samples.len() < 5 {
                    samples.push(format!("{}: {}", m.author, m.text));
                }
            }
        }
    }
    let _ = write.close().await;
    let message = match (joined, samples.len()) {
        (false, _) => "チャンネルに参加できませんでした".to_string(),
        (true, 0) => "接続できました(8秒の間にコメントはありませんでした。配信していないか、コメントが少ない可能性があります)".to_string(),
        (true, n) => format!("接続できました(8秒の間に{n}件以上のコメントを受信)"),
    };
    serde_json::json!({ "ok": joined, "message": message, "channel": channel, "samples": samples })
}

/// 板のスレッド一覧(手動でスレッドを選ぶ用)。urlは板かスレッドのURL。
#[tauri::command]
pub async fn comments_list_threads(engine: Engine<'_>, url: String) -> Result<serde_json::Value, String> {
    let (board, _) = source::parse_url(&url)?;
    let list = source::list_threads(&source::http_client(), &board).await?;
    let current = engine.status().thread_url;
    let mut items: Vec<serde_json::Value> = list
        .iter()
        .map(|t| {
            let read_url = ThreadRef { board: board.clone(), key: t.key.clone() }.read_url();
            serde_json::json!({
                "key": t.key,
                "title": t.title,
                "count": t.count,
                "url": read_url,
                "current": current.as_deref() == Some(read_url.as_str()),
            })
        })
        .collect();
    items.sort_by_key(|v| std::cmp::Reverse(v["key"].as_str().and_then(|k| k.parse::<u64>().ok()).unwrap_or(0)));
    Ok(serde_json::json!({ "threads": items }))
}

/// 今読んでいるスレッドに書き込む(「スレッドのレス」タブから)。名前・メール欄は設定のもの。
#[tauri::command]
pub async fn comments_post(engine: Engine<'_>, body: String) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("本文が空です".into());
    }
    let url = engine
        .status()
        .thread_url
        .ok_or("読み込み中のスレッドがありません(「開始」を押してから書き込んでください)")?;
    let (board, key) = source::parse_url(&url)?;
    let key = key.ok_or("スレッドのURLが分かりません")?;
    let settings = engine.settings();
    source::post_reply(
        &source::http_client(),
        &ThreadRef { board, key },
        &settings.post_name,
        &settings.post_mail,
        body.trim_end(),
    )
    .await?;
    log::info!("スレッドに書き込みました: {url}");
    Ok(())
}

/// 掲示板の読み込み先を、選んだスレッドに切り替える。
#[tauri::command]
pub fn comments_switch_thread(engine: Engine, url: String) -> Result<(), String> {
    engine.switch_thread(&url)
}


#[tauri::command]
pub fn comments_desktop_adjusting(engine: Engine) -> bool {
    engine.adjusting()
}

/// デスクトップ字幕をマウスで動かすモードの開始/終了。終了した時点の
/// ウィンドウの位置・大きさを保存する。
#[tauri::command(async)]
pub fn comments_desktop_adjust(
    app: AppHandle,
    engine: Engine,
    store: tauri::State<ConfigStore>,
    on: bool,
) -> Result<(), String> {
    if !on {
        if let Some(win) = app.get_webview_window("subtitle") {
            if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
                let mut s = engine.settings();
                s.desktop_rect = Some(DesktopRect { x: pos.x, y: pos.y, width: size.width, height: size.height });
                save_settings(&store, &s)?;
                engine.apply_settings(s.clone());
                let _ = app.emit("comments:settings", &s);
            }
        }
    }
    // 位置の調整を始める時は、字幕の窓を作り直す(窓が見えないままになっていた時でも、
    // 「マウスで位置を調整」を押せば必ず出るように)
    if on {
        if let Some(win) = app.get_webview_window("subtitle") {
            let _ = win.destroy();
        }
    }
    engine.set_adjusting(on);
    crate::sync_subtitle_window(&app);
    let _ = app.emit("subtitle:adjust", on);
    Ok(())
}

/// デスクトップ字幕の位置を、モニター・割合の指定(既定の位置)に戻す。
#[tauri::command(async)]
pub fn comments_desktop_reset(app: AppHandle, engine: Engine, store: tauri::State<ConfigStore>) -> Result<(), String> {
    let mut s = engine.settings();
    s.desktop_rect = None;
    save_settings(&store, &s)?;
    engine.apply_settings(s.clone());
    let _ = app.emit("comments:settings", &s);
    crate::sync_subtitle_window(&app);
    Ok(())
}

/// 読み上げソフト(exe)を選ぶダイアログ。
#[tauri::command(async)]
pub fn comments_pick_exe(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("読み上げソフトを選択")
        .add_filter("実行ファイル", &["exe"])
        .blocking_pick_file()
        .map(|p| p.to_string())
}

/// アイコン画像のフォルダを選ぶダイアログ。
#[tauri::command(async)]
pub fn comments_pick_folder(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("アイコン画像のフォルダを選択")
        .blocking_pick_folder()
        .map(|p| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(no: u32, body: &str) -> Post {
        Post { no, name: "名無し".into(), date: String::new(), body: body.into(), system: false, icon: None, source: "bbs", avatar: None, catch_up: false, paid: None }
    }

    #[test]
    fn speech_text_for_normal_and_aa_posts() {
        let mut s = CommentSettings::default();
        let c = Compiled::from(&s);
        // 既定は本文だけ(レス番号は読まない)
        assert_eq!(speech_text(&post(69, "おつかれ www"), false, &s, &c), "おつかれ ワラワラ");
        assert_eq!(speech_text(&post(5, "(´Д｀)「やあ」"), true, &s, &c), "やあ");
        assert_eq!(speech_text(&post(5, "(´Д｀)"), true, &s, &c), "アスキーアート");
        s.read_res_number = true;
        assert_eq!(speech_text(&post(69, "おつかれ"), false, &s, &c), "69、おつかれ");
        s.read_res_number = false;
        s.max_chars = 4;
        assert_eq!(speech_text(&post(1, "あいうえおか"), false, &s, &c), "あいうえ");
        // お知らせにはレス番号を付けない
        assert_eq!(speech_text(&Post::system("次スレです"), false, &s, &c), "次スレで");
    }

    #[test]
    fn next_thread_draft_and_template_import() {
        let mut s = CommentSettings::default();
        s.create_title_template = "{next}".into();
        s.create_body_template = "前スレ\n{prevUrl}\n{prevTitle}→{title}".into();
        let (title, body) = build_draft(&s, "配信スレ★539", "https://example.com/test/read.cgi/b/1/").unwrap();
        assert_eq!(title, "配信スレ★540");
        assert_eq!(body, "前スレ\nhttps://example.com/test/read.cgi/b/1/\n配信スレ★539→配信スレ★540");
        s.create_body_template = " ".into();
        assert!(build_draft(&s, "配信スレ★539", "").is_err());
        assert_eq!(
            thread_urls_to_placeholder("前スレ\nhttps://bbs.jpnkn.com/test/read.cgi/ao/1790705505/\nルール: https://example.com/rule"),
            "前スレ\n{prevUrl}\nルール: https://example.com/rule"
        );
    }

    #[test]
    fn default_reading_rules_read_naturally() {
        assert_eq!(text::parse_reading_rules(&DEFAULT_READING_RULES.join("\n")).len(), DEFAULT_READING_RULES.len());
        let s = CommentSettings::default();
        let c = Compiled::from(&s);
        let read = |body: &str| speech_text(&post(5, body), false, &s, &c);
        assert_eq!(read(">>12 わかる"), " わかる");
        assert_eq!(read("見て https://example.com/a?b=1"), "見て ユーアールエル省略");
        assert_eq!(read("ID 12345678 です"), "ID 数字省略 です");
        assert_eq!(read("すごい!!!!"), "すごい！");
        assert_eq!(read("やったー😀👍"), "やったー");
        assert_eq!(read("kwsk"), "くわしく");
        assert_eq!(read("8888"), "パチパチ");
    }

    #[test]
    #[ignore] // VOICEVOXが入っているPCで: 初めて使う時の読み上げソフトがVOICEVOXになる
    fn initial_settings_pick_voicevox_when_installed() {
        assert_eq!(initial_settings().speech_engine, "voicevox");
    }

    #[test]
    fn posts_with_the_same_time_are_not_lost() {
        let raw = |no: u32, body: &str| source::RawPost {
            no,
            name: "名無しさん".into(),
            date: "2026/10/21(水) 01:06:55.31".into(),
            body_html: body.into(),
        };
        // 即時通知が926の内容を927として届けた → 定期取得の本物の927は内容が違うので読む
        let mut seen = SeenPosts::default();
        assert!(seen.from_fast(&raw(927, "ドドドドブーン")));
        assert!(!seen.from_fast(&raw(927, "アヘ顔")));
        assert!(!seen.from_dat(&raw(926, "ドドドドブーン"))); // 内容は読み済み
        assert!(seen.from_dat(&raw(927, "アヘ顔"))); // 本物の927は読む
        assert!(!seen.from_dat(&raw(927, "アヘ顔")));
        // 普段: 即時通知で読んだものは定期取得では読まない(空白・タグの違いは同じとみなす)
        let mut seen = SeenPosts::default();
        assert!(seen.from_fast(&raw(5, "テスト<br>2行目")));
        assert!(!seen.from_dat(&raw(5, "テスト <br> 2行目")));
        // 定期取得で読んだものは即時通知では読まない
        assert!(seen.from_dat(&raw(6, "a")));
        assert!(!seen.from_fast(&raw(6, "a")));
    }

    #[test]
    fn resumes_only_after_an_unexpected_end() {
        let thread = "https://bbs.jpnkn.com/test/read.cgi/x/123/";
        let now = 10_000_000;
        let p = ReadProgress { thread: thread.into(), no: 120, clean: false, at_ms: now - 60_000 };
        assert_eq!(resume_point(&p, thread, 150, now), Some(120));
        // 未読が無い・別のスレッド・停止で止めた・1時間以上前 → 続きから読まない
        assert_eq!(resume_point(&p, thread, 120, now), None);
        assert_eq!(resume_point(&p, "https://bbs.jpnkn.com/test/read.cgi/x/456/", 150, now), None);
        assert_eq!(resume_point(&ReadProgress { clean: true, ..p.clone() }, thread, 150, now), None);
        assert_eq!(resume_point(&ReadProgress { at_ms: now - RESUME_WITHIN_MS - 1, ..p.clone() }, thread, 150, now), None);
    }

    #[test]
    fn every_reaction_has_a_default_rule() {
        let emoji_set = include_str!("../../../frontend/shared/emoji-set.js");
        let ids: Vec<&str> = emoji_set
            .split("id: '")
            .skip(1)
            .map(|rest| &rest[..rest.find('\'').unwrap()])
            .collect();
        assert!(ids.len() >= 50);
        for id in ids {
            assert!(
                DEFAULT_REACTION_RULES.iter().any(|r| r.ends_with(&format!("/{id}"))),
                "{id}の既定のルールがありません"
            );
        }
    }

    #[test]
    fn reaction_presets_are_added_once_to_saved_settings() {
        let old = serde_json::json!({ "reactionRules": "おめ/cake\n自分の言葉/mango\n" });
        let mut s: CommentSettings = serde_json::from_value(old).unwrap();
        assert!(!s.reaction_presets_v2);
        add_reaction_presets(&mut s);
        let lines: Vec<&str> = s.reaction_rules.lines().collect();
        assert_eq!(&lines[..2], &["おめ/cake", "自分の言葉/mango"]); // 自分のルールはそのまま先頭に
        // cake・mangoは自分のルールがあるので足さない、他は足す
        assert!(!lines[2..].iter().any(|l| l.ends_with("/cake") || l.ends_with("/mango")));
        assert!(lines.iter().any(|l| l.ends_with("/peach")));
        assert!(lines.iter().any(|l| l.ends_with("/kusa")));
        assert!(s.reaction_presets_v2);
        assert!(CommentSettings::default().reaction_presets_v2);
    }

    #[test]
    fn reading_presets_are_added_once_to_saved_settings() {
        // v1.5.5までの設定(reading_presets_v2が無い)を読み込んだ時
        let old = serde_json::json!({ "readingRules": format!("{}\n自分の/読み方", OLD_URL_READINGS[0]) });
        let mut s: CommentSettings = serde_json::from_value(old).unwrap();
        assert!(!s.reading_presets_v2);
        add_reading_presets(&mut s);
        let lines: Vec<&str> = s.reading_rules.lines().collect();
        assert_eq!(lines[0], DEFAULT_READING_RULES[0]); // 以前の既定のURLの読みは新しい読みに
        assert_eq!(lines[1], "自分の/読み方"); // 自分で足した行はそのまま
        assert_eq!(lines.len(), 1 + DEFAULT_READING_RULES.len());
        assert!(s.reading_presets_v2);
        // 新しく作った設定では足し済み扱い
        assert!(CommentSettings::default().reading_presets_v2);
    }

    #[test]
    fn default_reaction_rules_are_valid() {
        // 全部の行が正規表現として正しく、IDがリアクションの一覧(emoji-set.js)にある
        let rules = text::parse_reaction_rules(&DEFAULT_REACTION_RULES.join("\n"));
        assert_eq!(rules.len(), DEFAULT_REACTION_RULES.len());
        let emoji_set = include_str!("../../../frontend/shared/emoji-set.js");
        for line in DEFAULT_REACTION_RULES {
            let id = &line[line.rfind('/').unwrap() + 1..];
            assert!(emoji_set.contains(&format!("id: '{id}'")), "{id}がemoji-set.jsにありません");
        }
        let m = |body: &str| text::matched_reactions(body, &rules, 2);
        // 話した字幕(音声認識の1文)でも当てはまる
        assert_eq!(m("おめでとうございます"), vec!["cake"]);
        assert_eq!(m("いやかわいいね"), vec!["heart"]);
        assert_eq!(m("ナイスナイス"), vec!["nice"]);
        assert_eq!(m("みんなお疲れ様でした"), vec!["otsu"]);
        assert!(m("今日はこれから雑談をします").is_empty());
        assert_eq!(m("マンゴー食べたい"), vec!["mango"]);
        assert_eq!(m("ブーイング"), vec!["boo"]);
        assert_eq!(m("ドヤ顔やめろ"), vec!["tengu"]);
        assert_eq!(m("↑"), vec!["arrowup"]);
        assert_eq!(m("もうこんな時間か"), vec!["clock5"]);
        assert_eq!(m("RTA走者"), vec!["stopwatch"]);
        assert!(m("上手い").first().map(|s| s.as_str()) != Some("arrowup"));
        assert_eq!(m("wwwww"), vec!["kusa"]);
        assert_eq!(m("8888"), vec!["clap"]);
        assert_eq!(m("おつー"), vec!["otsu"]);
        assert_eq!(m("誕生日おめでとう！"), vec!["cake"]);
        // 誤爆しやすい言葉
        assert!(m("少しだけ").is_empty());
        assert!(m("8時に始めます").is_empty());
        assert!(m("今日はよろしく").is_empty());
    }

    #[test]
    fn long_speech_is_cut_to_the_max_wait() {
        let mut s = CommentSettings::default(); // 160ms/文字、最大20000ms=125文字
        let short = "あ".repeat(100);
        assert_eq!(fit_speech_to_max_wait(short.clone(), &s), short);
        let cut = fit_speech_to_max_wait("い".repeat(270), &s);
        assert!(cut.ends_with("、以下略"));
        assert!(speech_duration(&cut, &s) <= s.max_wait_ms);
        assert_eq!(cut.chars().filter(|c| *c == 'い').count(), 121);
        s.max_wait_ms = 0;
        assert_eq!(fit_speech_to_max_wait("い".repeat(270), &s).chars().count(), 270);
    }

    #[test]
    #[ignore] // 実際のVOICEVOXで、遅い声の長いレスが最大待ち時間に収まるまで削り直されるか
    fn shortens_slow_voice_with_voicevox() {
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speaker = "3".into();
        s.tts_speed = 0.6;
        s.speech_ms_per_char = 160;
        let body = "このスレッドは配信の実況用です。荒らしはスルーしてください。次スレは九百七十を踏んだ人が立ててください。".repeat(3);
        let mut spoken = fit_speech_to_max_wait(body, &s);
        let voice = crate::tts::pick_voice(&s, "bbs", "");
        let target = s.max_wait_ms - 150;
        let (_, mut ms) = tauri::async_runtime::block_on(crate::tts::synthesize_or_launch(&voice, &spoken)).unwrap().unwrap();
        eprintln!("1回目: {ms}ms {}文字", spoken.chars().count());
        for _ in 0..3 {
            if ms <= target {
                break;
            }
            spoken = shorten_speech_to_duration(&spoken, ms, target).unwrap();
            ms = tauri::async_runtime::block_on(crate::tts::synthesize_or_launch(&voice, &spoken)).unwrap().unwrap().1;
            eprintln!("作り直し: {ms}ms {}文字", spoken.chars().count());
        }
        assert!(ms <= target, "{ms}");
        assert!(spoken.ends_with("、以下略"));
    }

    #[test]
    fn long_speech_is_shortened_by_the_real_voice_speed() {
        // 「、以下略」付きの90文字が30秒かかった(遅い声) → 20秒に収まるよう削り直す
        let spoken = format!("{}、以下略", "う".repeat(86));
        let cut = shorten_speech_to_duration(&spoken, 30000, 19850).unwrap();
        assert!(cut.ends_with("、以下略"));
        let n = cut.chars().filter(|c| *c == 'う').count();
        // 1文字333ms → 19850*0.95/333 = 56文字 - 「、以下略」4文字 = 52
        assert_eq!(n, 52);
        // 文字数ではほぼ収まる計算でも、最低1割は削る(無音の分で収まらなかったため)
        let cut = shorten_speech_to_duration(&"え".repeat(100), 20100, 19850).unwrap();
        assert!(cut.chars().filter(|c| *c == 'え').count() <= 90);
        assert_eq!(shorten_speech_to_duration("", 30000, 19850), None);
    }

    #[test]
    fn long_posts_stay_longer_and_hold_the_queue() {
        let s = CommentSettings::default(); // 3000ms + 80ms/文字、上限12000ms
        assert_eq!(display_duration(&post(1, "おつ"), false, &s), 3160);
        assert_eq!(display_duration(&post(1, &"あ".repeat(50)), false, &s), 7000);
        assert_eq!(display_duration(&post(1, &"あ".repeat(500)), false, &s), 12000);
        assert_eq!(display_duration(&post(1, &"あ".repeat(500)), true, &s), 6000);
        // 普段は表示し終わるまで待つ(短いレスは読み上げ間隔3000ms)
        assert_eq!(next_interval(0, 3160, 0, false, &s), 3160);
        assert_eq!(next_interval(0, 1000, 0, false, &s), 3000);
        // 読み上げが長ければ、読み終わるまで待つ
        assert_eq!(next_interval(0, 7000, 16000, false, &s), 16000);
        // 溜まっている時はターボ間隔
        assert_eq!(next_interval(3, 12000, 16000, false, &s), 400);
        // VOICEVOX等(本当の長さが分かる)は、ターボ中も読み終わるまで待つ(声を途切れさせない)
        assert_eq!(next_interval(3, 12000, 5000, true, &s), 5000);
        assert_eq!(next_interval(3, 12000, 100, true, &s), 400);
        // 読み上げ時間の目安: 100文字 × 160ms
        assert_eq!(speech_duration(&"あ".repeat(100), &s), 16000);
        assert_eq!(speech_duration("", &s), 0);
        let mut s2 = s.clone();
        s2.wait_for_display = false;
        s2.display_per_char_ms = 0;
        assert_eq!(display_duration(&post(1, &"あ".repeat(50)), false, &s2), 3000);
        assert_eq!(next_interval(0, 7000, 0, false, &s2), 3000);
        assert_eq!(next_interval(0, 7000, 5000, false, &s2), 5000);
    }

    #[test]
    fn settings_fill_defaults_for_missing_fields() {
        let s: CommentSettings = serde_json::from_value(serde_json::json!({ "threadUrl": "x" })).unwrap();
        assert_eq!(s.thread_url, "x");
        assert_eq!(s.read_interval_ms, 3000);
        assert_eq!(s.command_args, "#Res#");
    }
}
