//! 読み上げソフトとの連携(コメント読み上げ)。
//!
//! - VOICEVOX互換(VOICEVOX・COEIROINK v1・SHAREVOX・AivisSpeechなど)と
//!   COEIROINK v2: ReaCastからHTTPで音声を作ってもらい、ReaCast自身が鳴らす。
//!   鳴らし終わる時間が正確に分かるので、読み上げ時間を見積もらずに済む。
//!   話速・音高・抑揚・音量・開始/終了無音・間の長さを設定できる。
//! - 棒読みちゃん: HTTP連携(既定は50080番ポート)で読ませる(鳴らすのは棒読みちゃん)。
//! - それ以外(民安★TALK・SofTalkなど)は、これまでどおりコマンドライン引渡し
//!   (comments::run_speech_command)。
//!
//! 音を鳴らす処理(rodio)は、出力先を持ったまま使い続ける必要があるため、
//! 専用のスレッドで動かしてチャンネルで頼む。

use crate::comments::CommentSettings;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// ---------------- 音を鳴らす ----------------

enum AudioCmd {
    /// WAVを鳴らす。鳴らし始めたら、その長さ(ミリ秒)を返す
    Play { wav: Vec<u8>, device: String, done: mpsc::Sender<Result<u64, String>> },
    /// 鳴らしている音を止める(スキップ)
    Stop,
    /// 出力先(スピーカー等)の名前の一覧
    ListDevices(mpsc::Sender<Vec<String>>),
}

fn audio() -> &'static Mutex<mpsc::Sender<AudioCmd>> {
    static TX: OnceLock<Mutex<mpsc::Sender<AudioCmd>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<AudioCmd>();
        std::thread::spawn(move || audio_thread(rx));
        Mutex::new(tx)
    })
}

fn audio_thread(rx: mpsc::Receiver<AudioCmd>) {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    // 出力先ごとに開いたものを使い回す(開き直すと一瞬遅れるため)
    let mut stream: Option<(String, rodio::OutputStream, rodio::OutputStreamHandle)> = None;
    let mut sink: Option<rodio::Sink> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AudioCmd::Play { wav, device, done } => {
                let result = (|| -> Result<u64, String> {
                    let ms = wav_duration_ms(&wav).ok_or("音声の形式が分かりません")?;
                    if stream.as_ref().map(|(d, _, _)| d != &device).unwrap_or(true) {
                        sink = None;
                        stream = None;
                        let host = rodio::cpal::default_host();
                        let picked = if device.is_empty() {
                            None
                        } else {
                            host.output_devices()
                                .ok()
                                .and_then(|mut it| it.find(|d| d.name().map(|n| n == device).unwrap_or(false)))
                        };
                        // 指定の出力先が見つからない時は、Windowsの既定の出力先で鳴らす
                        let (s, h) = match picked {
                            Some(d) => rodio::OutputStream::try_from_device(&d),
                            None => rodio::OutputStream::try_default(),
                        }
                        .map_err(|e| format!("音を出せません: {e}"))?;
                        stream = Some((device.clone(), s, h));
                    }
                    let (_, _, handle) = stream.as_ref().unwrap();
                    let new_sink = rodio::Sink::try_new(handle).map_err(|e| format!("音を出せません: {e}"))?;
                    let source = rodio::Decoder::new(std::io::Cursor::new(wav)).map_err(|e| format!("音声を読めません: {e}"))?;
                    new_sink.append(source);
                    if let Some(old) = sink.replace(new_sink) {
                        old.stop();
                    }
                    Ok(ms)
                })();
                let _ = done.send(result);
            }
            AudioCmd::Stop => {
                if let Some(s) = sink.take() {
                    s.stop();
                }
            }
            AudioCmd::ListDevices(reply) => {
                let names = rodio::cpal::default_host()
                    .output_devices()
                    .map(|it| it.filter_map(|d| d.name().ok()).collect())
                    .unwrap_or_default();
                let _ = reply.send(names);
            }
        }
    }
}

