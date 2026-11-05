# Control protocol

Every user-visible action is an engine command, and the same command registry is reachable from
outside the UI: tests, scripts and AI agents drive screenIt exactly the way the buttons do.

## Transports

| Transport | Start | Notes |
|---|---|---|
| Running app | automatic | Loopback TCP on an ephemeral port. `%LOCALAPPDATA%\screenIt\control.json` holds `{"port", "token", "pid"}`; every request must carry that `token`. |
| CLI | `screenit-cli serve` | Same JSON lines over stdin/stdout, in a fresh headless session (no token). |
| MCP | `screenit-cli mcp` | Model Context Protocol over stdio; every command is a tool. |
| One-shot | `screenit-cli exec <command> [json]` | Prints the JSON result. `screenit-cli run '[["cmd",{...}],...]'` runs steps in one session. |

## Messages

One JSON object per line, one reply per line.

```json
{"id":1,"token":"<token>","method":"engine.execute","params":{"command":"capture.fullscreen","params":{}}}
```

Reply: `{"id":1,"ok":true,"result":{...}}` or `{"id":1,"ok":false,"error":{"code":"...","message":"..."}}`.

## Methods

- `engine.commands`: list commands (`id`, `label`, `menu`, `shortcut`, `params`, `needsDocument`).
- `engine.execute {command, params}`: run one command by id.

Unknown fields are errors and nothing is applied when validation fails. Requests sent to the
running app are executed on the UI thread, so the window reflects the result immediately; replies
time out after 60 s.

The AI tutorial commands are ordinary commands: `tutor.explore`, `tutor.plan`, `tutor.rehearse`,
`tutor.perform` and `tutor.produce` start a background job; poll `tutor.status` or block up to 55 s
with `tutor.wait`; `tutor.abort` stops. `tutor.explore` shows the model the window and takes one
pointer move at a time (a point from 0 to 1000 inside that picture, a key, or text). A changed
window title is kept; a dialog is followed until it closes. Pass `save` (a `.json` path) and the
script is written there. `tutor.status` reports `progress_done` and `progress_total`. When
`tutor.perform` finishes, each chapter recording is added to the library (`kind` `video`, tag
`tutorial`), and `tutor.produce` adds the finished movie the same way. While it is
stuck it sets `question`
in `tutor.status` and waits for `tutor.answer` (`action`: `hint` with `text`, `did_it`, `skip` or
`finish`). Input commands refuse to act unless the target window owns the foreground.

## Example (PowerShell)

```powershell
$c = Get-Content "$env:LOCALAPPDATA\screenIt\control.json" | ConvertFrom-Json
$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $c.port)
$w = [System.IO.StreamWriter]::new($tcp.GetStream()); $w.AutoFlush = $true
$r = [System.IO.StreamReader]::new($tcp.GetStream())
$w.WriteLine((@{id=1; token=$c.token; method="engine.execute";
  params=@{command="capture.fullscreen"; params=@{}}} | ConvertTo-Json -Depth 5 -Compress))
$r.ReadLine()
```
