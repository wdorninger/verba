# Getting started

[README](../README.md) · **Getting started** · [Usage](usage.md) · [Configuration](configuration.md) ·
[Summaries and answers](summaries.md) · [Privacy](privacy.md) · [Development](development.md)

## Requirements

- Windows x64. Verba is developed and tested on Windows 11.
- For GPU speech recognition: an Intel GPU with the Intel graphics driver installed. The driver provides
  `OpenCL.dll`, the only library the bundled OpenVINO runtime does not include. Without it, Verba runs on the
  CPU.
- Internet access on the first start, to download the models (about 0.5 GB).
- Optional: the Claude Code CLI, for summaries and live answers. See [Summaries and answers](summaries.md).

## Installation

1. Download `verba-<version>-windows-x64.zip` from the
   [latest release](https://github.com/wdorninger/verba/releases/latest).
2. Unzip it to any folder you can write to.
3. Start `verba\verba.exe`.

There is no installer. To remove Verba, delete the folder and, optionally, the model cache
`%LOCALAPPDATA%\transcribe-models`.

When upgrading, copy `config.toml` and `speakers.json` from the old folder into the new one to keep your
settings and named voices.

## First start

On the first start Verba downloads the Whisper, voice-activity and speaker models to
`%LOCALAPPDATA%\transcribe-models`. The live window shows the download and loading progress next to the
status. An OpenVINO Whisper model that is already in the Hugging Face cache (`%USERPROFILE%\.cache\huggingface`)
is used instead of downloading it again.

The GPU engine compiles its kernels on first use and caches them in
`%LOCALAPPDATA%\transcribe-models\ov-cache`, so later starts are fast.

Once the models are loaded the tray icon turns green and recording starts. Double-click the icon to open the
live window.

## Folder layout

| Path | Contents |
|---|---|
| `verba.exe` | The application. whisper.cpp and the C runtime are linked in. |
| `sherpa-onnx-c-api.dll`, `onnxruntime.dll` | Speaker recognition (sherpa-onnx) |
| `openvino\` | GPU speech recognition (OpenVINO GenAI) and the Visual C++ runtime it needs. Without this folder Verba uses the CPU. |
| `config.toml` | Settings, created on first start |
| `speakers.json` | Voiceprints and names of known speakers |
| `transcripts\` | One folder per transcript, with its screenshots, summary and answers |
| `logs\` | `transcribe.log` |

A transcript folder looks like this:

```
transcripts\2026-10-17_14-00-05_Teams\
    2026-10-17_14-00-05_Teams.txt            transcript
    2026-10-17_14-00-05_Teams_summary.md     summary, if requested
    2026-10-17_14-00-05_Teams_answers.md     answered questions, if any
    screenshot_14-03-40.png
```

Transcript lines are plain text:

```
[14:03:12] [Hanna] Can you hear me?
[14:03:15] [Speaker 2] Yes, loud and clear.
[14:03:40] [Screenshot] screenshot_14-03-40.png
```

## Command-line options

| Option | Purpose |
|---|---|
| `verba.exe --console` | Show the log in a console window while the app runs |
| `verba.exe --transcribe file.wav` | Transcribe a 16 kHz mono WAV file and print the result and timings. Useful to check an installation without a call. |
| `verba.exe --feed a.wav b.wav ...` | Play 16 kHz mono WAV files through the full pipeline as speaker audio, to try the window, speaker recognition and summaries without a call |

Only one instance runs at a time; starting a second one exits immediately.

## Starting with Windows

Press Win+R, enter `shell:startup` and place a shortcut to `verba.exe` in the folder that opens.
