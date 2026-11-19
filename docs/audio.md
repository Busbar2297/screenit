# Audio and speech

This page lists what each sound feature needs and how to set it up.

## What works out of the box

These run inside screenIt with nothing else installed:

| Feature | Where | Notes |
|---|---|---|
| Record microphone and system sound | Capture bar chips, Settings > Recording | Windows audio (WASAPI). Pick the default devices in Windows Sound settings. |
| Cut silences | Editor, Speech row, **Cut silences** | Finds quiet stretches in the recording's own sound. |
| Reduce noise | Editor, Speech row, **Reduce noise** | Saves `clip-clean.mp4`. |
| Narration with the Windows voice | `video.narrate` | Used when the text-to-speech URL is empty. |
| **Transcribe** with the Windows recognizer | Editor, Speech row | Used when the speech-to-text URL is empty (or the word `windows`). Offline, basic accuracy, only the languages installed under Settings > Time & language > Speech. |

## Several steps in a row

The editor's Speech area has a **Steps** row. Add steps with **+ Add step**, reorder them with
`<` and `>`, then press **Run steps**. Each step works on the file the previous step made,
so `Reduce noise > Transcribe > Cut silences > Burn subtitles` transcribes the cleaned sound
and burns subtitles that match the cut video. When a step needs subtitles of a file that has
none, a Transcribe step is added on its own. Only the final `clip-stepsN.mp4` is kept.

Tip for the Windows recognizer: put **Reduce noise** before **Transcribe**.

## What needs a speech server

Subtitles and filler-word cuts need speech-to-text. With no server set, screenIt uses the
recognizer built into Windows (see above): good enough for short, clear speech. For better
accuracy, longer recordings, other languages or accents, point it at a local server that speaks
the OpenAI audio API. screenIt does not ship a model. A configured server always wins:

| Feature | Needs |
|---|---|
| **Transcribe** (writes `clip.transcript.json` and `clip.captions.json`) | speech-to-text server |
| **Burn subtitles** | a transcript (run Transcribe first) |
| Cut silences > **removes um / uh** | a transcript |
| Cut silences > **adds subtitles** | a transcript |
| Narration with another voice | text-to-speech server |

The server must answer:

- `POST {url}/audio/transcriptions`, multipart with `file` (16 kHz mono WAV), `model`,
  `language`, `response_format=verbose_json` and `timestamp_granularities[]=word`. Word times
  are needed for filler cuts and subtitle timing.
- `POST {url}/audio/speech`, JSON `{"input", "model", "voice", "response_format": "wav"}`, for
  narration.

Servers known to work this way: [whisper.cpp server](https://github.com/ggml-org/whisper.cpp)
(`whisper-server`) and [Speaches](https://github.com/speaches-ai/speaches) (speech-to-text
and text-to-speech). Any OpenAI-compatible endpoint works, including a cloud one with an API key.

### Example: whisper.cpp on this PC

1. Download a whisper.cpp Windows release and a model, for example `ggml-base.en.bin`
   (English) or `ggml-small.bin` (multilingual, Arabic included).
2. Start the server:

   ```sh
   whisper-server -m ggml-small.bin --host 127.0.0.1 --port 8000 --inference-path /v1/audio/transcriptions
   ```

3. In screenIt open **Settings > Speech** and set:
   - Speech-to-text URL: `http://127.0.0.1:8000/v1`
   - Speech-to-text model: leave empty (whisper.cpp ignores it) or `whisper-1`
   - Spoken language: empty to detect, or `en` / `ar`
4. Press **Test connection**.

### Example: Speaches (speech-to-text and voices)

```sh
docker run -p 8000:8000 ghcr.io/speaches-ai/speaches:latest-cpu
```

Settings > Speech:

- Speech-to-text URL `http://localhost:8000/v1`, model `Systran/faster-whisper-small`
- Text-to-speech URL `http://localhost:8000/v1`, model and voice as listed by the server

## Settings reference

All of these live in `%LOCALAPPDATA%\screenIt\settings.json` under `speech`:

| Key | Meaning | Default |
|---|---|---|
| `stt_url` | speech-to-text base URL, ending in `/v1`; empty or `windows` uses the built-in Windows recognizer | empty (Windows recognizer) |
| `stt_model` | model name sent to the server | empty |
| `stt_language` | language hint (`en`, `ar`) | empty (detect) |
| `tts_url` | text-to-speech base URL; empty or the word `windows` uses the built-in Windows voice | empty (Windows voice) |
| `tts_model`, `tts_voice` | model and voice for narration | empty |
| `api_key` | sent as `Authorization: Bearer`; stored in plain text | empty |
| `timeout_secs` | per request, 1 to 1800 | 300 |

Recording sound lives under `recording`: `microphone`, `system_audio`.

## Auto-polish and sound

**Render polished video** keeps the recording's sound and follows the cuts and speed-ups. It
needs no server. The zoom, rings and badges come from the event log (`clip.events.json`)
written when Auto-polish is on while recording.

## Troubleshooting

- *"Needs a speech server"* or the Transcribe button fails: the speech-to-text URL is empty or
  the server is not running. Use **Test connection**.
- Subtitles have no word timing: the server ignored `timestamp_granularities[]=word`. Use
  whisper.cpp or Speaches.
- Long recordings time out: raise **Timeout** in Settings > Speech.
