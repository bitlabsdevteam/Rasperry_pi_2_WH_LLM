use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    adapters::{
        storage::SqliteStore,
        voice::{
            AudioCaptureAdapter, AudioPlaybackAdapter, CapturedAudio, SpeechToTextAdapter,
            TextToSpeechAdapter,
        },
    },
    config::{AppConfig, AssistantPaths, VoiceConfig, resolve_config_path},
    core::service::run_chat_session,
    util::ensure_dir,
};

pub const DEFAULT_VOICE_SESSION: &str = "voice:local:default";

#[derive(Clone, Debug)]
pub struct VoiceTurnOutput {
    pub session: String,
    pub transcript: Option<String>,
    pub response: Option<String>,
    pub input_audio_path: Option<PathBuf>,
    pub output_audio_path: Option<PathBuf>,
    pub skipped: bool,
    pub errors: Vec<String>,
}

pub trait VoiceCapture {
    fn capture(&self, path: &Path) -> Result<CapturedAudio, String>;
}

pub trait VoiceTranscriber {
    fn transcribe(&self, audio_path: &Path) -> Result<String, String>;
}

pub trait VoiceSynthesizer {
    fn synthesize(&self, text: &str, path: &Path) -> Result<PathBuf, String>;
}

pub trait VoicePlayer {
    fn play(&self, path: &Path) -> Result<(), String>;
}

impl VoiceCapture for AudioCaptureAdapter {
    fn capture(&self, path: &Path) -> Result<CapturedAudio, String> {
        AudioCaptureAdapter::capture(self, path)
    }
}

impl VoiceTranscriber for SpeechToTextAdapter {
    fn transcribe(&self, audio_path: &Path) -> Result<String, String> {
        SpeechToTextAdapter::transcribe(self, audio_path)
    }
}

impl VoiceSynthesizer for TextToSpeechAdapter {
    fn synthesize(&self, text: &str, path: &Path) -> Result<PathBuf, String> {
        TextToSpeechAdapter::synthesize(self, text, path)
    }
}

impl VoicePlayer for AudioPlaybackAdapter {
    fn play(&self, path: &Path) -> Result<(), String> {
        AudioPlaybackAdapter::play(self, path)
    }
}

pub fn run_voice_turn(
    paths: &AssistantPaths,
    config: &AppConfig,
    store: &SqliteStore,
    session: &str,
) -> Result<VoiceTurnOutput, String> {
    let capture = AudioCaptureAdapter::new(config.voice.clone());
    let transcriber = SpeechToTextAdapter::new(paths.clone(), config.voice.clone());
    let synthesizer = TextToSpeechAdapter::new(paths.clone(), config.voice.clone());
    let player = AudioPlaybackAdapter::new(config.voice.clone());
    run_voice_turn_with_adapters(
        paths,
        &config.voice,
        session,
        &capture,
        &transcriber,
        &synthesizer,
        &player,
        |message| {
            run_chat_session(paths, config, store, session, message, config.llm.stream)
                .map(|output| output.response)
        },
    )
}

