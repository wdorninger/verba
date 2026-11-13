# Summaries and answers

[README](../README.md) · [Getting started](getting-started.md) · [Usage](usage.md) ·
[Configuration](configuration.md) · **Summaries and answers** · [Privacy](privacy.md) ·
[Development](development.md)

Verba can summarise a transcript and answer questions asked during a call. Both features use the Claude Code
CLI installed on the same computer. They are optional: transcription works without them, and nothing is sent
while the **Claude** switch in the window is off. The switch is off at every start.

## Features

- **Summary**: summary, decisions, action items and open questions, written in the language of the call and
  saved as `<transcript>_summary.md` in the transcript's folder. Start it with *Summarize*, or automatically
  at the end of each call with `[summary] auto_after_call = true` (*Only calls* mode).
- **Live answers**: with *Answer questions* on, questions asked in the call are answered in a side panel,
  optionally with web search. Right-click any line and choose *Ask Claude about this* to ask about it directly.
  Every answer is also saved to `<transcript>_answers.md` with the question, who asked it, the model and the
  answer. Speaker renames update this file too.

## Setup

1. **Account**: a Claude subscription (Pro, Max, Team or Enterprise) or an Anthropic Console account. The
   available models depend on the plan.
2. **Install the Claude Code CLI**, for example with the native installer in PowerShell:

   ```powershell
   irm https://claude.ai/install.ps1 | iex
   ```

   Verba looks for `claude.exe` or `claude.cmd` on `PATH` and in `%USERPROFILE%\.local\bin\claude.exe`.
3. **Sign in**: run `claude` once in a terminal and log in. Verba uses this login and stores no keys itself.
4. **Keep it current**: Verba passes `-p`, `--model`, `--system-prompt`, `--tools`, `--allowedTools`,
   `--no-session-persistence` and `--strict-mcp-config`. It has been tested with Claude Code 2.1.322. Update
   older installations with `claude update`.
5. **Network**: the CLI needs access to Anthropic. Behind a proxy, set `HTTPS_PROXY` as usual.

Check the setup from a terminal:

```
echo Say hello | claude -p --model haiku
```

## How the CLI is used

- Each request is a new, non-interactive session with no saved history, no MCP servers and no tools.
- With `[answers] web_search = true`, only the `WebSearch` tool is allowed. It must be available for your
  account; organisations can disable it.
- Time limits: 300 s per summary, 90 s per answer, 150 s per answer with web search.
- Errors such as *CLI not found*, *not signed in* or a timeout appear in the summary window or the answers
  panel, and in `logs\transcribe.log`.

## What is sent

| Feature | Data sent to Anthropic |
|---|---|
| Summary | The whole transcript |
| Answer | The question and the last `context_lines` lines of the transcript |

Nothing is sent while the Claude switch is off. Audio and screenshots are never sent.

## Settings

### `[summary]`

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Ignored at start; the switch is always off until turned on in the window |
| `model` | `"sonnet"` | `haiku`, `sonnet`, `opus`, or a full model name |
| `auto_after_call` | `false` | Summarise automatically when a call ends (*Only calls* mode) |

### `[answers]`

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Answer questions live; needs the Claude switch on |
| `model` | `"haiku"` | `haiku` is fastest; `sonnet` and `opus` are better but slower |
| `web_search` | `false` | Allow web search (slower) |
| `sources` | `"all"` | Whose questions to answer: `"all"`, `"them"` (call side) or `"me"` (microphone) |
| `context_lines` | `15` | Recent transcript lines sent with a question |
| `min_words` | `4` | Shorter questions are ignored |

Models are chosen from drop-downs in Settings. A full model name set in `config.toml` is shown there and kept.