/// WAVの長さ(ミリ秒)をヘッダーから計算する。
fn wav_duration_ms(wav: &[u8]) -> Option<u64> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut byte_rate = 0u32;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 12 <= wav.len() {
            byte_rate = u32::from_le_bytes(wav[body + 8..body + 12].try_into().ok()?);
        } else if id == b"data" && byte_rate > 0 {
            // 長さが書かれていない(0xFFFFFFFF等)時は、ファイルの残り全部をデータとみなす
            let data_len = size.min(wav.len().saturating_sub(body));
            return Some(data_len as u64 * 1000 / byte_rate as u64);
        }
        pos = body + size + (size & 1);
    }
    None
}

/// 鳴らし始めて、長さ(ミリ秒)を返す。
fn play(wav: Vec<u8>, device: &str) -> Result<u64, String> {
    let (done, wait) = mpsc::channel();
    audio()
        .lock()
        .unwrap()
        .send(AudioCmd::Play { wav, device: device.to_string(), done })
        .map_err(|_| "音を鳴らす準備ができていません".to_string())?;
    wait.recv_timeout(Duration::from_secs(10)).map_err(|_| "音を鳴らせませんでした".to_string())?
}

/// 鳴らしている読み上げを止める(スキップ・停止)。
pub fn stop() {
    let _ = audio().lock().unwrap().send(AudioCmd::Stop);
}

// ---------------- 読み上げソフト ----------------

/// どの読み上げソフトを使うか(CommentSettings::speech_engine)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EngineKind {
    /// コマンドライン引渡し(民安★TALK・SofTalkなど)
    Command,
    /// VOICEVOX互換のHTTP API
    VoicevoxCompat,
    /// COEIROINK v2のHTTP API
    CoeiroinkV2,
    /// 棒読みちゃんのHTTP連携
    Bouyomi,
}

pub fn engine_kind(s: &CommentSettings) -> EngineKind {
    match s.speech_engine.as_str() {
        "voicevox" | "coeiroinkv1" | "sharevox" | "aivisspeech" | "voicevoxcompat" => EngineKind::VoicevoxCompat,
        "coeiroink" => EngineKind::CoeiroinkV2,
        "bouyomi" => EngineKind::Bouyomi,
        _ => EngineKind::Command,
    }
}

