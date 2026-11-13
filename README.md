<p align="center">
  <img src="docs/icon.svg" width="112" height="112" alt="Verba Transcribe icon">
</p>

<h1 align="center">Verba Transcribe</h1>

<p align="center">
  Private, on-device call transcription for Windows.
</p>

<p align="center">
  <a href="https://github.com/wdorninger/verba/releases/latest"><img src="https://img.shields.io/github/v/release/wdorninger/verba?label=release&color=2f6fd6" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20x64-2f6fd6" alt="Windows x64">
  <img src="https://img.shields.io/badge/built%20with-Rust-555" alt="Built with Rust">
  <img src="https://img.shields.io/badge/speech-Whisper%20%C2%B7%20OpenVINO-555" alt="Whisper on OpenVINO">
</p>

<p align="center">
  <a href="docs/getting-started.md">Getting started</a> ·
  <a href="docs/usage.md">Usage</a> ·
  <a href="docs/configuration.md">Configuration</a> ·
  <a href="docs/summaries.md">Summaries and answers</a> ·
  <a href="docs/development.md">Development</a>
</p>

---

Verba sits in the system tray and writes down what is said in your calls: your microphone and everything your
speakers play, from Teams, Zoom, Meet or a browser, with no meeting bot or plugin. It tells voices apart, keeps
one plain-text transcript per call and can take screenshots along the way.

Speech recognition runs locally on the Intel GPU, with a CPU fallback, and audio never leaves the computer.
Summaries and live answers to questions asked in the call are optional; they use the Claude Code CLI and stay
off until you switch them on.

## Features

- **Records both sides** of any call, or any audio playing on the PC, at the same time.
- **Fast local Whisper**: OpenVINO GenAI on the Intel GPU (about 1.3 s per 10 s of speech), whisper.cpp on the
  CPU as fallback. A badge shows which engine is active.
- **Speaker recognition**: voices are told apart and labelled. Rename a speaker once and every line of that voice
  follows, including in later calls.
- **One transcript per call** in *Only calls* mode, started and stopped by your call app's use of the
  microphone.
- **Screenshots** on demand or whenever the screen changes noticeably, linked from the transcript.
- **Optional summaries and live answers**: decisions, action items and open questions after the call; answers to
  questions while it is still running.
- **Windows 11 look** with light and dark mode, search, zoom and always-on-top.
- **Portable**: one folder, no installer. Models download on first start.

## Screenshots

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/answers.png" alt="Live transcript with the answers panel">
      <p><b>Live transcript and answers.</b> A question from the call, answered in the side panel.</p>
    </td>
    <td width="50%" valign="top">
      <img src="docs/summary.png" alt="Summary window with decisions, action items and open questions">
      <p><b>Summary.</b> Saved as Markdown next to the transcript.</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/rename.png" alt="A speaker name being edited inline">
      <p><b>Renaming a speaker.</b> Earlier and later lines of that voice take the new name.</p>
    </td>
    <td width="50%" valign="top">
      <img src="docs/settings.png" alt="Settings dialog">
      <p><b>Settings.</b> Stored in a commented <code>config.toml</code>.</p>
    </td>
  </tr>
</table>

<sub>The screenshots were made with synthetic text-to-speech voices. No real call or person is shown.</sub>

## Quick start

1. Download `verba-<version>-windows-x64.zip` from the [latest release](https://github.com/wdorninger/verba/releases/latest).
2. Unzip it anywhere and start `verba\verba.exe`. The first start downloads the speech models (about 0.5 GB).
3. Double-click the tray icon to open the live window.

Transcripts are written to `transcripts\` next to the executable. Requirements, the first start and
command-line options are covered in [Getting started](docs/getting-started.md).

> [!NOTE]
> Make sure everyone you record has agreed to it.

## Documentation

| Page | Contents |
|---|---|
| [Getting started](docs/getting-started.md) | Requirements, installation, first start, folder layout, command-line options |
| [Usage](docs/usage.md) | Live window, record modes, speakers, screenshots, tray icon |
| [Configuration](docs/configuration.md) | Settings, `config.toml`, speech recognition engines, tuning |
| [Summaries and answers](docs/summaries.md) | Setting up the Claude Code CLI, what is sent and when |
| [Privacy](docs/privacy.md) | What stays on the computer and what does not |
| [Development](docs/development.md) | Building from source, tests, project layout |
