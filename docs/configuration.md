# Configuration

[README](../README.md) · [Getting started](getting-started.md) · [Usage](usage.md) · **Configuration** ·
[Summaries and answers](summaries.md) · [Privacy](privacy.md) · [Development](development.md)

Most settings can be changed in the Settings dialog, which has the tabs *Claude*, *Transcription*, *Voices*,
*Screenshots* and *Appearance*. All settings are stored in `config.toml` next to the executable. The file is
created with comments on first start, and changes made in the dialog are written back without removing them.

Some settings always start with a fixed value, regardless of the file: the Claude switch is off and the record
mode is *All audio*.

## Speech recognition engines

Set with `[whisper] backend` or in Settings. The engine is chosen at start, so restart Verba after changing it.

| Engine | Description |
|---|---|
| `"gpu"` (default) | OpenVINO GenAI on the Intel GPU through its C API. About 1.3 s for 10 s of speech, including language detection. A Silero voice-activity check skips silent chunks. |
| `"cpu"` | whisper.cpp with an 8-bit quantized model (`small` becomes `small-q8_0`: about twice as fast at practically the same accuracy). Used automatically if the GPU engine cannot start. When it falls more than `catchup_seconds` behind, it switches to `base-q8_0` until it has caught up. |

The GPU engine needs the Intel graphics driver (see [Getting started](getting-started.md#requirements)). If it
cannot start, the badge in the window says *fallback* and shows the reason.

## Settings reference

### `[whisper]`

| Key | Default | Description |
|---|---|---|
| `model` | `"small"` | `tiny`, `base`, `small`, `medium`, `large-v3`; on the GPU also `large-v3-turbo`; on the CPU also quantized names such as `small-q8_0` |
| `backend` | `"gpu"` | `"gpu"` or `"cpu"` |
| `language` | `"en"` | Language code, or `""` for automatic detection. Can also be changed in the window. |
| `redetect_every` | `1` | With automatic detection: detect the language every N chunks |
| `beam_size` | `1` | `1` is fastest (greedy); `5` is slightly more accurate |
| `cpu_threads` | `0` | `0` chooses automatically |
| `catchup_seconds` | `30` | CPU only: backlog after which the faster `base` model is used temporarily |

### `[capture]`

| Key | Default | Description |
|---|---|---|
| `mic` | `true` | Record the microphone |
| `speakers` | `true` | Record everything played through the default output device |
| `calls_only` | `false` | Ignored at start; *Only calls* is chosen per session in the window |
| `auto_stop_minutes` | `1` | Pause after this many minutes without speech; `0` = never |

Verba follows changes of the default audio devices and restarts a microphone stream that stops delivering
audio, for example after sleep.

### `[segmenter]`

Controls how audio is cut into chunks for recognition.

| Key | Default | Description |
|---|---|---|
| `silence_seconds` | `0.8` | Pause length that ends a chunk. Lower is faster but cuts more sentences. |
| `max_chunk_seconds` | `15.0` | Maximum chunk length during continuous speech |
| `min_speech_seconds` | `0.4` | Shorter sounds are ignored |
| `min_rms` | `0.005` | Minimum loudness counted as speech |
| `noise_ratio` | `3.0` | How much louder than the background noise speech must be |

### `[output]`

| Key | Default | Description |
|---|---|---|
| `directory` | `"transcripts"` | Where transcript folders are created |
| `echo_dedupe` | `true` | Drop microphone lines that are only the speaker output echoing into the microphone |

### `[speakers]`

| Key | Default | Description |
|---|---|---|
| `enabled` | `true` | Tell voices apart |
| `sources` | `["them", "me"]` | `"them"` = call participants, `"me"` = people in the room on your microphone |
| `threshold` | `0.75` | Higher is stricter: one person may be split in two (fix by giving both the same name). Lower may merge different people, which cannot be undone. |
| `min_seconds` | `1.5` | Shorter sentences never create a new speaker |

### `[summary]` and `[answers]`

See [Summaries and answers](summaries.md#settings).

### `[screenshots]`

| Key | Default | Description |
|---|---|---|
| `monitor` | `"primary"` | `"primary"`, `"all"`, or a screen number (Settings lists them) |
| `auto` | `false` | Save a screenshot while recording whenever enough of the screen has changed |
| `change_percent` | `10` | Share of the screen, in percent, that must differ from the last screenshot |
| `check_seconds` | `2` | How often the screen is checked |

### `[ui]`

| Key | Default | Description |
|---|---|---|
| `show_window_on_start` | `true` | Open the live window on start |
| `close_to_tray` | `true` | `true`: closing the window hides it; `false`: closing quits |
| `always_on_top` | `false` | |
| `live_preview` | `false` | Grey preview text while someone is speaking (extra processing) |
| `font_size` | `11` | |
| `theme` | `"system"` | `"system"`, `"light"` or `"dark"` |

## Tuning

If the transcript lags behind on the GPU engine, try these in order:

1. Lower `[segmenter] max_chunk_seconds` from 15 to 8 or 10.
2. Lower `[segmenter] silence_seconds`.
3. Turn off `[ui] live_preview`.

On the CPU engine, set a fixed language and keep the default quantized model.
