# Privacy

[README](../README.md) · [Getting started](getting-started.md) · [Usage](usage.md) ·
[Configuration](configuration.md) · [Summaries and answers](summaries.md) · **Privacy** ·
[Development](development.md)

## On the computer

- Audio is processed locally and is never written to disk or sent anywhere.
- Transcripts, screenshots, summaries, answers, `speakers.json` and logs are stored in the application folder.
- `speakers.json` contains voiceprints of the people you have recorded. Treat it like the transcripts.

## Network access

| When | Destination | What |
|---|---|---|
| When a model is missing (normally only the first start, or after changing the model) | Hugging Face, sherpa-onnx releases on GitHub | Model downloads |
| Claude switch on | Anthropic, through the Claude Code CLI | The transcript (summary) or a question with recent lines (answers). See [What is sent](summaries.md#what-is-sent). |

There is no telemetry.

## Consent

Recording a conversation may require the consent of everyone involved. Make sure all participants have agreed
before you record.

## Working on the code

Keep real transcripts, screenshots, voiceprints and logs out of the repository. `.gitignore` excludes the
build and development folders (`target\`, `dist\`, `dev-home\`); store real data elsewhere. The screenshots and
test audio in `docs\` are synthetic.
