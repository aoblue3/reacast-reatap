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

/// ReaCastが鳴らすHTTPの読み上げソフトか(ランダムの声に使えるソフト)
fn is_http_engine(engine: &str) -> bool {
    matches!(engine, "voicevox" | "coeiroinkv1" | "sharevox" | "aivisspeech" | "voicevoxcompat" | "coeiroink")
}

/// このレスを読む声を決めて、その声で読むための設定(読み上げソフト・接続先・声)を返す。
/// 1. 取得元(YouTube・Twitch)ごとの声が指定されていればそれ
/// 2. ランダムがONなら、割合に応じてランダムに(同じ人には同じ声、も選べる)。
///    別の読み上げソフトの声なら、そのソフトの既定の接続先で
/// 3. それ以外は共通の声
/// `person`は同じ人を見分ける文字列(掲示板はID、配信サイトは名前。空なら毎回ランダム)。
pub fn pick_voice(s: &CommentSettings, source: &str, person: &str) -> CommentSettings {
    let mut v = pick_speaker(s, source, person);
    // 声のソフトの調整(話速・音量など)を使う
    if let Some(p) = s.tts_engine_params.get(&v.speech_engine) {
        v.tts_speed = p.speed;
        v.tts_pitch = p.pitch;
        v.tts_intonation = p.intonation;
        v.tts_volume = p.volume;
        v.tts_pre_silence = p.pre_silence;
        v.tts_post_silence = p.post_silence;
        v.tts_pause_scale = p.pause_scale;
    }
    v
}

fn pick_speaker(s: &CommentSettings, source: &str, person: &str) -> CommentSettings {
    let mut v = s.clone();
    let per = match source {
        "youtube" => s.tts_speaker_youtube.trim(),
        "twitch" => s.tts_speaker_twitch.trim(),
        _ => "",
    };
    if !per.is_empty() {
        v.tts_speaker = per.to_string();
        return v;
    }
    if !s.tts_random {
        return v;
    }
    let voices: Vec<_> = s
        .tts_random_voices
        .iter()
        .filter(|r| r.weight > 0 && !r.speaker.trim().is_empty() && is_http_engine(&r.engine))
        .collect();
    let total: u64 = voices.iter().map(|r| r.weight as u64).sum();
    if total == 0 {
        return v;
    }
    let roll = if s.tts_random_by_person && !person.trim().is_empty() {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        person.trim().hash(&mut h);
        h.finish() % total
    } else {
        use rand::Rng;
        rand::thread_rng().gen_range(0..total)
    };
    let mut acc = 0u64;
    for r in voices {
        acc += r.weight as u64;
        if roll < acc {
            if r.engine != s.speech_engine {
                v.speech_engine = r.engine.clone();
                v.tts_url = String::new();
            }
            v.tts_speaker = r.speaker.trim().to_string();
            break;
        }
    }
    v
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
/// `v`はpick_voiceで声を決めた後の設定。
async fn synthesize(v: &CommentSettings, text: &str) -> Result<Option<(Vec<u8>, u64)>, String> {
    let wav = match engine_kind(v) {
        EngineKind::VoicevoxCompat => synth_voicevox(v, text, &v.tts_speaker).await?,
        EngineKind::CoeiroinkV2 => synth_coeiroink(v, text, &v.tts_speaker).await?,
        _ => return Ok(None),
    };
    let ms = wav_duration_ms(&wav).ok_or("読み上げソフトから届いた音声の形式が分かりません")?;
    Ok(Some((wav, ms)))
}

/// 作っておいた音声を鳴らす(前の読み上げが残っていれば止めてから)。
pub fn play_wav(wav: Vec<u8>, s: &CommentSettings) -> Result<u64, String> {
    play(wav, &s.tts_device)
}

/// 棒読みちゃんに読ませる(終わるのは待たない)。起動していなければ起動してから。
pub fn speak_bouyomi(s: &CommentSettings, text: &str) {
    let (s, text) = (s.clone(), text.to_string());
    tauri::async_runtime::spawn(async move {
        if talk_bouyomi(&s, &text).await.is_err() && ensure_running(&s, Duration::from_secs(30)).await.is_ok() {
            if let Err(e) = talk_bouyomi(&s, &text).await {
                log::warn!("{e}");
            }
        }
    });
}

/// 音声を作る。読み上げソフトに繋がらない時は(自動起動がONなら)起動を待ってからもう一度。
/// `v`はpick_voiceで声を決めた後の設定(同じレスで作り直す時も同じ声にするため)。
pub async fn synthesize_or_launch(v: &CommentSettings, text: &str) -> Result<Option<(Vec<u8>, u64)>, String> {
    match synthesize(v, text).await {
        Err(e) if v.tts_auto_launch && !is_alive(v).await => {
            ensure_running(v, Duration::from_secs(60)).await.map_err(|le| format!("{e}({le})"))?;
            synthesize(v, text).await
        }
        r => r,
    }
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
        ensure_running(s, Duration::from_secs(30)).await?;
        return talk_bouyomi(s, text).await;
    }
    if let Some((wav, _)) = synthesize_or_launch(&pick_voice(s, source, ""), text).await? {
        play_wav(wav, s)?;
    }
    Ok(())
}

