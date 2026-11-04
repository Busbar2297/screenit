# Start the local SonarQube if it is down, then run the screenIt quality gate.
# Exit code is the scanner's: non-zero when the gate fails.
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $Root

$SonarHome = "E:\BitBucket\sonarqube-26.10.0.133078"
$Java = Join-Path $SonarHome "jres\jdk-21-full\jdk-21.0.13+9\bin\java.exe"
$StartBat = Join-Path $SonarHome "bin\windows-x86-64\StartSonar.bat"
$Credentials = Join-Path $SonarHome "screenit-credentials.txt"
$SonarLog = Join-Path $SonarHome "logs\sonar.log"
$HostUrl = "http://localhost:9000"

function Get-SonarStatus {
    try {
        $status = Invoke-RestMethod -Uri "$HostUrl/api/system/status" -TimeoutSec 5
        return [string]$status.status
    } catch {
        return $null
    }
}

function Test-SonarProcess {
    $procs = Get-CimInstance Win32_Process -Filter "Name = 'java.exe'" -ErrorAction SilentlyContinue
    foreach ($proc in $procs) {
        if ($proc.CommandLine -match "org\.sonar\.application\.App") {
            return $true
        }
    }
    return $false
}

$status = Get-SonarStatus
if ($status -ne "UP") {
    if (-not (Test-SonarProcess)) {
        if (-not (Test-Path -LiteralPath $Java)) {
            throw "JDK 21 not found at $Java"
        }
        if (-not (Test-Path -LiteralPath $StartBat)) {
            throw "StartSonar.bat not found at $StartBat"
        }
        Write-Host "Starting SonarQube..."
        $launcher = Join-Path $env:TEMP "screenit-start-sonar.cmd"
        $launcherBody = "@echo off`r`nset SONAR_JAVA_PATH=$Java`r`ncall `"$StartBat`"`r`n"
        [System.IO.File]::WriteAllText($launcher, $launcherBody)
        Start-Process -FilePath $launcher -WorkingDirectory (Split-Path -Parent $StartBat) -WindowStyle Minimized | Out-Null
    } else {
        Write-Host "SonarQube is already starting."
    }

    $deadline = (Get-Date).AddMinutes(6)
    do {
        Start-Sleep -Seconds 3
        $status = Get-SonarStatus
        if ($status) {
            Write-Host "SonarQube status: $status"
        }
    } while ($status -ne "UP" -and (Get-Date) -lt $deadline)

    if ($status -ne "UP") {
        if (Test-Path -LiteralPath $SonarLog) {
            Write-Host "---- last lines of sonar.log ----"
            Get-Content -LiteralPath $SonarLog -Tail 40
        }
        throw "SonarQube did not become UP at $HostUrl"
    }
} else {
    Write-Host "SonarQube is already up."
}

if (-not $env:SONAR_TOKEN) {
    if (-not (Test-Path -LiteralPath $Credentials)) {
        throw "No SONAR_TOKEN and no credentials file at $Credentials"
    }
    $match = Select-String -LiteralPath $Credentials -Pattern "^token=(.+)$"
    if (-not $match) {
        throw "credentials file has no token= line"
    }
    $env:SONAR_TOKEN = $match.Matches[0].Groups[1].Value
}

if (-not (Get-Command cargo-sonar-scanner -ErrorAction SilentlyContinue)) {
    Write-Host "Installing cargo-sonar-scanner..."
    cargo install cargo-sonar-scanner --locked
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

$env:SONAR_SCANNER_JSON_PARAMS = '{"sonar.qualitygate.wait":"true","sonar.qualitygate.timeout":"900"}'
cargo sonar-scanner
exit $LASTEXITCODE