/// 読み上げソフトの接続先(空欄なら、そのソフトの既定のアドレス)。
pub fn base_url(s: &CommentSettings) -> String {
    let custom = s.tts_url.trim().trim_end_matches('/');
    if !custom.is_empty() {
        return custom.to_string();
    }
    let port = match s.speech_engine.as_str() {
        "coeiroinkv1" => 50031,
        "coeiroink" => 50032,
        "sharevox" => 50025,
        "aivisspeech" => 10101,
        "bouyomi" => 50080,
        _ => 50021,
    };
    format!("http://127.0.0.1:{port}")
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

/// 取得元(掲示板・YouTube・Twitch)ごとの声。空欄なら共通の声。
fn speaker_for(s: &CommentSettings, source: &str) -> String {
    let per = match source {
        "youtube" => s.tts_speaker_youtube.trim(),
        "twitch" => s.tts_speaker_twitch.trim(),
        _ => "",
    };
    if per.is_empty() { s.tts_speaker.trim() } else { per }.to_string()
}

async fn synth_voicevox(s: &CommentSettings, text: &str, speaker: &str) -> Result<Vec<u8>, String> {
    let base = base_url(s);
    let speaker: u32 = speaker.parse().map_err(|_| "声が選ばれていません(「声の一覧を取得」から選んでください)".to_string())?;
    let query: serde_json::Value = client()
        .post(format!("{base}/audio_query"))
        .query(&[("text", text), ("speaker", &speaker.to_string())])
        .send()
        .await
        .map_err(|e| format!("読み上げソフト({base})に繋がりません: {e}"))?
        .error_for_status()
        .map_err(|e| format!("読み上げの準備に失敗しました: {e}"))?
        .json()
        .await
        .map_err(|e| format!("読み上げの準備に失敗しました: {e}"))?;
    let mut query = query;
    query["speedScale"] = serde_json::json!(s.tts_speed);
    query["pitchScale"] = serde_json::json!(s.tts_pitch);
    query["intonationScale"] = serde_json::json!(s.tts_intonation);
    query["volumeScale"] = serde_json::json!(s.tts_volume);
    query["prePhonemeLength"] = serde_json::json!(s.tts_pre_silence);
    query["postPhonemeLength"] = serde_json::json!(s.tts_post_silence);
    // 句読点などの「間」の長さ(対応しているソフトだけ。無い項目は送らない)
    if query.get("pauseLengthScale").is_some() {
        query["pauseLengthScale"] = serde_json::json!(s.tts_pause_scale);
    }
    let wav = client()
        .post(format!("{base}/synthesis"))
        .query(&[("speaker", speaker.to_string())])
        .json(&query)
        .send()
        .await
        .map_err(|e| format!("読み上げソフト({base})に繋がりません: {e}"))?
        .error_for_status()
        .map_err(|e| format!("音声を作れませんでした: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("音声を作れませんでした: {e}"))?;
    Ok(wav.to_vec())
}

/// COEIROINK v2: 声は「話者のUUID:スタイルの番号」の形で持つ。
async fn synth_coeiroink(s: &CommentSettings, text: &str, speaker: &str) -> Result<Vec<u8>, String> {
    let base = base_url(s);
    let (uuid, style) = speaker
        .rsplit_once(':')
        .and_then(|(u, st)| st.parse::<i64>().ok().map(|st| (u.to_string(), st)))
        .ok_or("声が選ばれていません(「声の一覧を取得」から選んでください)")?;
    // 読み方・アクセント(prosody)を先に推定してもらい、それを付けて音声を作る
    let prosody: serde_json::Value = client()
        .post(format!("{base}/v1/estimate_prosody"))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await
        .map_err(|e| format!("COEIROINK({base})に繋がりません: {e}"))?
        .json()
        .await
        .unwrap_or_default();
    let body = serde_json::json!({
        "speakerUuid": uuid,
        "styleId": style,
        "text": text,
        "prosodyDetail": prosody.get("detail").cloned().unwrap_or(serde_json::json!([])),
        "speedScale": s.tts_speed,
        "volumeScale": s.tts_volume,
        "pitchScale": s.tts_pitch,
        "intonationScale": s.tts_intonation,
        "prePhonemeLength": s.tts_pre_silence,
        "postPhonemeLength": s.tts_post_silence,
        "outputSamplingRate": 24000,
    });
    let wav = client()
        .post(format!("{base}/v1/synthesis"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("COEIROINK({base})に繋がりません: {e}"))?
        .error_for_status()
        .map_err(|e| format!("音声を作れませんでした: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("音声を作れませんでした: {e}"))?;
    Ok(wav.to_vec())
}

async fn talk_bouyomi(s: &CommentSettings, text: &str) -> Result<(), String> {
    let base = base_url(s);
    client()
        .get(format!("{base}/Talk"))
        .query(&[
            ("text", text.to_string()),
            ("voice", s.bouyomi_voice.to_string()),
            ("speed", s.bouyomi_speed.to_string()),
            ("tone", s.bouyomi_tone.to_string()),
            ("volume", s.bouyomi_volume.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("棒読みちゃん({base})に繋がりません(棒読みちゃんの「HTTP連携」を有効にしてください): {e}"))?;
    Ok(())
}

/// ReaCastが鳴らす読み上げソフト(VOICEVOX互換・COEIROINK v2)か。
pub fn plays_itself(s: &CommentSettings) -> bool {
    matches!(engine_kind(s), EngineKind::VoicevoxCompat | EngineKind::CoeiroinkV2)
}

/// 読み上げの音声(WAV)を作ってもらう。ReaCastが鳴らすソフトでなければNone。
/// 字幕を出す前に作っておき、長さ(wav_duration_ms)に合わせて字幕を出しておく。
pub async fn synthesize(s: &CommentSettings, text: &str, source: &str) -> Result<Option<(Vec<u8>, u64)>, String> {
    let wav = match engine_kind(s) {
        EngineKind::VoicevoxCompat => synth_voicevox(s, text, &speaker_for(s, source)).await?,
        EngineKind::CoeiroinkV2 => synth_coeiroink(s, text, &speaker_for(s, source)).await?,
        _ => return Ok(None),
    };
    let ms = wav_duration_ms(&wav).ok_or("読み上げソフトから届いた音声の形式が分かりません")?;
    Ok(Some((wav, ms)))
}

/// 作っておいた音声を鳴らす(前の読み上げが残っていれば止めてから)。
pub fn play_wav(wav: Vec<u8>, s: &CommentSettings) -> Result<u64, String> {
    play(wav, &s.tts_device)
}

/// 棒読みちゃんに読ませる(終わるのは待たない)。
pub fn speak_bouyomi(s: &CommentSettings, text: &str) {
    let (s, text) = (s.clone(), text.to_string());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = talk_bouyomi(&s, &text).await {
            log::warn!("{e}");
        }
    });
}

/// スキップ: ReaCastが鳴らしている声を止める。棒読みちゃんには「スキップ」を頼む。
pub fn skip(s: &CommentSettings) {
    stop();
    if engine_kind(s) == EngineKind::Bouyomi {
        let base = base_url(s);
        tauri::async_runtime::spawn(async move {
            let _ = client().get(format!("{base}/Skip")).send().await;
        });
    }
}

/// 設定画面の「試しに読み上げ」用: 作ってすぐ鳴らす。
async fn speak(s: &CommentSettings, text: &str, source: &str) -> Result<(), String> {
    if engine_kind(s) == EngineKind::Bouyomi {
        return talk_bouyomi(s, text).await;
    }
    if let Some((wav, _)) = synthesize(s, text, source).await? {
        play_wav(wav, s)?;
    }
    Ok(())
}

// ---------------- 設定画面から使うコマンド ----------------

/// 声の一覧([{id, name}])。idは設定のtts_speakerにそのまま入れる値。
#[tauri::command]
pub async fn tts_list_speakers(settings: CommentSettings) -> Result<Vec<serde_json::Value>, String> {
    let base = base_url(&settings);
    match engine_kind(&settings) {
        EngineKind::VoicevoxCompat => {
            let list: Vec<serde_json::Value> = client()
                .get(format!("{base}/speakers"))
                .send()
                .await
                .map_err(|e| format!("読み上げソフト({base})に繋がりません。ソフトを起動しているか確認してください: {e}"))?
                .json()
                .await
                .map_err(|e| format!("声の一覧を読めませんでした: {e}"))?;
            Ok(list
                .iter()
                .flat_map(|sp| {
                    let name = sp["name"].as_str().unwrap_or_default().to_string();
                    sp["styles"].as_array().cloned().unwrap_or_default().into_iter().map(move |st| {
                        serde_json::json!({
                            "id": st["id"].as_i64().unwrap_or_default().to_string(),
                            "name": format!("{name}({})", st["name"].as_str().unwrap_or_default()),
                        })
                    })
                })
                .collect())
        }
        EngineKind::CoeiroinkV2 => {
            let list: Vec<serde_json::Value> = client()
                .get(format!("{base}/v1/speakers"))
                .send()
                .await
                .map_err(|e| format!("COEIROINK({base})に繋がりません。COEIROINKを起動しているか確認してください: {e}"))?
                .json()
                .await
                .map_err(|e| format!("声の一覧を読めませんでした: {e}"))?;
            Ok(list
                .iter()
                .flat_map(|sp| {
                    let name = sp["speakerName"].as_str().unwrap_or_default().to_string();
                    let uuid = sp["speakerUuid"].as_str().unwrap_or_default().to_string();
                    sp["styles"].as_array().cloned().unwrap_or_default().into_iter().map(move |st| {
                        serde_json::json!({
                            "id": format!("{uuid}:{}", st["styleId"].as_i64().unwrap_or_default()),
                            "name": format!("{name}({})", st["styleName"].as_str().unwrap_or_default()),
                        })
                    })
                })
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

/// 音の出力先(スピーカー・仮想オーディオデバイス等)の名前の一覧。
#[tauri::command(async)]
pub fn tts_list_devices() -> Vec<String> {
    let (tx, rx) = mpsc::channel();
    if audio().lock().unwrap().send(AudioCmd::ListDevices(tx)).is_err() {
        return Vec::new();
    }
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
}

/// 設定画面の「試しに読み上げ」(入力途中の設定で読み上げる)。
#[tauri::command]
pub async fn tts_test(settings: CommentSettings, text: String) -> Result<(), String> {
    if engine_kind(&settings) == EngineKind::Command {
        return Err("コマンドで渡す読み上げソフトは、「取得」タブのテスト表示で試してください".into());
    }
    speak(&settings, &text, "bbs").await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(sample_rate: u32, channels: u16, bits: u16, data_len: u32) -> Vec<u8> {
        let byte_rate = sample_rate * channels as u32 * bits as u32 / 8;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&byte_rate.to_le_bytes());
        v.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.resize(v.len() + data_len as usize, 0);
        v
    }

    #[test]
    fn computes_wav_length() {
        // 24000Hz・モノラル・16bitで48000バイト = 1秒
        assert_eq!(wav_duration_ms(&wav(24000, 1, 16, 48000)), Some(1000));
        assert_eq!(wav_duration_ms(&wav(48000, 2, 16, 96000)), Some(500));
        assert_eq!(wav_duration_ms(b"not a wav"), None);
    }

    #[test]
    fn picks_engine_and_default_address() {
        let mut s = CommentSettings::default();
        assert_eq!(engine_kind(&s), EngineKind::Command);
        s.speech_engine = "voicevox".into();
        assert_eq!(engine_kind(&s), EngineKind::VoicevoxCompat);
        assert_eq!(base_url(&s), "http://127.0.0.1:50021");
        s.speech_engine = "coeiroink".into();
        assert_eq!(engine_kind(&s), EngineKind::CoeiroinkV2);
        assert_eq!(base_url(&s), "http://127.0.0.1:50032");
        s.speech_engine = "aivisspeech".into();
        assert_eq!(base_url(&s), "http://127.0.0.1:10101");
        s.tts_url = "http://192.168.0.5:50021/".into();
        assert_eq!(base_url(&s), "http://192.168.0.5:50021");
    }

    #[test]
    #[ignore] // 実際にVOICEVOXへ繋ぐ(VOICEVOXを起動してから cargo test -- --ignored)
    fn synthesizes_with_voicevox() {
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speaker = "3".into();
        s.tts_speed = 1.2;
        let wav = tauri::async_runtime::block_on(synth_voicevox(&s, "テストです", "3")).unwrap();
        let ms = wav_duration_ms(&wav).unwrap();
        eprintln!("{} bytes, {ms} ms", wav.len());
        assert!(ms > 300 && ms < 5000);
    }

    #[test]
    #[ignore] // VOICEVOXで作った音量0の音声を、既定の出力先で実際に鳴らしてみる
    fn plays_silent_voicevox_audio() {
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speaker = "3".into();
        s.tts_volume = 0.0;
        let (wav, ms) = tauri::async_runtime::block_on(synthesize(&s, "無音のテスト", "bbs")).unwrap().unwrap();
        assert_eq!(play_wav(wav, &s).unwrap(), ms);
        assert!(!tts_list_devices().is_empty());
        std::thread::sleep(Duration::from_millis(300));
        stop();
    }
}
