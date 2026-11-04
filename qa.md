# Quality gate

Each commit runs the screenIt quality gate on the local SonarQube Community Build before Git
creates the commit. The gate reads `[workspace.metadata.sonar]` in the root `Cargo.toml`
(project key `screenit`, server `http://localhost:9000`). A failed gate refuses the commit.

GitHub Actions stays on `cargo xtask ci`. A hosted runner cannot see this server.

Dashboard: http://localhost:9000/dashboard?id=screenit

## Once per machine

SonarQube Community Build 26.10 lives at `E:\BitBucket\sonarqube-26.10.0.133078`.
Elasticsearch needs a full JDK 21. The one unpacked for this machine is Temurin 21.0.13:

```powershell
$env:SONAR_JAVA_PATH = "E:\BitBucket\sonarqube-26.10.0.133078\jres\jdk-21-full\jdk-21.0.13+9\bin\java.exe"
& "E:\BitBucket\sonarqube-26.10.0.133078\bin\windows-x86-64\StartSonar.bat"
```

Leave that window open. The server is ready when the log says `SonarQube is operational`,
or when this prints `UP`:

```powershell
(Invoke-RestMethod http://localhost:9000/api/system/status).status
```

Install the scanner once:

```powershell
cargo install cargo-sonar-scanner --locked
```

The admin password and the analysis token are in
`E:\BitBucket\sonarqube-26.10.0.133078\screenit-credentials.txt`. That file stays out of git.
Do not put the token in `Cargo.toml`.

## On each commit

From the repository root, in PowerShell, install the hook (once):

```powershell
$hook = @'
#!/bin/sh
set -e
cd "$(git rev-parse --show-toplevel)"

status=$(curl -sf http://localhost:9000/api/system/status || true)
echo "$status" | grep -q '"status":"UP"' || {
  echo "SonarQube is not up at http://localhost:9000. Start it, then commit again."
  exit 1
}

if [ -z "$SONAR_TOKEN" ]; then
  cred="E:/BitBucket/sonarqube-26.10.0.133078/screenit-credentials.txt"
  SONAR_TOKEN=$(sed -n "s/^token=//p" "$cred")
  export SONAR_TOKEN
fi
if [ -z "$SONAR_TOKEN" ]; then
  echo "SONAR_TOKEN is empty. Set it, or fill token= in the credentials file."
  exit 1
fi

export SONAR_SCANNER_JSON_PARAMS='{"sonar.qualitygate.wait":"true","sonar.qualitygate.timeout":"900"}'
cargo sonar-scanner
'@
$dir = Join-Path (git rev-parse --git-path hooks) ""
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$hookPath = Join-Path $dir "pre-commit"
[System.IO.File]::WriteAllText($hookPath, $hook.Replace("`r`n", "`n"))
Write-Host "Installed $hookPath"
```

Git for Windows runs that file with `sh` on every `git commit`. The scan takes about a minute
when the server is already up. `sonar.qualitygate.wait` makes `cargo sonar-scanner` exit
non-zero when the gate fails, and the commit is not created.

`target/**` is excluded. The scanner also skips files ignored by git. `Invalid highlighting`
lines in the log are the Rust analyzer miscounting Windows line endings. They do not fail the gate.

To commit without the gate once: `git commit --no-verify`.

## Run it by hand

Same check as the hook, without committing:

```powershell
$env:SONAR_TOKEN = (Select-String -Path "E:\BitBucket\sonarqube-26.10.0.133078\screenit-credentials.txt" -Pattern "^token=(.+)$").Matches[0].Groups[1].Value
$env:SONAR_SCANNER_JSON_PARAMS = '{"sonar.qualitygate.wait":"true","sonar.qualitygate.timeout":"900"}'
cargo sonar-scanner
```

Pass `-D` flags through `cmd` if you need them. PowerShell hands `-Dsonar...` to Cargo as a
broken `--define` value.

```powershell
cmd /c "cargo sonar-scanner -Dsonar.qualitygate.wait=true -Dsonar.qualitygate.timeout=900"
```
