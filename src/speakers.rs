//! Tells call participants apart by voice and remembers the ones the user has named.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

use crate::segmenter::{RATE, Source};

pub const MODEL_NAME: &str = "wespeaker_en_voxceleb_resnet34.onnx";
const MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx";
const RECENT_SECONDS: f64 = 10.0; // short chunks are attributed to whoever spoke within this window
const MAX_COUNT: u32 = 50; // caps the centroid weight so a voice can keep adapting
/// How far below the threshold a named voice may still match (and win over unnamed ones). Different people
/// can score around 0.7 on call audio, so this stays small.
const NAMED_MARGIN: f32 = 0.08;

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn weighted(a: &[f32], wa: u32, b: &[f32], wb: u32) -> Vec<f32> {
    normalize(a.iter().zip(b).map(|(x, y)| x * wa as f32 + y * wb as f32).collect())
}

/// Voice embeddings with the wespeaker ResNet34 model, computed by sherpa-onnx.
pub struct Embedder {
    extractor: SpeakerEmbeddingExtractor,
}

impl Embedder {
    pub fn load() -> Result<Embedder> {
        let path = crate::models::ensure(MODEL_NAME, MODEL_URL)?;
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(path.to_string_lossy().into_owned()),
            num_threads: 2,
            ..Default::default()
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config).context("could not load the voice model")?;
        Ok(Embedder { extractor })
    }

    pub fn embed(&self, audio: &[f32]) -> Result<Vec<f32>> {
        let stream = self.extractor.create_stream().context("voice model stream")?;
        stream.accept_waveform(RATE as i32, audio);
        stream.input_finished();
        anyhow::ensure!(self.extractor.is_ready(&stream), "audio too short for a voiceprint");
        Ok(normalize(self.extractor.compute(&stream).context("voiceprint failed")?))
    }
}

