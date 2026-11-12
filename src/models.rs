//! Downloads model files once into %LOCALAPPDATA%\transcribe-models.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
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
