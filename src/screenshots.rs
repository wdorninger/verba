//! Screenshots (xcap): on demand from the window or tray, and automatically while recording whenever enough
//! of the screen has changed. Saved in the current transcript's folder and referenced in the transcript.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Local;
use image::{GrayImage, RgbaImage, imageops};
use xcap::Monitor;

use crate::app::{Core, State, UiEvent};

const THUMB_WIDTH: u32 = 192; // change detection works on a small greyscale copy
const PIXEL_DELTA: i16 = 24; // grey-level difference that counts a thumbnail pixel as changed

/// Choices for the "screenshots.monitor" setting: ("primary" | "all" | "1", "2", …, label).
pub fn monitor_choices() -> Vec<(String, String)> {
    let mut out = vec![("primary".to_string(), "Primary screen".to_string()), ("all".to_string(), "All screens".to_string())];
    if let Ok(monitors) = Monitor::all() {
        for (i, m) in monitors.iter().enumerate() {
            let name = m.friendly_name().or_else(|_| m.name()).unwrap_or_default();
            let size = format!("{}×{}", m.width().unwrap_or(0), m.height().unwrap_or(0));
            out.push(((i + 1).to_string(), format!("Screen {}: {name} ({size})", i + 1)));
        }
    }
    out
}

/// The monitors the setting refers to, with a short tag for file names when there are several.
fn selected(setting: &str) -> Result<Vec<(String, Monitor)>> {
    let all = Monitor::all().context("listing screens")?;
    let picked: Vec<(String, Monitor)> = match setting {
        "all" => all.into_iter().enumerate().map(|(i, m)| (format!("_screen{}", i + 1), m)).collect(),
        "primary" | "" => all.into_iter().filter(|m| m.is_primary().unwrap_or(false)).map(|m| (String::new(), m)).collect(),
        n => {
            let i: usize = n.parse().unwrap_or(1);
            all.into_iter().nth(i.saturating_sub(1)).map(|m| (String::new(), m)).into_iter().collect()
        }
    };
    if picked.is_empty() {
        // The configured screen is gone (e.g. undocked): fall back to the primary one.
        let primary = Monitor::all()?.into_iter().find(|m| m.is_primary().unwrap_or(false)).context("no screen found")?;
        return Ok(vec![(String::new(), primary)]);
    }
    Ok(picked)
}

fn thumbnail(img: &RgbaImage) -> GrayImage {
    let h = (img.height() as u64 * THUMB_WIDTH as u64 / img.width().max(1) as u64).max(1) as u32;
    imageops::grayscale(&imageops::thumbnail(img, THUMB_WIDTH, h))
}

/// Share of thumbnail pixels that changed, 0–100.
fn changed_percent(a: &GrayImage, b: &GrayImage) -> f32 {
    if a.dimensions() != b.dimensions() {
        return 100.0;
    }
    let changed = a.pixels().zip(b.pixels()).filter(|(p, q)| (p.0[0] as i16 - q.0[0] as i16).abs() > PIXEL_DELTA).count();
    changed as f32 * 100.0 / (a.width() * a.height()).max(1) as f32
}

/// Saves the image in the current transcript's folder and notes it in the transcript and the window.
fn save(core: &Core, img: &RgbaImage, tag: &str, reason: &str) -> Result<PathBuf> {
    let folder = core.writer.folder().context("creating the transcript folder")?;
    let now = Local::now();
    let path = folder.join(format!("screenshot_{}{tag}.png", now.format("%H-%M-%S")));
    img.save(&path).with_context(|| format!("saving {}", path.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let time = now.format("%H:%M:%S").to_string();
    core.writer.append(&format!("[{time}] [Screenshot] {name}"));
    log::info!("Screenshot ({reason}): {}", path.display());
    core.ui.send(UiEvent::Screenshot { time, path: path.clone() });
    Ok(path)
}

/// Takes a screenshot now (button or tray), off the UI thread.
pub fn capture_now(core: Arc<Core>) {
    std::thread::Builder::new()
        .name("screenshot".into())
        .spawn(move || {
            let setting = core.cfg.lock().unwrap().str("screenshots", "monitor");
            let result = selected(&setting).and_then(|monitors| {
                for (tag, m) in monitors {
                    let img = m.capture_image().context("capturing the screen")?;
                    save(&core, &img, &tag, "button")?;
                }
                Ok(())
            });
            if let Err(e) = result {
                log::error!("Screenshot failed: {e:#}");
                core.ui.send(UiEvent::Note(format!("Screenshot failed: {e}")));
            }
        })
        .expect("spawn screenshot thread");
}

/// Auto screenshots: while recording, checks the screen every few seconds and saves it when at least
/// `change_percent` of it differs from the last saved screenshot.
pub fn run_auto(core: Arc<Core>) {
    let mut last: HashMap<String, GrayImage> = HashMap::new();
    let mut last_setting = String::new();
    let mut next = Instant::now();
    while !core.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        if Instant::now() < next {
            continue;
        }
        let (auto, setting, percent, every) = {
            let c = core.cfg.lock().unwrap();
            (
                c.bool("screenshots", "auto"),
                c.str("screenshots", "monitor"),
                c.f64("screenshots", "change_percent") as f32,
                c.f64("screenshots", "check_seconds").max(0.5),
            )
        };
        next = Instant::now() + Duration::from_secs_f64(every);
        if !auto || core.state() != State::Recording {
            last.clear(); // the first check after (re)starting always saves a screenshot
            continue;
        }
        if setting != last_setting {
            last.clear();
            last_setting = setting.clone();
        }
        let monitors = match selected(&setting) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("Auto screenshot: {e:#}");
                continue;
            }
        };
        for (tag, m) in monitors {
            let img = match m.capture_image() {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("Auto screenshot capture failed: {e}");
                    continue;
                }
            };
            let thumb = thumbnail(&img);
            let change = last.get(&tag).map_or(100.0, |prev| changed_percent(prev, &thumb));
            if change >= percent {
                match save(&core, &img, &tag, &format!("{change:.0}% changed")) {
                    Ok(_) => {
                        last.insert(tag, thumb);
                    }
                    Err(e) => log::error!("Auto screenshot failed: {e:#}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_detection() {
        let a = GrayImage::from_pixel(10, 10, image::Luma([100]));
        let mut b = a.clone();
        assert_eq!(changed_percent(&a, &b), 0.0);
        for x in 0..10 {
            for y in 0..3 {
                b.put_pixel(x, y, image::Luma([200]));
            }
        }
        assert_eq!(changed_percent(&a, &b), 30.0);
        b.put_pixel(9, 9, image::Luma([110])); // small differences (noise, video compression) don't count
        assert_eq!(changed_percent(&a, &b), 30.0);
    }

    /// Lists the screens and times one capture plus change check (nothing is saved).
    #[test]
    #[ignore]
    fn capture_speed() {
        for (k, l) in monitor_choices() {
            println!("{k}: {l}");
        }
        for (tag, m) in selected("all").unwrap() {
            let t = Instant::now();
            let img = m.capture_image().unwrap();
            let thumb = thumbnail(&img);
            println!("screen{tag}: {}x{} captured + thumbnail in {} ms", img.width(), img.height(), t.elapsed().as_millis());
            assert_eq!(changed_percent(&thumb, &thumb), 0.0);
        }
    }
}
