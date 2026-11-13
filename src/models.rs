//! Downloads model files once into %LOCALAPPDATA%\transcribe-models.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// What the start-up is doing right now (download, loading, compiling), shown in the window and tray
/// while the model loads: text and, for downloads, the fraction done.
static PROGRESS: std::sync::Mutex<Option<(String, Option<f32>)>> = std::sync::Mutex::new(None);

pub fn set_progress(text: impl Into<String>, fraction: Option<f32>) {
    *PROGRESS.lock().unwrap() = Some((text.into(), fraction));
}

pub fn clear_progress() {
    *PROGRESS.lock().unwrap() = None;
}

pub fn progress() -> Option<(String, Option<f32>)> {
    PROGRESS.lock().unwrap().clone()
}

/// Creates the folder a model file goes into. File names may contain subfolders
/// (`OpenVINO--whisper-small-fp16-ov/openvino_encoder_model.xml`), so this is the file's own folder.
fn prepare(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create folder {}", parent.display()))?;
    }
    Ok(())
}

pub fn ensure(file_name: &str, url: &str) -> Result<PathBuf> {
    let path = crate::config::model_dir().join(file_name);
    if path.exists() {
        return Ok(path);
    }
    prepare(&path)?;
    log::info!("Downloading {file_name} ...");
    let mut response = ureq::get(url).call().with_context(|| format!("download {url}"))?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let tmp = path.with_extension("part");
    let mut file = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut next_report) = (0u64, 10u64);
    // Shown in the window while loading; the caller's own progress text comes back afterwards.
    let previous = progress();
    let short = file_name.rsplit('/').next().unwrap_or(file_name).to_string();
    let show = |done: u64| match total {
        Some(t) if t > 1_000_000 => set_progress(
            format!("Downloading {short}: {} of {} MB", done / 1_000_000, t / 1_000_000),
            Some(done as f32 / t as f32),
        ),
        _ => set_progress(format!("Downloading {short}…"), None),
    };
    show(0);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        show(done);
        if let Some(total) = total.filter(|t| *t > 10_000_000) {
            let pct = done * 100 / total.max(1);
            if pct >= next_report {
                log::info!("  {file_name}: {pct}% of {} MB", total / 1_000_000);
                next_report = pct / 10 * 10 + 10;
            }
        }
    }
    file.flush()?;
    drop(file);
    if let Some(total) = total {
        anyhow::ensure!(done == total, "download of {file_name} incomplete ({done} of {total} bytes)");
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("move {} into place", tmp.display()))?;
    log::info!("Downloaded {file_name}");
    match previous {
        Some((text, fraction)) => set_progress(text, fraction),
        None => clear_progress(),
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_creates_nested_model_folders() {
        let base = std::env::temp_dir().join(format!("transcribe-models-test-{}", std::process::id()));
        let file = base.join("OpenVINO--whisper-small-fp16-ov").join("openvino_encoder_model.xml");
        prepare(&file).unwrap();
        assert!(file.parent().unwrap().is_dir());
        std::fs::write(file.with_extension("part"), b"x").unwrap(); // what ensure() writes first
        let _ = std::fs::remove_dir_all(&base);
    }
}
