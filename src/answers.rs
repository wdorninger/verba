//! Spots questions in the live transcript and lets Claude (via the CLI) answer them.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::app::{Core, UiEvent};
use crate::claude::run_claude;
use crate::segmenter::Source;

const QUESTION_WORDS: &[&str] = &[
    // German
    "wer", "wen", "wem", "wessen", "was", "wann", "wo", "woher", "wohin", "wie", "warum", "wieso", "weshalb",
    "welche", "welcher", "welches", "welchen", "kann", "kannst", "können", "könnte", "könnten", "hast", "hat",
    "haben", "habt", "ist", "sind", "gibt", "weißt", "wisst", "wissen", "soll", "sollen", "sollte", "muss",
    "müssen", "wird", "werden", "darf", "dürfen",
    // English
    "who", "whom", "whose", "what", "when", "where", "why", "how", "which", "can", "could", "would", "should",
    "do", "does", "did", "is", "are", "was", "were", "will", "have", "has", "may", "might", "shall",
];
const MAX_WAITING: usize = 2; // stale questions are worthless live, so only the newest ones are kept
const REPEAT: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(90);
const WEB_TIMEOUT: Duration = Duration::from_secs(150);

const SYSTEM_PROMPT: &str = "You are a live meeting assistant. You receive the recent part of a conversation (an \
automatic speech transcript that may contain recognition errors) and one marked question from it. Answer that \
question helpfully and concisely: at most about 5 short sentences or bullet points, in the language of the \
question, using the conversation for context. No preamble, no repeating the question. \
If it is not a genuine question worth answering (small talk like 'how are you', rhetorical, a purely \
personal or organisational question only the participants can answer, or already answered in the \
conversation), reply with exactly: SKIP";

/// Cheap local pre-filter so the CLI is only called for likely questions.
pub fn find_question(text: &str, min_words: usize) -> Option<String> {
    let words_re = Regex::new(r"[\w']+").unwrap();
    // Split into sentences after . ! or ? followed by whitespace.
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.trim().char_indices().collect();
    let text = text.trim();
    for w in chars.windows(2) {
        if ".!?".contains(w[0].1) && w[1].1.is_whitespace() {
            sentences.push(&text[start..w[1].0]);
            start = w[1].0;
        }
    }
    sentences.push(&text[start..]);
    for sentence in sentences {
        let sentence = sentence.trim();
        let words: Vec<&str> = words_re.find_iter(sentence).map(|m| m.as_str()).collect();
        if words.len() < min_words {
            continue;
        }
        if sentence.ends_with('?') || QUESTION_WORDS.contains(&words[0].to_lowercase().as_str()) {
            return Some(sentence.to_string());
        }
    }
    None
}

struct Question {
    qid: u64,
    time: String,
    label: String,
    text: String,
    context: Vec<String>,
    manual: bool,
}

#[derive(Default)]
struct State {
    context: VecDeque<String>,
    waiting: VecDeque<Question>,
    recent: HashMap<String, Instant>,
    next_id: u64,
}

#[derive(Default)]
pub struct Answerer {
    state: Mutex<State>,
    wake: Condvar,
}

impl Answerer {
    /// Every transcript line passes through here (called from the transcriber thread).
    pub fn on_line(&self, core: &Core, source: Source, time: &str, label: &str, text: &str) {
        {
            let mut s = self.state.lock().unwrap();
            if s.context.len() == 50 {
                s.context.pop_front();
            }
            s.context.push_back(format!("[{time}] [{label}] {text}"));
        }
        let (active, wanted, min_words) = {
            let c = core.cfg.lock().unwrap();
            (
                c.bool("summary", "enabled") && c.bool("answers", "enabled"),
                c.str("answers", "sources"),
                c.i64("answers", "min_words").max(1) as usize,
            )
        };
        if !active || (wanted != "all" && wanted != source.id()) {
            return;
        }
        if let Some(q) = find_question(text, min_words) {
            self.enqueue(core, time, label, &q, false);
        }
    }

    /// Renamed speaker: recent lines sent to Claude as context use the new name too.
    pub fn rename(&self, old_label: &str, new_label: &str) {
        let (old, new) = (format!("] [{old_label}] "), format!("] [{new_label}] "));
        for line in self.state.lock().unwrap().context.iter_mut() {
            if line.contains(&old) {
                *line = line.replace(&old, &new);
            }
        }
    }

    /// Manual "Ask Claude about this" from the window; skips the question filter.
    pub fn ask(&self, core: &Core, time: &str, label: &str, text: &str) {
        self.enqueue(core, time, label, text.trim(), true);
    }

