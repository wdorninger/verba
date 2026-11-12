//! WASAPI capture of the default microphone and the default speaker (loopback), on one audio thread
//! that also reopens the streams when Windows' default devices change or the mic stream dies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::app::Core;
use crate::segmenter::{AudioBlock, Source};

const WATCH_INTERVAL: Duration = Duration::from_secs(3); // between default-device checks
const MIC_STALL: Duration = Duration::from_secs(5); // the mic delivers blocks even in silence

pub enum Command {
    Restart,
    Stop,
}

#[derive(Clone)]
pub struct CaptureHandle {
    tx: Sender<Command>,
}

impl CaptureHandle {
    pub fn restart(&self) {
        let _ = self.tx.send(Command::Restart);
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}

pub fn channel() -> (CaptureHandle, Receiver<Command>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    (CaptureHandle { tx }, rx)
}

struct Streams {
    streams: Vec<Stream>,
    devices: HashMap<Source, String>,
}

fn device_id(d: &cpal::Device) -> String {
    d.id().map(|id| format!("{id:?}")).unwrap_or_default()
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|d| d.name().to_string()).unwrap_or_else(|_| device_id(d))
}

fn default_ids(host: &cpal::Host) -> (String, String) {
    (
        host.default_output_device().map(|d| device_id(&d)).unwrap_or_default(),
        host.default_input_device().map(|d| device_id(&d)).unwrap_or_default(),
    )
}

/// Milliseconds since `epoch` at which the mic last delivered a block.
struct MicClock {
    epoch: Instant,
    last: AtomicU64,
}

impl MicClock {
    fn touch(&self) {
        self.last.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    fn since(&self) -> Duration {
        let last = Duration::from_millis(self.last.load(Ordering::Relaxed));
        self.epoch.elapsed().saturating_sub(last)
    }
}

fn open(
    core: &Arc<Core>,
    source: Source,
    device: &cpal::Device,
    loopback: bool,
    tx: Sender<AudioBlock>,
    mic: Arc<MicClock>,
) -> anyhow::Result<Stream> {
    let supported = if loopback { device.default_output_config()? } else { device.default_input_config()? };
    let rate = supported.sample_rate();
    let channels = supported.channels().max(1) as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let paused = core.paused.clone();

    let deliver = move |mono: Vec<f32>| {
        if source == Source::Me {
            mic.touch();
        }
        if !paused.load(Ordering::Relaxed) {
            let _ = tx.send((rate, mono));
        }
    };
    let err = move |e: cpal::Error| match e.kind() {
        cpal::ErrorKind::Xrun => log::debug!("Audio buffer overrun ({})", source.id()), // harmless, frequent
        _ => log::warn!("Audio stream error ({}): {e}", source.id()),
    };

    fn mono<T: Copy>(data: &[T], channels: usize, conv: impl Fn(T) -> f32) -> Vec<f32> {
        data.chunks(channels).map(|f| f.iter().map(|&s| conv(s)).sum::<f32>() / f.len() as f32).collect()
    }

    let stream = match format {
        SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| deliver(mono(data, channels, |s| s)),
            err,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            config,
            move |data, _| deliver(mono(data, channels, |s| s as f32 / 32768.0)),
            err,
            None,
        )?,
        SampleFormat::I32 => device.build_input_stream::<i32, _, _>(
            config,
            move |data, _| deliver(mono(data, channels, |s| s as f32 / 2147483648.0)),
            err,
            None,
        )?,
        other => anyhow::bail!("unsupported sample format {other:?}"),
    };
    stream.play()?;
    log::info!("Capturing {} from '{}' ({} Hz, {} ch)", source.id(), device_name(device), rate, channels);
    Ok(stream)
}

fn start(core: &Arc<Core>, host: &cpal::Host, senders: &HashMap<Source, Sender<AudioBlock>>, mic: &Arc<MicClock>) -> Streams {
    let mut s = Streams { streams: Vec::new(), devices: HashMap::new() };
    for (&source, tx) in senders {
        let (device, loopback) = match source {
            Source::Me => (host.default_input_device(), false),
            Source::Them => (host.default_output_device(), true),
        };
        let Some(device) = device else {
            log::warn!("No default {} device", if loopback { "output" } else { "input" });
            continue;
        };
        match open(core, source, &device, loopback, tx.clone(), mic.clone()) {
            Ok(stream) => {
                s.streams.push(stream);
                s.devices.insert(source, device_name(&device));
            }
            Err(e) => log::error!(
                "Could not open {}: {e:#}",
                if loopback { "speaker loopback" } else { "microphone" }
            ),
        }
    }
    s
}

pub fn run(core: Arc<Core>, senders: HashMap<Source, Sender<AudioBlock>>, commands: Receiver<Command>) {
    let host = cpal::default_host();
    let mic = Arc::new(MicClock { epoch: Instant::now(), last: AtomicU64::new(0) });
    mic.touch();
    let mut streams = start(&core, &host, &senders, &mic);
    let mut current = default_ids(&host);
    loop {
        let reason = match commands.recv_timeout(WATCH_INTERVAL) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Command::Restart) => Some("requested"),
            Err(RecvTimeoutError::Timeout) => {
                let ids = default_ids(&host);
                if ids != current {
                    Some("default audio device changed")
                } else if senders.contains_key(&Source::Me)
                    && streams.devices.contains_key(&Source::Me)
                    && mic.since() > MIC_STALL
                {
                    Some("microphone stream stalled")
                } else {
                    None
                }
            }
        };
        if let Some(reason) = reason {
            log::info!("Restarting audio: {reason}");
            drop(std::mem::replace(&mut streams, Streams { streams: Vec::new(), devices: HashMap::new() }));
            current = default_ids(&host);
            mic.touch();
            streams = start(&core, &host, &senders, &mic);
        }
    }
    drop(streams);
}