#[derive(Clone)]
struct Speaker {
    id: String,
    name: String,
    centroid: Vec<f32>,
    count: u32,
    named: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedSpeaker {
    id: String,
    name: String,
    count: u32,
    centroid: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
struct SavedFile {
    model: String,
    speakers: Vec<SavedSpeaker>,
}

struct Inner {
    speakers: Vec<Speaker>,
    next_number: u32,
    last: Vec<(Source, String, Instant)>, // source -> (speaker id, time)
    /// Voices merged into another one: old id -> surviving id.
    merged: HashMap<String, String>,
}

impl Inner {
    /// Follows merges, so an id from before a merge leads to the voice it was merged into.
    fn resolve<'a>(&'a self, mut id: &'a str) -> &'a str {
        while let Some(next) = self.merged.get(id) {
            id = next;
        }
        id
    }

    /// Merges voice `from` into voice `into` (indices); returns the new index of `into`.
    fn merge(&mut self, from: usize, into: usize) -> usize {
        let sp = self.speakers.remove(from);
        let into = if into > from { into - 1 } else { into };
        let other = &mut self.speakers[into];
        other.centroid = weighted(&other.centroid, other.count, &sp.centroid, sp.count);
        other.count = (other.count + sp.count).min(MAX_COUNT);
        let oid = other.id.clone();
        for entry in self.last.iter_mut() {
            if entry.1 == sp.id {
                entry.1 = oid.clone();
            }
        }
        self.merged.insert(sp.id, oid);
        into
    }

    /// The voice a voiceprint belongs to, or None. Named voices win: one that is within `margin` of the
    /// threshold and nearly as close as the best match is taken, so a renamed person keeps the name even
    /// when a sentence sounds a little different. An unnamed voice that turns out to sound like a named one
    /// (their averaged voiceprints match) is merged into it.
    fn choose(&mut self, emb: &[f32], threshold: f32, margin: f32) -> Option<usize> {
        let (mut best, mut score) = (None, -1.0f32);
        let (mut named, mut named_score) = (None, -1.0f32);
        for (i, sp) in self.speakers.iter().enumerate() {
            let s = dot(emb, &sp.centroid);
            if s > score {
                best = Some(i);
                score = s;
            }
            if sp.named && s > named_score {
                named = Some(i);
                named_score = s;
            }
        }
        if let Some(n) = named {
            if named_score >= threshold - margin && named_score >= score - margin / 2.0 {
                return Some(n);
            }
        }
        let b = best.filter(|_| score >= threshold)?;
        if !self.speakers[b].named {
            let centroid = self.speakers[b].centroid.clone();
            let closest = self
                .speakers
                .iter()
                .enumerate()
                .filter(|(_, sp)| sp.named)
                .map(|(i, sp)| (i, dot(&centroid, &sp.centroid)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((n, s)) = closest.filter(|(_, s)| *s >= threshold) {
                log::info!("{} sounds like {} ({s:.2}): merged", self.speakers[b].name, self.speakers[n].name);
                return Some(self.merge(b, n));
            }
        }
        Some(b)
    }
}

pub struct SpeakerRegistry {
    path: PathBuf,
    threshold: Mutex<f32>,
    min_samples: usize,
    inner: Mutex<Inner>,
    embedder: Mutex<Option<Embedder>>,
}

fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:08x}", h.finish() as u32)
}

impl SpeakerRegistry {
    pub fn new(path: PathBuf, threshold: f64, min_seconds: f64) -> SpeakerRegistry {
        let r = SpeakerRegistry {
            path,
            threshold: Mutex::new(threshold as f32),
            min_samples: (min_seconds * RATE as f64) as usize,
            inner: Mutex::new(Inner { speakers: Vec::new(), next_number: 1, last: Vec::new(), merged: HashMap::new() }),
            embedder: Mutex::new(None),
        };
        r.load();
        r
    }

    pub fn set_threshold(&self, t: f64) {
        *self.threshold.lock().unwrap() = t as f32;
    }

    pub fn load_model(&self) -> Result<()> {
        let e = Embedder::load()?;
        *self.embedder.lock().unwrap() = Some(e);
        log::info!("Speaker model loaded ({} known speakers)", self.inner.lock().unwrap().speakers.len());
        Ok(())
    }

    /// The current (id, name) of a voice, following renames and merges. Lines identified before a rename or merge
    /// are written with this, so the new name is used everywhere from then on.
    pub fn current(&self, speaker_id: &str) -> Option<(String, String)> {
        let inner = self.inner.lock().unwrap();
        let id = inner.resolve(speaker_id);
        inner.speakers.iter().find(|s| s.id == id).map(|s| (s.id.clone(), s.name.clone()))
    }

    /// Whoever spoke on this source within the last few seconds, as a fallback for audio too short to tell.
    pub fn last_recent(&self, source: Source) -> Option<(String, String)> {
        let inner = self.inner.lock().unwrap();
        let (_, id, at) = inner.last.iter().find(|(s, _, _)| *s == source)?;
        if at.elapsed().as_secs_f64() >= RECENT_SECONDS {
            return None;
        }
        let sp = inner.speakers.iter().find(|s| &s.id == id)?;
        Some((sp.id.clone(), sp.name.clone()))
    }

    /// Returns (speaker_id, name), or None when short audio matches no known voice clearly.
    pub fn identify(&self, audio: &[f32], source: Source) -> Result<Option<(String, String)>> {
        let emb = {
            let e = self.embedder.lock().unwrap();
            let Some(e) = e.as_ref() else { return Ok(None) };
            // Below 0.5 s a voiceprint is mostly noise; otherwise short audio may still match a known voice.
            if audio.len() >= RATE / 2 { Some(e.embed(audio)?) } else { None }
        };
        let long_enough = audio.len() >= self.min_samples;
        let base = *self.threshold.lock().unwrap();
        // Short voiceprints are noisier and score lower, so they get a looser bar (but never create a voice).
        let threshold = if long_enough { base } else { base - 0.15 };
        let mut inner = self.inner.lock().unwrap();
        let chosen = emb.as_ref().and_then(|e| inner.choose(e, threshold, NAMED_MARGIN));
        log::debug!(
            "{:.1}s: voice {:?}",
            audio.len() as f32 / RATE as f32,
            chosen.map(|i| inner.speakers[i].name.clone())
        );
        let index = match chosen {
            Some(i) => {
                if long_enough {
                    // only reliable voiceprints refine the centroid
                    let sp = &mut inner.speakers[i];
                    sp.centroid = weighted(&sp.centroid, sp.count, emb.as_ref().unwrap(), 1);
                    sp.count = (sp.count + 1).min(MAX_COUNT);
                }
                i
            }
            None if long_enough => {
                let emb = emb.unwrap();
                let score = inner.speakers.iter().map(|s| dot(&emb, &s.centroid)).fold(-1.0f32, f32::max);
                let name = format!("Speaker {}", inner.next_number);
                inner.next_number += 1;
                log::info!("New voice: {name} (best previous match {score:.2})");
                inner.speakers.push(Speaker { id: random_id(), name, centroid: emb, count: 1, named: false });
                inner.speakers.len() - 1
            }
            None => return Ok(None), // the caller decides from the neighbouring sentences
        };
        let (id, name) = (inner.speakers[index].id.clone(), inner.speakers[index].name.clone());
        inner.last.retain(|(s, _, _)| *s != source);
        inner.last.push((source, id.clone(), Instant::now()));
        Ok(Some((id, name)))
    }

    /// Names a speaker; an existing name merges both voices. Returns (old_name, surviving_id, name).
    pub fn rename(&self, speaker_id: &str, new_name: &str) -> Option<(String, String, String)> {
        let new_name = new_name.trim();
        let mut inner = self.inner.lock().unwrap();
        let speaker_id = inner.resolve(speaker_id).to_string();
        let speaker_id = speaker_id.as_str();
        let i = inner.speakers.iter().position(|s| s.id == speaker_id)?;
        if new_name.is_empty() {
            return None;
        }
        let old_name = inner.speakers[i].name.clone();
        let other =
            inner.speakers.iter().position(|o| o.id != speaker_id && o.name.to_lowercase() == new_name.to_lowercase());
        let (surviving_id, final_name) = if let Some(o) = other {
            let o = inner.merge(i, o);
            let other = &mut inner.speakers[o];
            other.named = true;
            (other.id.clone(), other.name.clone())
        } else {
            let sp = &mut inner.speakers[i];
            sp.name = new_name.to_string();
            sp.named = true;
            (sp.id.clone(), sp.name.clone())
        };
        if let Err(e) = self.save_locked(&inner) {
            log::error!("Could not save {}: {e:#}", self.path.display());
        }
        log::info!("Renamed {old_name} -> {final_name}");
        Some((old_name, surviving_id, final_name))
    }

    /// Known voices as (id, name, named), named ones first.
    pub fn list(&self) -> Vec<(String, String, bool)> {
        let inner = self.inner.lock().unwrap();
        let mut v: Vec<_> = inner.speakers.iter().map(|s| (s.id.clone(), s.name.clone(), s.named)).collect();
        v.sort_by_key(|s| !s.2);
        v
    }

    /// Forgets one voice (its voiceprint and name); returns its name. Later sentences of that person start
    /// again as a new speaker.
    pub fn forget(&self, speaker_id: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.resolve(speaker_id).to_string();
        let i = inner.speakers.iter().position(|s| s.id == id)?;
        let sp = inner.speakers.remove(i);
        inner.merged.retain(|from, into| *from != id && *into != id);
        inner.last.retain(|(_, l, _)| *l != id);
        if let Err(e) = self.save_locked(&inner) {
            log::error!("Could not save {}: {e:#}", self.path.display());
        }
        log::info!("Forgot voice {}", sp.name);
        Some(sp.name)
    }

    /// Forgets every voice and name and deletes speakers.json; returns how many named voices were dropped.
    /// Numbering continues, so new voices never reuse a "Speaker N" label already in the transcript.
    pub fn clear(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let named = inner.speakers.iter().filter(|s| s.named).count();
        inner.speakers.clear();
        inner.merged.clear();
        inner.last.clear();
        if self.path.exists() {
            if let Err(e) = std::fs::remove_file(&self.path) {
                log::error!("Could not delete {}: {e}", self.path.display());
            }
        }
        log::info!("Forgot all voices ({named} named)");
        named
    }

    // --- persistence (only named speakers survive a restart) ---
    fn load(&self) {
        if !self.path.exists() {
            return;
        }
        let parsed = std::fs::read_to_string(&self.path)
            .map_err(anyhow::Error::from)
            .and_then(|t| serde_json::from_str::<SavedFile>(&t).map_err(anyhow::Error::from));
        match parsed {
            Ok(data) if data.model != MODEL_NAME => log::warn!("speakers.json was made with another model; ignoring it"),
            Ok(data) => {
                let mut inner = self.inner.lock().unwrap();
                for s in data.speakers {
                    inner.speakers.push(Speaker {
                        id: s.id,
                        name: s.name,
                        centroid: normalize(s.centroid),
                        count: s.count,
                        named: true,
                    });
                }
            }
            Err(e) => log::error!("Could not read {}: {e:#}", self.path.display()),
        }
    }

    pub fn save(&self) {
        let inner = self.inner.lock().unwrap();
        if let Err(e) = self.save_locked(&inner) {
            log::error!("Could not save {}: {e:#}", self.path.display());
        }
    }

    fn save_locked(&self, inner: &Inner) -> Result<()> {
        let data = SavedFile {
            model: MODEL_NAME.to_string(),
            speakers: inner
                .speakers
                .iter()
                .filter(|s| s.named)
                .map(|s| SavedSpeaker {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    count: s.count,
                    centroid: s.centroid.iter().map(|x| (x * 1e5).round() / 1e5).collect(),
                })
                .collect(),
        };
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SpeakerRegistry {
        let path = std::env::temp_dir().join(format!("transcribe-test-{}.json", random_id()));
        let r = SpeakerRegistry::new(path, 0.75, 1.5);
        let mut inner = r.inner.lock().unwrap();
        for (id, name, c) in [("a", "Speaker 1", [1.0, 0.0]), ("b", "Speaker 2", [0.0, 1.0])] {
            inner.speakers.push(Speaker { id: id.into(), name: name.into(), centroid: c.to_vec(), count: 1, named: false });
        }
        drop(inner);
        r
    }

    #[test]
    fn renamed_voice_keeps_its_new_name() {
        let r = registry();
        assert_eq!(r.rename("a", "Hanna"), Some(("Speaker 1".into(), "a".into(), "Hanna".into())));
        assert_eq!(r.current("a"), Some(("a".into(), "Hanna".into())));
        let _ = std::fs::remove_file(&r.path);
    }

    fn inner(voices: &[(&str, &str, &[f32], bool)]) -> Inner {
        Inner {
            speakers: voices
                .iter()
                .map(|(id, name, c, named)| Speaker {
                    id: id.to_string(),
                    name: name.to_string(),
                    centroid: normalize(c.to_vec()),
                    count: 5,
                    named: *named,
                })
                .collect(),
            next_number: 3,
            last: Vec::new(),
            merged: HashMap::new(),
        }
    }

    #[test]
    fn named_voice_wins_just_below_the_threshold() {
        let mut i = inner(&[("a", "Hanna", &[1.0, 0.0], true), ("b", "Speaker 2", &[0.0, 1.0], false)]);
        // 0.70 to Hanna (below 0.75, within the margin), 0.71 to the unnamed voice: Hanna.
        assert_eq!(i.choose(&normalize(vec![0.70, 0.714]), 0.75, NAMED_MARGIN), Some(0));
        // Clearly someone else: 0.6 to Hanna, 0.8 to the unnamed voice.
        assert_eq!(i.choose(&normalize(vec![0.6, 0.8]), 0.75, NAMED_MARGIN), Some(1));
        // Nobody close: a new voice (None).
        let mut j = inner(&[("a", "Hanna", &[1.0, 0.0, 0.0], true)]);
        assert_eq!(j.choose(&[0.0, 0.0, 1.0], 0.75, NAMED_MARGIN), None);
    }

    #[test]
    fn unnamed_voice_that_sounds_like_a_named_one_is_merged() {
        // "c" was created for Hanna's voice before she was named (voiceprints 0.8 alike).
        let mut i = inner(&[("a", "Hanna", &[1.0, 0.0, 0.0], true), ("c", "Speaker 3", &[0.8, 0.6, 0.0], false)]);
        let chosen = i.choose(&normalize(vec![0.3, 0.95, 0.0]), 0.75, NAMED_MARGIN);
        assert_eq!(chosen.map(|k| i.speakers[k].name.as_str()), Some("Hanna"));
        assert_eq!(i.speakers.len(), 1);
        assert_eq!(i.resolve("c"), "a");
    }

    #[test]
    fn clear_forgets_voices_and_the_file() {
        let r = registry();
        r.rename("a", "Hanna");
        assert!(r.path.exists());
        assert_eq!(r.clear(), 1);
        assert!(!r.path.exists());
        assert_eq!(r.current("a"), None);
        assert!(r.inner.lock().unwrap().speakers.is_empty());
    }

    #[test]
    fn forget_drops_one_voice() {
        let r = registry();
        r.rename("a", "Hanna");
        assert_eq!(r.list().iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), ["Hanna", "Speaker 2"]);
        assert_eq!(r.forget("a"), Some("Hanna".into()));
        assert_eq!(r.current("a"), None);
        assert_eq!(r.current("b"), Some(("b".into(), "Speaker 2".into())));
        assert!(!std::fs::read_to_string(&r.path).unwrap().contains("Hanna"));
        let _ = std::fs::remove_file(&r.path);
    }

    #[test]
    fn merged_voice_resolves_to_the_survivor() {
        let r = registry();
        r.rename("a", "Hanna");
        // Same name: "b" is merged into "a"; lines identified as "b" earlier now resolve to Hanna.
        assert_eq!(r.rename("b", "hanna"), Some(("Speaker 2".into(), "a".into(), "Hanna".into())));
        assert_eq!(r.current("b"), Some(("a".into(), "Hanna".into())));
        r.rename("b", "Hanna Meier"); // renaming via the old id renames the survivor
        assert_eq!(r.current("a"), Some(("a".into(), "Hanna Meier".into())));
        let _ = std::fs::remove_file(&r.path);
    }
}