// ---------------- 読み上げソフトの自動起動 ----------------

/// そのソフトのexeの名前(起動中のプロセスから場所を探す用)
fn exe_names(engine: &str) -> &'static [&'static str] {
    match engine {
        "voicevox" => &["VOICEVOX.exe"],
        "aivisspeech" => &["AivisSpeech.exe"],
        "sharevox" => &["SHAREVOX.exe"],
        "coeiroink" => &["COEIROINKv2.exe", "COEIROINK.exe"],
        "coeiroinkv1" => &["COEIROINK.exe"],
        "bouyomi" => &["BouyomiChan.exe"],
        _ => &[],
    }
}

/// インストーラーが入れる標準の場所(COEIROINK・棒読みちゃんはzipを好きな所に
/// 展開するソフトなので、起動中のプロセスから探すか、場所を指定してもらう)
fn install_candidates(engine: &str) -> Vec<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    let pf = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
    let (dir, exe) = match engine {
        "voicevox" => ("VOICEVOX", "VOICEVOX.exe"),
        "aivisspeech" => ("AivisSpeech", "AivisSpeech.exe"),
        "sharevox" => ("SHAREVOX", "SHAREVOX.exe"),
        _ => return Vec::new(),
    };
    let mut v = Vec::new();
    if let Some(l) = local {
        v.push(l.join("Programs").join(dir).join(exe));
    }
    if let Some(p) = pf {
        v.push(p.join(dir).join(exe));
    }
    v
}

/// 起動中のプロセスから、その名前のexeの場所を探す。
#[cfg(windows)]
fn running_exe(names: &[&str]) -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if names.is_empty() {
        return None;
    }
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut size).is_ok() {
                        found = Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..size as usize])));
                    }
                    let _ = CloseHandle(h);
                }
                if found.is_some() {
                    break;
                }
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}

#[cfg(not(windows))]
fn running_exe(_names: &[&str]) -> Option<std::path::PathBuf> {
    None
}

/// 読み上げソフトのexeの場所: 設定で指定した場所 → 起動中のプロセス → 標準のインストール先。
pub fn find_exe(s: &CommentSettings) -> Option<std::path::PathBuf> {
    let custom = s.tts_exe_paths.get(&s.speech_engine).map(|p| p.trim().trim_matches('"')).unwrap_or_default();
    if !custom.is_empty() {
        let p = std::path::PathBuf::from(custom);
        return p.is_file().then_some(p);
    }
    running_exe(exe_names(&s.speech_engine)).or_else(|| install_candidates(&s.speech_engine).into_iter().find(|p| p.is_file()))
}