    fn enqueue(&self, core: &Core, time: &str, label: &str, text: &str, manual: bool) {
        let key = Regex::new(r"\W+").unwrap().replace_all(&text.to_lowercase(), " ").trim().to_string();
        let context_lines = core.cfg.lock().unwrap().i64("answers", "context_lines").max(0) as usize;
        let mut dropped = Vec::new();
        let qid;
        {
            let mut s = self.state.lock().unwrap();
            if !manual && s.recent.get(&key).is_some_and(|t| t.elapsed() < REPEAT) {
                return; // echo or the same question repeated
            }
            s.recent.insert(key, Instant::now());
            s.next_id += 1;
            qid = s.next_id;
            let skip = s.context.len().saturating_sub(context_lines);
            let context = s.context.iter().skip(skip).cloned().collect();
            s.waiting.push_back(Question {
                qid,
                time: time.into(),
                label: label.into(),
                text: text.into(),
                context,
                manual,
            });
            while s.waiting.len() > MAX_WAITING {
                dropped.push(s.waiting.pop_front().unwrap().qid);
            }
        }
        for d in dropped {
            core.ui.send(UiEvent::AnswerSkipped(d));
        }
        core.ui.send(UiEvent::AnswerPending { qid, time: time.into(), label: label.into(), question: text.into() });
        self.wake.notify_one();
    }

    pub fn run(core: Arc<Core>) {
        let me = &core.answerer;
        while !core.stop.load(Ordering::Relaxed) {
            let q = {
                let s = me.state.lock().unwrap();
                let (mut s, _) = me.wake.wait_timeout_while(s, Duration::from_millis(500), |s| s.waiting.is_empty()).unwrap();
                s.waiting.pop_front()
            };
            if let Some(q) = q {
                Self::answer(&core, q);
            }
        }
    }

    fn answer(core: &Core, q: Question) {
        let (web, model) = {
            let c = core.cfg.lock().unwrap();
            (c.bool("answers", "web_search"), c.str("answers", "model"))
        };
        let context = if q.context.is_empty() { "(none)".to_string() } else { q.context.join("\n") };
        let mut prompt = format!(
            "Recent conversation:\n{context}\n\nQuestion to answer (asked by {} at {}):\n\"{}\"",
            q.label, q.time, q.text
        );
        if q.manual {
            prompt.push_str("\n\nThe user explicitly asked for this one, so do not reply SKIP; answer or explain it.");
        }
        let started = Instant::now();
        let tools: &[&str] = if web { &["WebSearch"] } else { &[] };
        match run_claude(&prompt, SYSTEM_PROMPT, &model, if web { WEB_TIMEOUT } else { TIMEOUT }, core.writer.directory(), tools)
        {
            Err(e) => {
                log::warn!("Answer failed: {e:#}");
                let msg: String = format!("{e:#}").chars().take(300).collect();
                core.ui.send(UiEvent::AnswerError { qid: q.qid, message: msg });
            }
            Ok(answer) => {
                let took = started.elapsed().as_secs_f32();
                let skip = Regex::new(r"(?i)^\W*SKIP\W*$").unwrap();
                if skip.is_match(answer.trim()) {
                    log::info!("Claude skipped ({took:.1}s): {}", q.text);
                    core.ui.send(UiEvent::AnswerSkipped(q.qid));
                } else {
                    log::info!("Claude answered ({took:.1}s, {model}): {}", q.text);
                    core.ui.send(UiEvent::AnswerDone { qid: q.qid, text: answer });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_question;

    #[test]
    fn spots_questions() {
        assert_eq!(find_question("Okay. Can you send me the file today?", 4).as_deref(), Some("Can you send me the file today?"));
        assert_eq!(find_question("Wie weit seid ihr mit dem Projekt", 4).as_deref(), Some("Wie weit seid ihr mit dem Projekt"));
        assert_eq!(find_question("Ja?", 4), None);
        assert_eq!(find_question("That is fine. We will do it tomorrow.", 4), None);
    }

    #[test]
    fn rename_updates_context_lines() {
        let a = super::Answerer::default();
        {
            let mut s = a.state.lock().unwrap();
            s.context.push_back("[10:00:00] [Speaker 1] Hello".into());
            s.context.push_back("[10:00:05] [Speaker 12] Hi".into());
        }
        a.rename("Speaker 1", "Hanna");
        let s = a.state.lock().unwrap();
        assert_eq!(s.context[0], "[10:00:00] [Hanna] Hello");
        assert_eq!(s.context[1], "[10:00:05] [Speaker 12] Hi");
    }
}
