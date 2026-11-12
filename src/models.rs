//! Downloads model files once into %LOCALAPPDATA%\transcribe-models.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn ensure(file_name: &str, url: &str) -> Result<PathBuf> {
    let dir = crate::config::model_dir();
    let path = dir.join(file_name);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    log::info!("Downloading {file_name} ...");
    let mut response = ureq::get(url).call().with_context(|| format!("download {url}"))?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let tmp = path.with_extension("part");
    let mut file = std::fs::File::create(&tmp)?;
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
    std::fs::rename(&tmp, &path)?;
    log::info!("Downloaded {file_name}");
    Ok(path)
}