/// 読み上げソフトが受付中か(接続先のポートに繋がるか)。
async fn is_alive(s: &CommentSettings) -> bool {
    let Ok(url) = reqwest::Url::parse(&base_url(s)) else { return false };
    let host = url.host_str().unwrap_or("127.0.0.1").to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    matches!(
        tokio::time::timeout(Duration::from_millis(800), tokio::net::TcpStream::connect((host.as_str(), port))).await,
        Ok(Ok(_))
    )
}

/// 最後に自動で起動した時刻(起動に時間がかかる間に何度も起動しないように)
/// 読み上げソフトごと(ランダムで別々のソフトの声を混ぜた時は、それぞれ起動する)
static LAST_LAUNCH: Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> = Mutex::new(None);

/// 読み上げソフトが起動していなければ起動して、受付を始めるまで最大`wait`待つ。
/// 自動起動がOFF・コマンドで渡す方式の時は何もしない。
pub async fn ensure_running(s: &CommentSettings, wait: Duration) -> Result<(), String> {
    if engine_kind(s) == EngineKind::Command || !s.tts_auto_launch {
        return Ok(());
    }
    if is_alive(s).await {
        return Ok(());
    }
    let recently = LAST_LAUNCH
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&s.speech_engine))
        .map(|t| t.elapsed() < Duration::from_secs(90))
        .unwrap_or(false);
    if !recently {
        let exe = find_exe(s).ok_or("読み上げソフトの場所が分かりません(「読み上げ」タブでソフトの場所を指定してください)")?;
        log::info!("読み上げソフトを起動します: {}", exe.display());
        let mut cmd = std::process::Command::new(&exe);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        let child = cmd.spawn().map_err(|e| format!("読み上げソフトを起動できませんでした({}): {e}", exe.display()))?;
        LAST_LAUNCH.lock().unwrap().get_or_insert_with(Default::default).insert(s.speech_engine.clone(), std::time::Instant::now());
        // 配信の画面の前に出てこないよう、窓が出たら最小化する
        #[cfg(windows)]
        crate::voice::minimize_when_shown(child.id());
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
    }
    let until = std::time::Instant::now() + wait;
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if is_alive(s).await {
            // 受付を始めた直後は準備中のことがあるので少し待つ
            tokio::time::sleep(Duration::from_millis(500)).await;
            return Ok(());
        }
    }
    Err("読み上げソフトの起動を待ちましたが、まだ準備ができていません".into())
}

