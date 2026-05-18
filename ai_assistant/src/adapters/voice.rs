use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::config::{AssistantPaths, VoiceConfig, resolve_config_path};

#[derive(Clone, Debug)]
pub struct CapturedAudio {
    pub path: PathBuf,
    pub sample_rate: usize,
    pub duration_seconds_max: usize,
}

#[derive(Clone, Debug)]
pub struct AudioCaptureAdapter {
    config: VoiceConfig,
}

impl AudioCaptureAdapter {
    pub fn new(config: VoiceConfig) -> Self {
        Self { config }
    }

    pub fn capture(&self, path: &Path) -> Result<CapturedAudio, String> {
        let mut command = Command::new(&self.config.recorder_binary_path);
        command.args(build_recorder_args(&self.config, path));
        let output = command
            .output()
            .map_err(|error| format!("failed to invoke recorder: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "recorder failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(CapturedAudio {
            path: path.to_path_buf(),
            sample_rate: self.config.sample_rate,
            duration_seconds_max: self.config.capture_seconds_max,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SpeechToTextAdapter {
    paths: AssistantPaths,
    config: VoiceConfig,
}

impl SpeechToTextAdapter {
    pub fn new(paths: AssistantPaths, config: VoiceConfig) -> Self {
        Self { paths, config }
    }

    pub fn transcribe(&self, audio_path: &Path) -> Result<String, String> {
        let mut command = Command::new(&self.config.stt_binary_path);
        command.args(build_stt_args(&self.paths, &self.config, audio_path));
        let output = command
            .output()
            .map_err(|error| format!("failed to invoke STT: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "STT failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(normalize_transcript(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }
}

#[derive(Clone, Debug)]
pub struct TextToSpeechAdapter {
    paths: AssistantPaths,
    config: VoiceConfig,
}

impl TextToSpeechAdapter {
    pub fn new(paths: AssistantPaths, config: VoiceConfig) -> Self {
        Self { paths, config }
    }

    pub fn synthesize(&self, text: &str, path: &Path) -> Result<PathBuf, String> {
        let mut child = Command::new(&self.config.tts_binary_path)
            .args(build_tts_args(&self.paths, &self.config, path))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to invoke TTS: {error}"))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| "failed to open TTS stdin".to_string())?;
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| format!("failed to send text to TTS: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to wait for TTS: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "TTS failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(path.to_path_buf())
    }
}

#[derive(Clone, Debug)]
pub struct AudioPlaybackAdapter {
    config: VoiceConfig,
}

impl AudioPlaybackAdapter {
    pub fn new(config: VoiceConfig) -> Self {
        Self { config }
    }

    pub fn play(&self, path: &Path) -> Result<(), String> {
        let mut command = Command::new(&self.config.player_binary_path);
        command.args(build_player_args(&self.config, path));
        let output = command
            .output()
            .map_err(|error| format!("failed to invoke player: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "playback failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

pub fn build_recorder_args(config: &VoiceConfig, output_path: &Path) -> Vec<String> {
    let binary = binary_name(&config.recorder_binary_path);
    if binary.contains("ffmpeg") {
        let mut args = vec!["-y".into()];
        if !config.input_device.trim().is_empty() {
            args.extend([
                "-f".into(),
                "alsa".into(),
                "-i".into(),
                config.input_device.clone(),
            ]);
        } else {
            args.extend(["-f".into(), "alsa".into(), "-i".into(), "default".into()]);
        }
        args.extend([
            "-ar".into(),
            config.sample_rate.to_string(),
            "-ac".into(),
            "1".into(),
            "-t".into(),
            config.capture_seconds_max.to_string(),
            output_path.display().to_string(),
        ]);
        return args;
    }

    let mut args = vec![
        "-q".into(),
        "-f".into(),
        "S16_LE".into(),
        "-r".into(),
        config.sample_rate.to_string(),
        "-c".into(),
        "1".into(),
        "-d".into(),
        config.capture_seconds_max.to_string(),
    ];
    if !config.input_device.trim().is_empty() {
        args.extend(["-D".into(), config.input_device.clone()]);
    }
    args.push(output_path.display().to_string());
    args
}

pub fn build_stt_args(
    paths: &AssistantPaths,
    config: &VoiceConfig,
    audio_path: &Path,
) -> Vec<String> {
    vec![
        "-m".into(),
        resolve_config_path(paths, &config.stt_model_path)
            .display()
            .to_string(),
        "-f".into(),
        audio_path.display().to_string(),
    ]
}

pub fn build_tts_args(
    paths: &AssistantPaths,
    config: &VoiceConfig,
    output_path: &Path,
) -> Vec<String> {
    vec![
        "--model".into(),
        resolve_config_path(paths, &config.tts_model_path)
            .display()
            .to_string(),
        "--output_file".into(),
        output_path.display().to_string(),
    ]
}

pub fn build_player_args(config: &VoiceConfig, audio_path: &Path) -> Vec<String> {
    let binary = binary_name(&config.player_binary_path);
    if binary.contains("ffplay") {
        return vec![
            "-nodisp".into(),
            "-autoexit".into(),
            audio_path.display().to_string(),
        ];
    }
    let mut args = Vec::new();
    if !config.output_device.trim().is_empty() {
        args.extend(["-D".into(), config.output_device.clone()]);
    }
    args.push(audio_path.display().to_string());
    args
}

pub fn normalize_transcript(raw: &str) -> String {
    raw.lines()
        .map(strip_timestamp_prefix)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("whisper_") && !line.starts_with("system_info:"))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_timestamp_prefix(line: &str) -> &str {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return trimmed;
    }
    let Some(close) = trimmed.find(']') else {
        return trimmed;
    };
    let prefix = &trimmed[..=close];
    if prefix.contains("-->") {
        trimmed[close + 1..].trim()
    } else {
        trimmed
    }
}

fn binary_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{config::AssistantPaths, util::unique_temp_dir};

    use super::{
        build_player_args, build_recorder_args, build_stt_args, build_tts_args,
        normalize_transcript,
    };

    #[test]
    fn normalizes_whisper_timestamp_output() {
        let text = normalize_transcript(
            "whisper_init\n[00:00:00.000 --> 00:00:01.000] hello there\n[00:00:01.000 --> 00:00:02.000] local assistant\n",
        );
        assert_eq!(text, "hello there local assistant");
    }

    #[test]
    fn builds_arecord_and_aplay_args_with_devices() {
        let root = unique_temp_dir("voice-adapter-args");
        let paths = AssistantPaths::new(root);
        let mut config = crate::core::voice::tests::voice_config_for_tests(&paths);
        config.input_device = "plughw:1,0".into();
        config.output_device = "plughw:0,0".into();

        let record_args = build_recorder_args(&config, &PathBuf::from("/tmp/input.wav"));
        assert!(record_args.contains(&"-D".to_string()));
        assert!(record_args.contains(&"plughw:1,0".to_string()));
        assert!(record_args.contains(&"16000".to_string()));

        let play_args = build_player_args(&config, &PathBuf::from("/tmp/output.wav"));
        assert_eq!(play_args, vec!["-D", "plughw:0,0", "/tmp/output.wav"]);
    }

    #[test]
    fn builds_stt_and_tts_args_from_config_paths() {
        let root = unique_temp_dir("voice-adapter-models");
        let paths = AssistantPaths::new(root);
        let config = crate::core::voice::tests::voice_config_for_tests(&paths);

        let stt_args = build_stt_args(&paths, &config, &PathBuf::from("/tmp/input.wav"));
        assert_eq!(stt_args[0], "-m");
        assert!(stt_args[1].ends_with("whisper.bin"));
        assert_eq!(stt_args[2], "-f");

        let tts_args = build_tts_args(&paths, &config, &PathBuf::from("/tmp/output.wav"));
        assert_eq!(tts_args[0], "--model");
        assert!(tts_args[1].ends_with("piper.onnx"));
        assert_eq!(tts_args[2], "--output_file");
    }
}