pub fn run_voice_turn_with_adapters<C, S, T, P, F>(
    paths: &AssistantPaths,
    config: &VoiceConfig,
    session: &str,
    capture: &C,
    transcriber: &S,
    synthesizer: &T,
    player: &P,
    mut respond: F,
) -> Result<VoiceTurnOutput, String>
where
    C: VoiceCapture,
    S: VoiceTranscriber,
    T: VoiceSynthesizer,
    P: VoicePlayer,
    F: FnMut(&str) -> Result<String, String>,
{
    let temp_dir = resolve_config_path(paths, &config.temp_audio_dir);
    ensure_dir(&temp_dir)?;
    let input_audio_path = temp_dir.join(format!("voice-{}-input.wav", turn_stamp()));
    let output_audio_path = temp_dir.join(format!("voice-{}-reply.wav", turn_stamp()));

    let captured = capture.capture(&input_audio_path)?;
    let transcript = transcriber.transcribe(&captured.path)?;
    if transcript_is_empty(&transcript) {
        return Ok(VoiceTurnOutput {
            session: session.to_string(),
            transcript: None,
            response: None,
            input_audio_path: Some(captured.path),
            output_audio_path: None,
            skipped: true,
            errors: Vec::new(),
        });
    }

    let mut errors = Vec::new();
    let response = match respond(&transcript) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("LLM failed: {error}"));
            "I could not reach the local language model. Please try again.".to_string()
        }
    };

    let mut generated_audio = None;
    match synthesizer.synthesize(&response, &output_audio_path) {
        Ok(path) => {
            generated_audio = Some(path.clone());
            if let Err(error) = player.play(&path) {
                errors.push(error);
            }
        }
        Err(error) => {
            errors.push(error);
        }
    }

    Ok(VoiceTurnOutput {
        session: session.to_string(),
        transcript: Some(transcript),
        response: Some(response),
        input_audio_path: Some(captured.path),
        output_audio_path: generated_audio,
        skipped: false,
        errors,
    })
}

pub fn doctor_voice(paths: &AssistantPaths, config: &VoiceConfig) -> Vec<(String, bool)> {
    let temp_dir = resolve_config_path(paths, &config.temp_audio_dir);
    vec![
        (
            "voice recorder".to_string(),
            command_or_executable_exists(&config.recorder_binary_path),
        ),
        (
            "voice player".to_string(),
            command_or_executable_exists(&config.player_binary_path),
        ),
        (
            "voice STT binary".to_string(),
            command_or_executable_exists(&config.stt_binary_path),
        ),
        (
            "voice STT model".to_string(),
            resolve_config_path(paths, &config.stt_model_path).exists(),
        ),
        (
            "voice TTS binary".to_string(),
            command_or_executable_exists(&config.tts_binary_path),
        ),
        (
            "voice TTS model".to_string(),
            resolve_config_path(paths, &config.tts_model_path).exists(),
        ),
        ("voice temp audio dir".to_string(), writable_dir(&temp_dir)),
    ]
}

pub fn transcript_is_empty(transcript: &str) -> bool {
    let normalized = transcript
        .trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .to_ascii_lowercase();
    normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "you" | "um" | "uh" | "hmm" | "thank you" | "thanks for watching"
        )
}

fn writable_dir(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".voice-write-check");
    let result = fs::write(&probe, "ok").is_ok();
    let _ = fs::remove_file(probe);
    result
}

fn command_or_executable_exists(value: &str) -> bool {
    if value.contains('/') {
        return fs::metadata(value)
            .map(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    metadata.is_file()
                }
            })
            .unwrap_or(false);
    }

    std::process::Command::new("which")
        .arg(value)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn turn_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
pub mod tests {
    use std::{
        cell::Cell,
        path::{Path, PathBuf},
    };

    use crate::{
        adapters::voice::CapturedAudio,
        config::{
            AssistantPaths, VoiceConfig, default_voice_stt_model_path,
            default_voice_temp_audio_dir, default_voice_tts_model_path,
        },
        util::unique_temp_dir,
    };

    use super::{
        VoiceCapture, VoicePlayer, VoiceSynthesizer, VoiceTranscriber,
        run_voice_turn_with_adapters, transcript_is_empty,
    };

    pub fn voice_config_for_tests(paths: &AssistantPaths) -> VoiceConfig {
        VoiceConfig {
            enabled: false,
            input_device: String::new(),
            output_device: String::new(),
            sample_rate: 16000,
            capture_seconds_max: 8,
            stt_binary_path: "whisper-cli".into(),
            stt_model_path: default_voice_stt_model_path(paths),
            tts_binary_path: "piper".into(),
            tts_model_path: default_voice_tts_model_path(paths),
            player_binary_path: "aplay".into(),
            recorder_binary_path: "arecord".into(),
            trigger_mode: "push_to_talk".into(),
            push_to_talk_command: String::new(),
            silence_timeout_ms: 1200,
            temp_audio_dir: default_voice_temp_audio_dir(paths),
        }
    }

