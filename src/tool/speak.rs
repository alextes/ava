use std::path::PathBuf;

use serde::Deserialize;
use serde_json::json;

use super::{ToolCall, ToolCallResult, ToolDefinition};
use crate::config;
use crate::message::MessageContent;

pub const SPEAK_TOOL_NAME: &str = "speak";

const MAX_SPEAK_CHARS: usize = 1000;

pub(super) fn speak_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: SPEAK_TOOL_NAME,
        description: "speak text aloud using text-to-speech. on telegram, sends a voice message. locally, plays through system audio. use this when the user asks you to speak or when voice output is appropriate.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "the text to speak aloud"
                }
            },
            "required": ["text"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SpeakInput {
    text: String,
}

fn err_result(call_id: &str, msg: impl Into<String>) -> ToolCallResult {
    ToolCallResult {
        content: MessageContent::tool_result(call_id, msg),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: None,
        attachment: None,
    }
}

pub(super) async fn handle_speak(call: &ToolCall) -> ToolCallResult {
    let input: SpeakInput = match serde_json::from_value(call.input.clone()) {
        Ok(i) => i,
        Err(err) => return err_result(&call.id, format!("invalid input: {err}")),
    };

    if input.text.trim().is_empty() {
        return err_result(&call.id, "text is empty");
    }

    let speak_text = if input.text.len() > MAX_SPEAK_CHARS {
        format!("{}...", &input.text[..MAX_SPEAK_CHARS])
    } else {
        input.text.clone()
    };

    let piper_path = match find_piper() {
        Some(p) => p,
        None => {
            return err_result(
                &call.id,
                "piper not found. install with: pip install piper-tts",
            );
        }
    };

    let model_path = default_model_path();
    if !model_path.exists() {
        return err_result(
            &call.id,
            format!(
                "TTS model not found at {}. download from: https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx\nalso get the config: https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json\nplace both in: {}",
                model_path.display(),
                tts_dir().display()
            ),
        );
    }

    let wav_bytes = match synthesize(&piper_path, &model_path, &speak_text).await {
        Ok(bytes) => bytes,
        Err(e) => return err_result(&call.id, format!("TTS failed: {e}")),
    };

    let ogg_bytes = match wav_to_ogg_opus(&wav_bytes).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!("ffmpeg conversion failed ({e}), attempting local playback");
            if let Err(play_err) = play_local(&wav_bytes).await {
                tracing::warn!("local playback also failed: {play_err}");
            }
            return err_result(
                &call.id,
                format!("spoke locally. ffmpeg not available for telegram voice: {e}"),
            );
        }
    };

    let chars = speak_text.len();
    ToolCallResult {
        content: MessageContent::tool_result(
            &call.id,
            format!("spoke {chars} chars as voice message"),
        ),
        switch_provider: None,
        complete: false,
        compact: false,
        voice: Some(ogg_bytes),
        attachment: None,
    }
}

fn tts_dir() -> PathBuf {
    config::ava_home_dir().join("tts")
}

fn default_model_path() -> PathBuf {
    tts_dir().join("en_US-lessac-medium.onnx")
}

fn find_piper() -> Option<String> {
    std::process::Command::new("which")
        .arg("piper")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

async fn synthesize(piper_path: &str, model_path: &PathBuf, text: &str) -> Result<Vec<u8>, String> {
    let mut child = tokio::process::Command::new(piper_path)
        .arg("--model")
        .arg(model_path)
        .arg("--output_file")
        .arg("/dev/stdout")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn piper: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("failed to write to piper stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("piper failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("piper exited with error: {stderr}"));
    }

    if output.stdout.is_empty() {
        return Err("piper produced no output".into());
    }

    Ok(output.stdout)
}

async fn wav_to_ogg_opus(wav_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            "-i", "pipe:0", "-c:a", "libopus", "-b:a", "48k", "-f", "ogg", "pipe:1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn ffmpeg: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(wav_bytes)
            .await
            .map_err(|e| format!("failed to write to ffmpeg stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("ffmpeg failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg error: {stderr}"));
    }

    Ok(output.stdout)
}

async fn play_local(wav_bytes: &[u8]) -> Result<(), String> {
    let player = if cfg!(target_os = "macos") {
        "afplay"
    } else {
        "aplay"
    };

    let temp_path = std::env::temp_dir().join("ava_speak.wav");
    tokio::fs::write(&temp_path, wav_bytes)
        .await
        .map_err(|e| format!("failed to write temp wav: {e}"))?;

    let status = tokio::process::Command::new(player)
        .arg(&temp_path)
        .status()
        .await
        .map_err(|e| format!("failed to play audio: {e}"))?;

    let _ = tokio::fs::remove_file(&temp_path).await;

    if !status.success() {
        return Err(format!("{player} exited with error"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speak_definition() {
        let def = speak_definition();
        assert_eq!(def.name(), SPEAK_TOOL_NAME);
    }

    #[test]
    fn test_default_model_path() {
        let path = default_model_path();
        assert!(path.to_str().unwrap().contains("tts"));
        assert!(path.to_str().unwrap().contains("en_US-lessac-medium.onnx"));
    }

    #[tokio::test]
    async fn test_handle_speak_empty_text() {
        let call = ToolCall {
            id: "test".into(),
            name: SPEAK_TOOL_NAME.into(),
            input: json!({"text": ""}),
        };
        let result = handle_speak(&call).await;
        assert!(result.voice.is_none());
    }

    #[tokio::test]
    async fn test_handle_speak_missing_text() {
        let call = ToolCall {
            id: "test".into(),
            name: SPEAK_TOOL_NAME.into(),
            input: json!({}),
        };
        let result = handle_speak(&call).await;
        assert!(result.voice.is_none());
    }
}
