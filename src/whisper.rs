//! Speech recognition with whisper.cpp (via whisper-rs), including its built-in Silero VAD.

use anyhow::{Context, Result};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperVadParams,
};

use crate::segmenter::RATE;

pub const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
pub const VAD_URL: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin";

/// (start_s, end_s, text)
pub type Part = (f32, f32, String);

pub struct Whisper {
    ctx: WhisperContext,
    state: WhisperState,
    vad_model: Option<String>,
    threads: i32,
    beam_size: i32,
}

impl Whisper {
    /// `model`: tiny, base, small, medium, large-v3, large-v3-turbo, or a quantized name like "small-q5_1".
    pub fn load(model: &str, threads: i32, beam_size: i32) -> Result<Whisper> {
        whisper_rs::install_logging_hooks(); // keeps whisper.cpp from printing to stderr
        let file = format!("ggml-{model}.bin");
        let path = crate::models::ensure(
            &file,
            &format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}"),
        )
        .with_context(|| format!("Whisper model '{model}'"))?;
        let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
            .with_context(|| format!("could not load {}", path.display()))?;
        let state = ctx.create_state()?;
        let vad_model = match crate::models::ensure(VAD_MODEL, VAD_URL) {
            Ok(p) => Some(p.to_string_lossy().into_owned()),
            Err(e) => {
                log::warn!("No voice activity detection: {e:#}");
                None
            }
        };
        let threads = if threads > 0 {
            threads
        } else {
            std::thread::available_parallelism().map(|n| n.get().min(8) as i32).unwrap_or(4)
        };
        Ok(Whisper { ctx, state, vad_model, threads, beam_size: beam_size.max(1) })
    }

    /// Transcribes 16 kHz mono audio. Returns sentence segments and the language that was used or detected.
    pub fn transcribe(&mut self, audio: &[f32], language: Option<&str>) -> Result<(Vec<Part>, Option<String>)> {
        let strategy = if self.beam_size > 1 {
            SamplingStrategy::BeamSearch { beam_size: self.beam_size, patience: -1.0 }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_n_threads(self.threads);
        params.set_language(Some(language.unwrap_or("auto")));
        params.set_no_context(true); // like condition_on_previous_text=False
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        if let Some(vad) = &self.vad_model {
            params.set_vad_model_path(Some(vad));
            params.enable_vad(true);
            params.set_vad_params(WhisperVadParams::default());
        }
        // whisper.cpp ignores input shorter than 1 s, so pad short chunks with silence.
        let mut samples = audio.to_vec();
        if samples.len() < RATE * 6 / 5 {
            samples.resize(RATE * 6 / 5, 0.0);
        }
        self.state.full(params, &samples)?;

        let eot = self.ctx.token_eot();
        let secs = audio.len() as f32 / RATE as f32;
        let mut parts = Vec::new();
        for seg in self.state.as_iter() {
            let text = seg.to_str_lossy()?.trim().to_string();
            if text.is_empty() {
                continue;
            }
            // Drop segments Whisper itself considers probably silent (like faster-whisper's filter).
            let logprobs: Vec<f32> = (0..seg.n_tokens())
                .filter_map(|i| seg.get_token(i))
                .filter(|t| t.token_id() < eot)
                .map(|t| t.token_data().plog)
                .collect();
            let avg_logprob = logprobs.iter().sum::<f32>() / logprobs.len().max(1) as f32;
            if seg.no_speech_probability() > 0.6 && avg_logprob < -1.0 {
                continue;
            }
            let start = (seg.start_timestamp() as f32 / 100.0).min(secs);
            let end = (seg.end_timestamp() as f32 / 100.0).clamp(start, secs);
            parts.push((start, end, text));
        }
        let detected = whisper_rs::get_lang_str(self.state.full_lang_id_from_state()).map(str::to_string);
        Ok((parts, language.map(str::to_string).or(detected)))
    }
}
