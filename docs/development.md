# Development

[README](../README.md) · [Getting started](getting-started.md) · [Usage](usage.md) ·
[Configuration](configuration.md) · [Summaries and answers](summaries.md) · [Privacy](privacy.md) ·
**Development**

## Prerequisites

- Rust with the MSVC toolchain
- Visual Studio C++ build tools
- CMake

Run cargo from PowerShell or a Developer Command Prompt. In Git Bash, GNU `link` shadows the MSVC linker and
linking fails.

## Building

```powershell
powershell -File setup-openvino.ps1   # once per machine: OpenVINO GenAI runtime for the GPU engine
powershell -File package.ps1          # release build, assembled in dist\verba plus a release zip
cargo test --release                  # unit tests
```

`package.ps1` builds the release, copies the executable, the sherpa-onnx and ONNX Runtime DLLs, the `openvino\`
folder and the documentation to `dist\verba\`, and creates `dist\verba-<version>-windows-x64.zip`.

`build.rs` copies the OpenVINO runtime installed by `setup-openvino.ps1` and the Visual C++ runtime into
`target\<profile>\openvino`, so every build can use the GPU engine, not only the packaged one.

### Notes

- `vendor\whisper-rs-sys` is the sys crate of whisper-rs with its bundled bindings fixed for Windows, so the
  build does not need libclang (see `vendor\whisper-rs-sys\PATCHED.md`).
- `.cargo\config.toml` passes optimisation and AVX2 flags to the whisper.cpp CMake build. Without them the CPU
  engine is about 50 times slower.
- The sherpa-onnx build script downloads its prebuilt release libraries.
- After moving the project folder, build-script outputs in `target\` still point to the old location
  (`LNK1181: cannot open input file 'sherpa-onnx-c-api.lib'`). Run `cargo clean --release -p sherpa-onnx-sys`.

## Testing without a call

- `verba.exe --transcribe file.wav` runs the engine and speaker recognition on one 16 kHz mono WAV and prints
  timings.
- `verba.exe --feed a.wav b.wav ...` sends WAV files through the full pipeline as speaker audio.
- `docs\make-demo-audio.ps1` generates a synthetic meeting with text-to-speech voices. It was used for the
  screenshots in the README and works with `--feed`.

Run test copies in a separate folder whose `config.toml` sets `[capture] mic = false`, so the room microphone is
not recorded. Only one instance can run at a time, so close a running copy first.

## RustRover

Open the project folder; RustRover detects the Cargo project. Shared run configurations are in `.run\`:

| Configuration | Runs |
|---|---|
| Run (console) | `cargo run --release -- --console`, with data in `dev-home\` and the GPU libraries from `dist\verba\openvino` (run *Package* once first) |
| Tests | `cargo test --release` |
| Package (dist) | `package.ps1` |
| Setup OpenVINO (once) | `setup-openvino.ps1` |

## Project layout

| File | Responsibility |
|---|---|
| `main.rs` | Start-up, threads, single instance, shutdown, `--transcribe` and `--feed` |
| `app.rs` | Shared state, status, settings, actions |
| `capture.rs` | WASAPI microphone and loopback capture (cpal), device changes, stall recovery |
| `segmenter.rs` | Energy-based voice activity detection that cuts speech into chunks |
| `engine.rs`, `openvino.rs`, `whisper.rs` | Whisper on the GPU (OpenVINO GenAI) and CPU (whisper.cpp) |
| `transcriber.rs` | Chunk to text to speaker turns to transcript file |
| `speakers.rs` | Voiceprints (sherpa-onnx), naming, merging, `speakers.json` |
| `callwatch.rs` | Call detection from Windows microphone usage |
| `claude.rs`, `answers.rs` | Summaries and live answers through the Claude Code CLI |
| `screenshots.rs` | Manual and change-triggered screenshots |
| `ui.rs`, `theme.rs`, `markdown.rs`, `tray.rs` | egui window, Windows 11 styling, tray icon |
| `config.rs`, `default_config.toml` | Settings, written back without losing comments |

`docs\icon.svg` matches the application icon drawn in `theme::mic_icon`.