/// 設定が変わった時・ReaCastの起動時に呼ぶ: 使う設定なら、裏で起動しておく。
pub fn launch_in_background(s: &CommentSettings) {
    if !s.speech_enabled || engine_kind(s) == EngineKind::Command || !s.tts_auto_launch {
        return;
    }
    // 設定を保存するたびに呼ばれるので、確かめている最中なら重ねない
    static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CHECKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    // ランダムの声で別の読み上げソフトも使うなら、そのソフトも起動しておく
    let mut targets = vec![s.clone()];
    if s.tts_random && plays_itself(s) {
        for r in &s.tts_random_voices {
            if r.weight > 0 && is_http_engine(&r.engine) && targets.iter().all(|t| t.speech_engine != r.engine) {
                let mut t = s.clone();
                t.speech_engine = r.engine.clone();
                t.tts_url = String::new();
                targets.push(t);
            }
        }
    }
    tauri::async_runtime::spawn(async move {
        for t in &targets {
            if let Err(e) = ensure_running(t, Duration::from_secs(90)).await {
                log::warn!("{e}");
            }
        }
        CHECKING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

// ---------------- 設定画面から使うコマンド ----------------

/// 読み上げソフトのexeの場所を探す(設定画面の「自動で探す」)。指定欄は見ずに探す。
#[tauri::command(async)]
pub fn tts_find_exe(settings: CommentSettings) -> Option<String> {
    let mut s = settings;
    s.tts_exe_paths.remove(&s.speech_engine);
    find_exe(&s).map(|p| p.display().to_string())
}

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
    fn picks_random_voice_by_weight() {
        use crate::comments::RandomVoice;
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speaker = "3".into();
        s.tts_url = "http://192.168.0.5:50021".into();
        // ランダムOFFなら共通の声
        assert_eq!(pick_voice(&s, "bbs", "").tts_speaker, "3");
        s.tts_random = true;
        s.tts_random_voices = vec![
            RandomVoice { engine: "voicevox".into(), speaker: "1".into(), weight: 80 },
            RandomVoice { engine: "coeiroink".into(), speaker: "uuid:0".into(), weight: 20 },
            RandomVoice { engine: "voicevox".into(), speaker: "9".into(), weight: 0 },
        ];
        let mut zunda = 0;
        for _ in 0..2000 {
            let v = pick_voice(&s, "bbs", "");
            match v.tts_speaker.as_str() {
                "1" => {
                    zunda += 1;
                    assert_eq!(v.tts_url, "http://192.168.0.5:50021");
                }
                "uuid:0" => {
                    assert_eq!(v.speech_engine, "coeiroink");
                    assert_eq!(v.tts_url, ""); // 別のソフトは既定の接続先
                }
                other => panic!("割合0の声が選ばれた: {other}"),
            }
        }
        assert!((1450..1750).contains(&zunda), "{zunda}");
        // 同じ人には同じ声
        s.tts_random_by_person = true;
        let first = pick_voice(&s, "bbs", "ID:abcd1234").tts_speaker;
        assert!((0..50).all(|_| pick_voice(&s, "bbs", "ID:abcd1234").tts_speaker == first));
        // YouTubeの声を指定していればそちらが優先
        s.tts_speaker_youtube = "8".into();
        assert_eq!(pick_voice(&s, "youtube", "x").tts_speaker, "8");
    }

    #[test]
    fn uses_params_of_the_picked_engine() {
        use crate::comments::{RandomVoice, TtsParams};
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speed = 1.1;
        // 調整が無いソフトは共通の値
        assert_eq!(pick_voice(&s, "bbs", "").tts_speed, 1.1);
        s.tts_engine_params.insert("voicevox".into(), TtsParams { speed: 1.3, volume: 0.8, ..Default::default() });
        s.tts_engine_params.insert("coeiroink".into(), TtsParams { speed: 0.9, volume: 1.5, ..Default::default() });
        s.tts_random = true;
        s.tts_random_voices = vec![RandomVoice { engine: "coeiroink".into(), speaker: "u:1".into(), weight: 1 }];
        let v = pick_voice(&s, "bbs", "");
        assert_eq!((v.speech_engine.as_str(), v.tts_speed, v.tts_volume), ("coeiroink", 0.9, 1.5));
        s.tts_random = false;
        let v = pick_voice(&s, "bbs", "");
        assert_eq!((v.speech_engine.as_str(), v.tts_speed, v.tts_volume), ("voicevox", 1.3, 0.8));
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
    #[ignore] // 起動中のVOICEVOXの場所を見つけられるか
    fn finds_voicevox_exe() {
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        let p = find_exe(&s).expect("VOICEVOXが見つからない");
        eprintln!("{}", p.display());
        assert!(p.ends_with("VOICEVOX.exe"));
        assert!(tauri::async_runtime::block_on(is_alive(&s)));
        // 起動済みなら何もしないで戻る
        tauri::async_runtime::block_on(ensure_running(&s, Duration::from_secs(1))).unwrap();
        assert!(LAST_LAUNCH.lock().unwrap().as_ref().map(|m| m.is_empty()).unwrap_or(true));
    }

    #[test]
    #[ignore] // VOICEVOXで作った音量0の音声を、既定の出力先で実際に鳴らしてみる
    fn plays_silent_voicevox_audio() {
        let mut s = CommentSettings::default();
        s.speech_engine = "voicevox".into();
        s.tts_speaker = "3".into();
        s.tts_volume = 0.0;
        let (wav, ms) = tauri::async_runtime::block_on(synthesize(&s, "無音のテスト")).unwrap().unwrap();
        assert_eq!(play_wav(wav, &s).unwrap(), ms);
        assert!(!tts_list_devices().is_empty());
        std::thread::sleep(Duration::from_millis(300));
        stop();
    }
}
