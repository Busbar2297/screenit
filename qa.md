# Quality gate

`qa.ps1` runs the whole cycle: start the local SonarQube when it is down, wait until it is
up, then run the screenIt quality gate. The gate reads `[workspace.metadata.sonar]` in the
root `Cargo.toml` (project key `screenit`, server `http://localhost:9000`). A failed gate
makes the script exit non-zero.

Dashboard: http://localhost:9000/dashboard?id=screenit

GitHub Actions stays on `cargo xtask ci`. A hosted runner cannot see this server.

## Run it

From the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\qa.ps1
```

The server stays running in a minimized window after the script finishes. The admin password
and the analysis token are in `E:\BitBucket\sonarqube-26.10.0.133078\screenit-credentials.txt`.
That file stays out of git. Do not put the token in `Cargo.toml`.

SonarQube Community Build 26.10 is at `E:\BitBucket\sonarqube-26.10.0.133078`. Elasticsearch
needs a full JDK 21, so the script starts it with Temurin 21.0.13 under `jres\jdk-21-full`.
The first start takes a few minutes. Later runs only scan, about a minute.

`target/**` is excluded. The scanner also skips files ignored by git. `Invalid highlighting`
lines in the log are the Rust analyzer miscounting Windows line endings. They do not fail the gate.

## On each commit

Install this hook once. Git for Windows runs it with `sh` before creating the commit, and the
hook calls the same script:

```powershell
$hook = @'
#!/bin/sh
cd "$(git rev-parse --show-toplevel)"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./qa.ps1
'@
$dir = Join-Path (git rev-parse --git-path hooks) ""
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$hookPath = Join-Path $dir "pre-commit"
[System.IO.File]::WriteAllText($hookPath, $hook.Replace("`r`n", "`n"))
Write-Host "Installed $hookPath"
```

To commit without the gate once: `git commit --no-verify`.