    struct MockCapture;

    impl VoiceCapture for MockCapture {
        fn capture(&self, path: &Path) -> Result<CapturedAudio, String> {
            Ok(CapturedAudio {
                path: path.to_path_buf(),
                sample_rate: 16000,
                duration_seconds_max: 8,
            })
        }
    }

    struct MockTranscriber {
        transcript: String,
    }

    impl VoiceTranscriber for MockTranscriber {
        fn transcribe(&self, _audio_path: &Path) -> Result<String, String> {
            Ok(self.transcript.clone())
        }
    }

    struct MockSynthesizer {
        fail: bool,
    }

    impl VoiceSynthesizer for MockSynthesizer {
        fn synthesize(&self, _text: &str, path: &Path) -> Result<PathBuf, String> {
            if self.fail {
                Err("mock TTS unavailable".into())
            } else {
                Ok(path.to_path_buf())
            }
        }
    }

    struct MockPlayer {
        fail: bool,
    }

    impl VoicePlayer for MockPlayer {
        fn play(&self, _path: &Path) -> Result<(), String> {
            if self.fail {
                Err("mock playback failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn recognizes_empty_transcripts() {
        assert!(transcript_is_empty(""));
        assert!(transcript_is_empty("..."));
        assert!(transcript_is_empty("thanks for watching"));
        assert!(!transcript_is_empty("turn on the desk light"));
    }

    #[test]
    fn skips_llm_when_transcript_is_empty() {
        let root = unique_temp_dir("voice-empty-turn");
        let paths = AssistantPaths::new(root);
        let config = voice_config_for_tests(&paths);
        let called = Cell::new(false);

        let output = run_voice_turn_with_adapters(
            &paths,
            &config,
            "voice:local:default",
            &MockCapture,
            &MockTranscriber {
                transcript: " ... ".into(),
            },
            &MockSynthesizer { fail: false },
            &MockPlayer { fail: false },
            |_message| {
                called.set(true);
                Ok("should not run".into())
            },
        )
        .unwrap();

        assert!(output.skipped);
        assert!(!called.get());
        assert!(output.response.is_none());
    }

    #[test]
    fn preserves_response_when_playback_fails() {
        let root = unique_temp_dir("voice-playback-failure");
        let paths = AssistantPaths::new(root);
        let config = voice_config_for_tests(&paths);

        let output = run_voice_turn_with_adapters(
            &paths,
            &config,
            "voice:local:default",
            &MockCapture,
            &MockTranscriber {
                transcript: "hello assistant".into(),
            },
            &MockSynthesizer { fail: false },
            &MockPlayer { fail: true },
            |_message| Ok("hello back".into()),
        )
        .unwrap();

        assert_eq!(output.response.as_deref(), Some("hello back"));
        assert!(output.output_audio_path.is_some());
        assert!(output.errors.iter().any(|error| error.contains("playback")));
    }

    #[test]
    fn treats_tts_failure_as_successful_text_turn() {
        let root = unique_temp_dir("voice-tts-failure");
        let paths = AssistantPaths::new(root);
        let config = voice_config_for_tests(&paths);

        let output = run_voice_turn_with_adapters(
            &paths,
            &config,
            "voice:local:default",
            &MockCapture,
            &MockTranscriber {
                transcript: "hello assistant".into(),
            },
            &MockSynthesizer { fail: true },
            &MockPlayer { fail: false },
            |_message| Ok("hello back".into()),
        )
        .unwrap();

        assert_eq!(output.transcript.as_deref(), Some("hello assistant"));
        assert_eq!(output.response.as_deref(), Some("hello back"));
        assert!(output.output_audio_path.is_none());
        assert!(output.errors.iter().any(|error| error.contains("TTS")));
    }
}
