//! Whisper on the Intel GPU through OpenVINO GenAI's C API (openvino_genai_c.dll), OpenVINO Whisper models
//! (~3x faster than the CPU). The DLLs are loaded at runtime from the `openvino` folder
//! next to the executable, so the app still runs (on the CPU backend) without them.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};

use crate::whisper::Part;

type Status = i32; // ov_status_e, 0 = OK
type Ptr = *mut c_void;

/// The few functions of openvino/genai/c/whisper_pipeline.h and whisper_generation_config.h we use.
struct Api {
    _lib: Library,
    pipeline_create: unsafe extern "C" fn(*const c_char, *const c_char, usize, *mut Ptr, ...) -> Status,
    pipeline_free: unsafe extern "C" fn(Ptr),
    pipeline_generate: unsafe extern "C" fn(Ptr, *const f32, usize, Ptr, *mut Ptr) -> Status,
    config_create: unsafe extern "C" fn(*mut Ptr) -> Status,
    config_free: unsafe extern "C" fn(Ptr),
    config_set_language: unsafe extern "C" fn(Ptr, *const c_char) -> Status,
    config_set_task: unsafe extern "C" fn(Ptr, *const c_char) -> Status,
    config_set_return_timestamps: unsafe extern "C" fn(Ptr, bool) -> Status,
    results_free: unsafe extern "C" fn(Ptr),
    results_texts_count: unsafe extern "C" fn(Ptr, *mut usize) -> Status,
    results_text_at: unsafe extern "C" fn(Ptr, usize, *mut c_char, *mut usize) -> Status,
    results_chunks_count: unsafe extern "C" fn(Ptr, *mut usize) -> Status,
    results_chunk_at: unsafe extern "C" fn(Ptr, usize, *mut Ptr) -> Status,
    chunk_free: unsafe extern "C" fn(Ptr),
    chunk_start: unsafe extern "C" fn(Ptr, *mut f32) -> Status,
    chunk_end: unsafe extern "C" fn(Ptr, *mut f32) -> Status,
    chunk_text: unsafe extern "C" fn(Ptr, *mut c_char, *mut usize) -> Status,
}

fn check(status: Status, what: &str) -> Result<()> {
    if status != 0 {
        bail!("OpenVINO {what} failed (status {status})");
    }
    Ok(())
}

/// Folder with openvino_genai_c.dll and its dependencies: `openvino\` next to the exe, or TRANSCRIBE_OPENVINO_DIR.
pub fn dll_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TRANSCRIBE_OPENVINO_DIR") {
        return PathBuf::from(d);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("openvino")
}

impl Api {
    fn load() -> Result<Api> {
        let path = dll_dir().join("openvino_genai_c.dll");
        anyhow::ensure!(path.exists(), "{} not found", path.display());
        // Altered search path: the DLL's own dependencies (openvino.dll, tbb12.dll, …) come from its folder.
        let lib = unsafe { Library::load_with_flags(&path, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .with_context(|| format!("loading {}", path.display()))?;
        unsafe {
            macro_rules! f {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).context($name)?
                };
            }
            Ok(Api {
                pipeline_create: f!("ov_genai_whisper_pipeline_create"),
                pipeline_free: f!("ov_genai_whisper_pipeline_free"),
                pipeline_generate: f!("ov_genai_whisper_pipeline_generate"),
                config_create: f!("ov_genai_whisper_generation_config_create"),
                config_free: f!("ov_genai_whisper_generation_config_free"),
                config_set_language: f!("ov_genai_whisper_generation_config_set_language"),
                config_set_task: f!("ov_genai_whisper_generation_config_set_task"),
                config_set_return_timestamps: f!("ov_genai_whisper_generation_config_set_return_timestamps"),
                results_free: f!("ov_genai_whisper_decoded_results_free"),
                results_texts_count: f!("ov_genai_whisper_decoded_results_get_texts_count"),
                results_text_at: f!("ov_genai_whisper_decoded_results_get_text_at"),
                results_chunks_count: f!("ov_genai_whisper_decoded_results_get_chunks_count"),
                results_chunk_at: f!("ov_genai_whisper_decoded_results_get_chunk_at"),
                chunk_free: f!("ov_genai_whisper_decoded_result_chunk_free"),
                chunk_start: f!("ov_genai_whisper_decoded_result_chunk_get_start_ts"),
                chunk_end: f!("ov_genai_whisper_decoded_result_chunk_get_end_ts"),
                chunk_text: f!("ov_genai_whisper_decoded_result_chunk_get_text"),
                _lib: lib,
            })
        }
    }

