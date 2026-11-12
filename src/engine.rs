//! Picks the speech recognition backend: OpenVINO on the Intel GPU (default, like the Python version) or
//! whisper.cpp on the CPU, which is also the fallback when the GPU backend can't start.

use anyhow::Result;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::openvino::OpenVinoWhisper;
use crate::whisper::{Part, VAD_MODEL, VAD_URL, Whisper};

/// Models whisper.cpp publishes an 8-bit quantized version of.
const QUANTIZABLE: &[&str] = &["tiny", "base", "small", "medium", "large-v3-turbo"];

/// What speech recognition runs on, shown as a badge in the window and in the tray tooltip.
#[derive(Clone, Debug)]
pub struct EngineInfo {
    pub gpu: bool,
    /// e.g. "GPU · OpenVINO" or "CPU · whisper.cpp"
    pub label: String,
    pub detail: String,
    /// Why the GPU backend could not start, when the CPU is a fallback.
    pub fallback: Option<String>,
}

pub enum Engine {
    Gpu { ov: OpenVinoWhisper, vad: Option<WhisperVadContext> },
    Cpu(Whisper),
}

impl Engine {
    /// Returns the engine and a description of what it runs on (for the status badge).
    pub fn load(model: &str, backend: &str, threads: i32, beam_size: i32) -> Result<(Engine, EngineInfo)> {
        let mut gpu_error = None;
        if backend != "cpu" {
            match OpenVinoWhisper::load(model, &backend.to_uppercase()) {
                Ok(ov) => {
                    let vad = crate::models::ensure(VAD_MODEL, VAD_URL)
                        .ok()
                        .and_then(|p| {
                            // Silero runs window by window; with more threads the synchronisation costs more than it saves.
                            let mut params = WhisperVadContextParams::default();
                            params.set_n_threads(1);
                            WhisperVadContext::new(&p.to_string_lossy(), params).ok()
                        });
                    if vad.is_none() {
                        log::warn!("No voice activity detection for the GPU backend");
                    }
                    log::info!("Whisper '{model}' runs on the {} via OpenVINO", backend.to_uppercase());
                    let info = EngineInfo {
                        gpu: true,
                        label: format!("{} · OpenVINO", backend.to_uppercase()),
                        detail: format!("Whisper '{model}' on the Intel {} via OpenVINO GenAI", backend.to_uppercase()),
                        fallback: None,
                    };
                    return Ok((Engine::Gpu { ov, vad }, info));
                }
                Err(e) => {
                    log::error!("OpenVINO {backend} backend failed; falling back to CPU: {e:#}");
                    gpu_error = Some(format!("{e:#}"));
                }
            }
        }
        // On the CPU the 8-bit quantized model is about twice as fast at practically the same accuracy.
        let quantized = format!("{model}-q8_0");
        let w = if QUANTIZABLE.contains(&model) {
            Whisper::load(&quantized, threads, beam_size).or_else(|e| {
                log::warn!("No quantized model ({e:#}); using '{model}'");
                Whisper::load(model, threads, beam_size)
            })?
        } else {
            Whisper::load(model, threads, beam_size)?
        };
        let used = if QUANTIZABLE.contains(&model) { quantized.as_str() } else { model };
        log::info!("Whisper '{used}' runs on the CPU (whisper.cpp)");
        let info = EngineInfo {
            gpu: false,
            label: "CPU · whisper.cpp".into(),
            detail: format!("Whisper '{used}' on the processor via whisper.cpp"),
            fallback: gpu_error,
        };
        Ok((Engine::Cpu(w), info))
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self, Engine::Gpu { .. })
    }

    /// Returns sentence segments and, when known, the language.
    pub fn transcribe(&mut self, audio: &[f32], language: Option<&str>) -> Result<(Vec<Part>, Option<String>)> {
        match self {
            Engine::Cpu(w) => w.transcribe(audio, language),
            Engine::Gpu { ov, vad } => {
                // No no-speech score here, so skip chunks Silero finds no speech in (avoids hallucinations).
                let started = std::time::Instant::now();
                if let Some(vad) = vad {
                    if vad.segments_from_samples(WhisperVadParams::default(), audio)?.num_segments() == 0 {
                        return Ok((Vec::new(), None));
                    }
                }
                let vad_time = started.elapsed();
                // Language detection is nearly free here, so it simply runs every time.
                let parts = ov.transcribe(audio, language)?;
                log::debug!("speech check {:.0} ms, recognition {:.0} ms", vad_time.as_secs_f32() * 1e3, (started.elapsed() - vad_time).as_secs_f32() * 1e3);
                Ok((parts, None))
            }
        }
    }
}
