# Usage

[README](../README.md) · [Getting started](getting-started.md) · **Usage** · [Configuration](configuration.md) ·
[Summaries and answers](summaries.md) · [Privacy](privacy.md) · [Development](development.md)

## Tray icon

| Colour | State |
|---|---|
| Orange | Loading models |
| Green | Recording |
| Blue | Waiting for a call (*Only calls* mode) |
| Grey | Paused |
| Red | Error; see the tooltip or `logs\transcribe.log` |

Double-click the icon to open the live window. The tray menu has *Show live window*, *Pause*,
*Only during calls*, *Take screenshot*, *New transcript*, *Open transcript*, *Open transcripts folder*,
*Restart audio devices* and *Quit*. The tooltip shows the active speech recognition engine.

## Live window

Closing the window hides it to the tray, and a notification says that Verba is still running. Quit from the
tray menu, or set `[ui] close_to_tray = false` to quit when the window is closed.

**Top row**: status, the engine badge, *Pause/Resume*, Settings and a menu with *Always on top*,
*Open transcript*, *Open folder*, *Copy* and *Clear*.

The engine badge is green for **GPU · OpenVINO** and orange for **CPU · whisper.cpp**. If the GPU engine could
not start, the badge says *fallback*; hover over it to see why.

**Second row**: language, record mode, search (Enter jumps to the next match), screenshot, and text size
(A− / A+ or Ctrl + mouse wheel).

**Bottom row**: the **Claude** switch, *Answer questions*, *Summarize*, *New transcript*, *Copy all* and *Clear*.
*New transcript* starts a new file and folder and clears both the window and the context used for answers.

Light and dark mode follow Windows unless `[ui] theme` is set.

## Language

English is the default. Choose another language (German, French, Spanish, Italian and others) or automatic
detection in the window. Automatic detection is free on the GPU engine but doubles the cost on the CPU engine,
so set a language when running on the CPU.

## Record modes

- **All audio** (selected at every start): records the microphone and the speaker output continuously.
- **Only calls**: recording starts when an application starts using the microphone and stops 10 seconds after
  it releases it. Each call gets its own transcript. The choice applies to the current session.

### Pause after silence

Recording pauses by itself when nothing has been transcribed for one minute. The window notes it and the tray
icon turns grey. Press *Resume* to continue; a new call resumes recording automatically. Change the time under
Settings → *Stop recording after silence* (`[capture] auto_stop_minutes`, `0` disables it).

## Speakers

Voices are told apart automatically and labelled *Speaker 1*, *Speaker 2* and so on.

- **Rename**: click a name, type the new one and press Enter (Esc cancels). Every earlier line in the window,
  in the transcript file and in the answers context changes, and every later line of that voice uses the new
  name, including in later sessions.
- **Merge**: give two voices the same name. This is the fix when one person has been split into two speakers.
- **Forget**: Settings → *Voices* lists the known voices. *Forget* (click twice) deletes one voiceprint and
  name; *Forget all voices* deletes all of them. Lines that were already written keep their names.

Named voices are stored in `speakers.json`. The file contains voiceprints of real people; keep it private.

Recognition strictness is set with `[speakers] threshold`; see [Configuration](configuration.md#speakers).

## Screenshots

- **On demand**: the camera button in the window, or *Take screenshot* in the tray menu.
- **Automatic**: under Settings → *Screenshots*, turn on automatic screenshots. While recording, the screen is
  checked every `check_seconds` and saved when at least `change_percent` % of it has changed since the last
  screenshot.

Choose the screen (primary, all, or a specific one) in the same tab. Screenshots are saved in the transcript's
folder as `screenshot_HH-MM-SS.png`, noted in the transcript and shown in the window, where a click opens them.

## Filtering

Whisper tends to produce stock phrases on silence, such as `[INAUDIBLE]` or "Thank you for watching". These
are removed. Lines where the microphone only picked up the speaker output are dropped as well
(`[output] echo_dedupe`).

## Summaries and live answers

The **Claude** switch enables summaries and answers for the current session. It is off at every start. See
[Summaries and answers](summaries.md).