    /// The C API's two-call pattern for strings: ask for the size, then fill the buffer.
    fn string(&self, get: impl Fn(*mut c_char, *mut usize) -> Status) -> String {
        let mut size = 0usize;
        if get(std::ptr::null_mut(), &mut size) != 0 || size == 0 {
            return String::new();
        }
        let mut buf = vec![0u8; size + 1];
        if get(buf.as_mut_ptr() as *mut c_char, &mut size) != 0 {
            return String::new();
        }
        CStr::from_bytes_until_nul(&buf).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// The model folder: an existing Hugging Face cache download if present, else our own download.
fn model_dir(model: &str) -> Result<PathBuf> {
    let repo = if model.contains('/') { model.to_string() } else { format!("OpenVINO/whisper-{model}-fp16-ov") };
    let hf = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", repo.replace('/', "--")))
        .join("snapshots");
    if let Ok(entries) = std::fs::read_dir(&hf) {
        for e in entries.flatten() {
            if e.path().join("openvino_encoder_model.xml").exists() {
                return Ok(e.path());
            }
        }
    }
    // Download all files of the repository (the Hugging Face API lists them).
    let local = repo.replace('/', "--");
    let dir = crate::config::model_dir().join(&local);
    if dir.join("openvino_encoder_model.xml").exists() && dir.join("openvino_decoder_model.bin").exists() {
        return Ok(dir);
    }
    let listing: serde_json::Value = serde_json::from_reader(
        ureq::get(&format!("https://huggingface.co/api/models/{repo}")).call()?.body_mut().as_reader(),
    )?;
    let files: Vec<String> = listing["siblings"]
        .as_array()
        .context("model listing")?
        .iter()
        .filter_map(|s| s["rfilename"].as_str().map(str::to_string))
        .filter(|f| !f.starts_with('.') && !f.ends_with(".md"))
        .collect();
    let count = files.len();
    for (i, f) in files.into_iter().enumerate() {
        crate::models::set_progress(format!("Checking speech model files ({} of {count})…", i + 1), None);
        crate::models::ensure(&format!("{local}/{f}"), &format!("https://huggingface.co/{repo}/resolve/main/{f}"))?;
    }
    Ok(dir)
}

pub struct OpenVinoWhisper {
    api: Api,
    pipeline: Ptr,
}

// The pipeline is only ever used from the transcriber thread.
unsafe impl Send for OpenVinoWhisper {}

impl OpenVinoWhisper {
    pub fn load(model: &str, device: &str) -> Result<OpenVinoWhisper> {
        let api = Api::load()?;
        let dir = model_dir(model)?;
        // Compiled GPU kernels: first start ~30 s, later ~2 s.
        let cache = crate::config::model_dir().join("ov-cache");
        std::fs::create_dir_all(&cache)?;
        crate::models::set_progress(
            format!("Preparing speech recognition on the {device} (the first start can take a minute)…"),
            None,
        );
        let (path, dev) = (CString::new(dir.to_string_lossy().as_bytes())?, CString::new(device)?);
        let (key, value) = (CString::new("CACHE_DIR")?, CString::new(cache.to_string_lossy().as_bytes())?);
        let mut pipeline: Ptr = std::ptr::null_mut();
        check(
            unsafe { (api.pipeline_create)(path.as_ptr(), dev.as_ptr(), 2, &mut pipeline, key.as_ptr(), value.as_ptr()) },
            "pipeline",
        )?;
        Ok(OpenVinoWhisper { api, pipeline })
    }

    pub fn transcribe(&mut self, audio: &[f32], language: Option<&str>) -> Result<Vec<Part>> {
        let a = &self.api;
        let mut config: Ptr = std::ptr::null_mut();
        check(unsafe { (a.config_create)(&mut config) }, "config")?;
        let task = CString::new("transcribe")?;
        let lang = language.map(|l| CString::new(format!("<|{l}|>"))).transpose()?;
        let result = unsafe {
            check((a.config_set_task)(config, task.as_ptr()), "task")
                .and_then(|_| check((a.config_set_return_timestamps)(config, true), "timestamps"))
                .and_then(|_| match &lang {
                    Some(l) => check((a.config_set_language)(config, l.as_ptr()), "language"),
                    None => Ok(()),
                })
                .and_then(|_| {
                    let mut results: Ptr = std::ptr::null_mut();
                    check((a.pipeline_generate)(self.pipeline, audio.as_ptr(), audio.len(), config, &mut results), "generate")?;
                    let parts = self.collect(results);
                    (a.results_free)(results);
                    Ok(parts)
                })
        };
        unsafe { (a.config_free)(config) };
        result
    }

    fn collect(&self, results: Ptr) -> Vec<Part> {
        let a = &self.api;
        let mut parts = Vec::new();
        let mut count = 0usize;
        unsafe {
            if (a.results_chunks_count)(results, &mut count) == 0 {
                for i in 0..count {
                    let mut chunk: Ptr = std::ptr::null_mut();
                    if (a.results_chunk_at)(results, i, &mut chunk) != 0 {
                        continue;
                    }
                    let (mut start, mut end) = (0f32, 0f32);
                    (a.chunk_start)(chunk, &mut start);
                    (a.chunk_end)(chunk, &mut end);
                    let text = a.string(|buf, size| (a.chunk_text)(chunk, buf, size)).trim().to_string();
                    (a.chunk_free)(chunk);
                    if !text.is_empty() {
                        parts.push((start.max(0.0), end.max(start), text));
                    }
                }
            }
            if parts.is_empty() {
                let mut texts = 0usize;
                if (a.results_texts_count)(results, &mut texts) == 0 && texts > 0 {
                    let text = a.string(|buf, size| (a.results_text_at)(results, 0, buf, size)).trim().to_string();
                    if !text.is_empty() {
                        parts.push((0.0, f32::MAX, text));
                    }
                }
            }
        }
        parts
    }
}

impl Drop for OpenVinoWhisper {
    fn drop(&mut self) {
        unsafe { (self.api.pipeline_free)(self.pipeline) };
    }
}
