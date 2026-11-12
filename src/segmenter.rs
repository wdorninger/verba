//! Splits a continuous audio stream into speech chunks using an adaptive energy VAD.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, TimeDelta};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::app::Core;

pub const RATE: usize = 16000;
const FRAME: usize = 480; // 30 ms
const PREROLL_FRAMES: usize = 10; // 300 ms kept before speech onset
const PARTIAL_EVERY: usize = RATE; // snapshot an unfinished chunk for live preview every 1 s of new audio

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    Me,
    Them,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::Me => "me",
            Source::Them => "them",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Source::Me => "Me",
            Source::Them => "Them",
        }
    }
    pub fn from_id(s: &str) -> Option<Source> {
        match s {
            "me" => Some(Source::Me),
            "them" => Some(Source::Them),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub source: Source,
    pub start: DateTime<Local>,
    pub audio: Vec<f32>,
    pub chunk_id: u64,
}

impl Chunk {
    pub fn end(&self) -> DateTime<Local> {
        self.start + TimeDelta::microseconds((self.audio.len() as f64 / RATE as f64 * 1e6) as i64)
    }
}

/// Holds only the newest unfinished chunk per source; older snapshots are simply overwritten.
#[derive(Default)]
pub struct LatestPartials {
    items: Mutex<HashMap<Source, Chunk>>,
}

impl LatestPartials {
    pub fn put(&self, chunk: Chunk) {
        self.items.lock().unwrap().insert(chunk.source, chunk);
    }
    pub fn discard(&self, source: Source) {
        self.items.lock().unwrap().remove(&source);
    }
    pub fn pop(&self) -> Option<Chunk> {
        let mut items = self.items.lock().unwrap();
        let key = *items.keys().next()?;
        items.remove(&key)
    }
}

pub type AudioBlock = (u32, Vec<f32>);

struct Segmenter {
    source: Source,
    out: Sender<Chunk>,
    partials: Option<Arc<LatestPartials>>,
    chunk_id: u64,
    last_partial_len: usize,
    silence_samples: usize,
    max_samples: usize,
    min_speech_samples: usize,
    silence_timeout: Duration,
    min_rms: f32,
    noise_ratio: f32,

    noise: f32,
    pending: Vec<f32>,
    preroll: VecDeque<Vec<f32>>,
    chunk: Option<Vec<f32>>,
    chunk_start: DateTime<Local>,
    speech_samples: usize,
    silence: usize,
    last_frame_at: Instant,
}

pub fn run(core: Arc<Core>, source: Source, input: Receiver<AudioBlock>, out: Sender<Chunk>) {
    let (silence_s, max_s, min_speech_s, min_rms, noise_ratio) = {
        let c = core.cfg.lock().unwrap();
        (
            c.f64("segmenter", "silence_seconds"),
            c.f64("segmenter", "max_chunk_seconds"),
            c.f64("segmenter", "min_speech_seconds"),
            c.f64("segmenter", "min_rms") as f32,
            c.f64("segmenter", "noise_ratio") as f32,
        )
    };
    let mut s = Segmenter {
        source,
        out,
        partials: core.partials.clone(),
        chunk_id: 0,
        last_partial_len: 0,
        silence_samples: (silence_s * RATE as f64) as usize,
        max_samples: (max_s * RATE as f64) as usize,
        min_speech_samples: (min_speech_s * RATE as f64) as usize,
        silence_timeout: Duration::from_secs_f64(silence_s),
        min_rms,
        noise_ratio,
        noise: min_rms,
        pending: Vec::new(),
        preroll: VecDeque::with_capacity(PREROLL_FRAMES + 1),
        chunk: None,
        chunk_start: Local::now(),
        speech_samples: 0,
        silence: 0,
        last_frame_at: Instant::now(),
    };
    let mut resampler: Option<(u32, sherpa_onnx::LinearResampler)> = None;
    while !core.stop.load(Ordering::Relaxed) {
        let (rate, block) = match input.recv_timeout(Duration::from_millis(100)) {
            Ok(b) => b,
            Err(RecvTimeoutError::Timeout) => {
                // Loopback delivers nothing while no audio plays, so silence must be detected by wall clock too.
                if s.chunk.is_some() && s.last_frame_at.elapsed() >= s.silence_timeout {
                    s.flush();
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let block = if rate as usize == RATE {
            block
        } else {
            if resampler.as_ref().map(|r| r.0) != Some(rate) {
                match sherpa_onnx::LinearResampler::create(rate as i32, RATE as i32) {
                    Some(r) => resampler = Some((rate, r)),
                    None => {
                        log::error!("No resampler for {rate} Hz");
                        continue;
                    }
                }
            }
            resampler.as_ref().unwrap().1.resample(&block, false)
        };
        s.pending.extend_from_slice(&block);
        let n = s.pending.len() / FRAME;
        let pending = std::mem::take(&mut s.pending);
        for i in 0..n {
            s.process(&pending[i * FRAME..(i + 1) * FRAME]);
        }
        s.pending = pending[n * FRAME..].to_vec();
    }
    s.flush();
}

impl Segmenter {
    fn process(&mut self, frame: &[f32]) {
        self.last_frame_at = Instant::now();
        let rms = (frame.iter().map(|v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
        // Noise floor drops instantly and rises slowly (~30 s), so steady background noise is learned.
        self.noise = if rms < self.noise { rms } else { self.noise * 0.999 + rms * 0.001 };
        let speech = rms > self.min_rms.max(self.noise * self.noise_ratio);

        let Some(chunk) = self.chunk.as_mut() else {
            if speech {
                let mut chunk: Vec<f32> = self.preroll.drain(..).flatten().collect();
                chunk.extend_from_slice(frame);
                let secs = chunk.len() as f64 / RATE as f64;
                self.chunk_start = Local::now() - TimeDelta::microseconds((secs * 1e6) as i64);
                self.chunk = Some(chunk);
                self.speech_samples = FRAME;
                self.silence = 0;
                self.chunk_id += 1;
                self.last_partial_len = 0;
            } else {
                if self.preroll.len() == PREROLL_FRAMES {
                    self.preroll.pop_front();
                }
                self.preroll.push_back(frame.to_vec());
            }
            return;
        };

        chunk.extend_from_slice(frame);
        let chunk_len = chunk.len();
        if speech {
            self.speech_samples += FRAME;
            self.silence = 0;
        } else {
            self.silence += FRAME;
        }
        if self.silence >= self.silence_samples || chunk_len >= self.max_samples {
            self.flush();
        } else if let Some(partials) = &self.partials {
            if speech && chunk_len - self.last_partial_len >= PARTIAL_EVERY {
                self.last_partial_len = chunk_len;
                partials.put(Chunk {
                    source: self.source,
                    start: self.chunk_start,
                    audio: chunk.clone(),
                    chunk_id: self.chunk_id,
                });
            }
        }
    }

    fn flush(&mut self) {
        if let Some(p) = &self.partials {
            p.discard(self.source);
        }
        if let Some(audio) = self.chunk.take() {
            let make = |audio| Chunk { source: self.source, start: self.chunk_start, audio, chunk_id: self.chunk_id };
            if self.speech_samples >= self.min_speech_samples {
                let _ = self.out.send(make(audio));
            } else if self.last_partial_len > 0 {
                // Too short to keep, but a preview was shown: an empty chunk tells the transcriber to clear it.
                let _ = self.out.send(make(Vec::new()));
            }
        }
        self.speech_samples = 0;
        self.silence = 0;
    }
}
