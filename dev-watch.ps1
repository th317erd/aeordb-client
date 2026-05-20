# dev-watch.ps1 — Windows-side debug watcher for aeordb-client.
#
# Polls an inbox directory for command requests and executes them in the
# user's interactive Windows session (which owns a desktop handle that
# SSH-launched commands do not). Supports rebuild and relaunch commands.
#
# Start:  powershell -ExecutionPolicy Bypass -File .\dev-watch.ps1
# Stop:   Ctrl+C in the window where it's running.
#
# Wire protocol:
#   inbox/<id>.req.json   { id, cmd, args }
#   outbox/<id>.res.json  { id, ok, exitCode, stdout, stderr, artifacts[], elapsedMs }
#   outbox/.heartbeat     ISO-8601 timestamp, refreshed every ~5s

$ErrorActionPreference = 'Continue'

# Shared root under C:\Users\Public so the watcher (running as the desktop
# user) and an SSH client (often a different account on dev VMs that
# auto-login as someone else) hit the same inbox/outbox files.
$WATCH_ROOT  = 'C:\Users\Public\aeordb-client-dev-watch'
$INBOX       = Join-Path $WATCH_ROOT 'inbox'
$OUTBOX      = Join-Path $WATCH_ROOT 'outbox'
$PROCESSED   = Join-Path $WATCH_ROOT 'processed'
foreach ($d in @($WATCH_ROOT, $INBOX, $OUTBOX, $PROCESSED)) {
  if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

# Redirect all output to a log file so we can debug post-mortem when the
# watcher dies. Start-Transcript captures Write-Host, Write-Output, and errors.
$LOG = Join-Path $WATCH_ROOT 'dev-watch.log'
try { Stop-Transcript -EA SilentlyContinue | Out-Null } catch { }
Start-Transcript -Path $LOG -Append -Force -ErrorAction SilentlyContinue | Out-Null
Write-Host ('[dev-watch] === startup ' + (Get-Date -Format 'o') + ' ===')
Write-Host ('[dev-watch] PID ' + $PID + '  user ' + $env:USERNAME + '  USERPROFILE ' + $env:USERPROFILE)

# Resolve paths from the script's own location so the watcher works
# regardless of which user account it runs under. dev-watch.ps1 lives at
# the workspace root next to Cargo.toml. The aeordb engine is a git dep
# (Cargo.toml) in this project — no local checkout to pull from.
$CLIENT_ROOT = $PSScriptRoot
$EXE         = Join-Path $CLIENT_ROOT 'target\release\aeordb-client.exe'

Write-Host ('[dev-watch] watching ' + $INBOX)
Write-Host ('[dev-watch] outbox   ' + $OUTBOX)
Write-Host '[dev-watch] commands: status, rebuild, relaunch'
Write-Host ''

$script:LAST_HEARTBEAT = [DateTime]::MinValue
function Update-Heartbeat {
  $beat = Join-Path $OUTBOX '.heartbeat'
  Set-Content -Path $beat -Value (Get-Date -Format 'o') -Force
  $script:LAST_HEARTBEAT = Get-Date
}

function To-Json($obj) {
  return ($obj | ConvertTo-Json -Depth 8 -Compress:$false)
}

function Build-Response($id, $exitCode, $stdout, $stderr, $artifacts, $elapsedMs) {
  # PS 5.1's ConvertTo-Json unwraps single-element arrays to scalars and
  # turns empty arrays into null, even through [string[]] casts inside
  # pscustomobject. A generic List<string> survives serialization as a
  # stable array regardless of element count, which the client jq pipeline
  # depends on.
  $list = New-Object 'System.Collections.Generic.List[string]'
  if ($null -ne $artifacts) {
    foreach ($a in @($artifacts | Where-Object { $null -ne $_ })) {
      $list.Add([string]$a)
    }
  }
  $resp = [pscustomobject]@{
    id        = $id
    ok        = ($exitCode -eq 0)
    exitCode  = $exitCode
    stdout    = $stdout
    stderr    = $stderr
    artifacts = $list
    elapsedMs = [int]$elapsedMs
  }
  return To-Json $resp
}

# ---------- Command handlers ----------

function Cmd-Status($id, $cmdArgs) {
  $proc = Get-Process -Name 'aeordb-client' -ErrorAction SilentlyContinue | Select-Object -First 1
  $exeInfo = if (Test-Path $EXE) { Get-Item $EXE } else { $null }
  $clientHead = $null
  if (Test-Path $CLIENT_ROOT) {
    Push-Location $CLIENT_ROOT
    try { $clientHead = (git rev-parse --short HEAD 2>$null).Trim() } catch { }
    Pop-Location
  }
  $http = [pscustomobject]@{ ok = $false; statusCode = $null; error = $null }
  try {
    $r = Invoke-WebRequest -Uri 'http://127.0.0.1:9400/api/v1/status' -UseBasicParsing -TimeoutSec 2
    $http = [pscustomobject]@{ ok = $true; statusCode = [int]$r.StatusCode; body = $r.Content }
  } catch {
    $http = [pscustomobject]@{ ok = $false; statusCode = $null; error = $_.Exception.Message }
  }
  $payload = [pscustomobject]@{
    process     = if ($proc) { [pscustomobject]@{ pid = $proc.Id; startTime = $proc.StartTime.ToString('o') } } else { $null }
    exe         = if ($exeInfo) { [pscustomobject]@{ size = [int]$exeInfo.Length; mtime = $exeInfo.LastWriteTime.ToString('o') } } else { $null }
    clientHead  = $clientHead
    http        = $http
  }
  return @{ stdout = (To-Json $payload); stderr = ''; exitCode = 0; artifacts = @() }
}

function Cmd-Relaunch($id, $cmdArgs) {
  $log = [System.Text.StringBuilder]::new()
  $stop = Get-Process -Name 'aeordb-client' -ErrorAction SilentlyContinue
  foreach ($p in $stop) {
    [void]$log.AppendLine('stopping PID ' + $p.Id)
    Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 2
  if (-not (Test-Path $EXE)) {
    [void]$log.AppendLine('EXE missing: ' + $EXE)
    return @{ stdout = $log.ToString(); stderr = ''; exitCode = 1; artifacts = @() }
  }
  Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','start','""',('"' + $EXE + '"') -WorkingDirectory $CLIENT_ROOT -WindowStyle Hidden | Out-Null
  $ok = $false
  for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Seconds 1
    try {
      $r = Invoke-WebRequest -Uri 'http://127.0.0.1:9400/api/v1/status' -UseBasicParsing -TimeoutSec 2
      if ($r.StatusCode -eq 200) { $ok = $true; [void]$log.AppendLine('HTTP 200 after ' + ($i + 1) + 's'); break }
    } catch { }
  }
  if (-not $ok) { [void]$log.AppendLine('HTTP not reachable after 20s') }
  $proc = Get-Process -Name 'aeordb-client' -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($proc) { [void]$log.AppendLine('new PID ' + $proc.Id) }
  return @{ stdout = $log.ToString(); stderr = ''; exitCode = if ($ok) { 0 } else { 1 }; artifacts = @() }
}

function Cmd-Rebuild($id, $cmdArgs) {
  $log = [System.Text.StringBuilder]::new()

  # Stop the client so cargo can overwrite the locked exe.
  $stop = Get-Process -Name 'aeordb-client' -ErrorAction SilentlyContinue
  foreach ($p in $stop) {
    [void]$log.AppendLine('stopping PID ' + $p.Id)
    Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
  }
  Start-Sleep -Seconds 2

  Push-Location $CLIENT_ROOT
  $preMtime  = if (Test-Path $EXE) { (Get-Item $EXE).LastWriteTime } else { $null }
  [void]$log.AppendLine('pre-build mtime: ' + $preMtime)
  $buildOut  = Join-Path $CLIENT_ROOT 'build-output.txt'
  $buildStart = Get-Date
  # -j 2 is mandatory per CLAUDE.md — unrestricted parallel cargo builds
  # trigger the Linux OOM killer; we keep the same job cap on Windows for
  # consistency (parallelism doesn't translate, but the cap is harmless).
  cmd /c "cargo build -j 2 --release > `"$buildOut`" 2>&1"
  $exit = $LASTEXITCODE
  $wall = ((Get-Date) - $buildStart).TotalSeconds
  [void]$log.AppendLine(('build wall: {0:N1}s   exit: {1}' -f $wall, $exit))
  Pop-Location

  if ($exit -ne 0) {
    [void]$log.AppendLine('BUILD FAILED — tail of build-output.txt:')
    (Get-Content $buildOut -Tail 25 -EA SilentlyContinue) | ForEach-Object { [void]$log.AppendLine('  | ' + $_) }
    # Fallback: relaunch the previous exe so the user is not left without a
    # working instance.
    if (Test-Path $EXE) {
      Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','start','""',('"' + $EXE + '"') -WorkingDirectory $CLIENT_ROOT -WindowStyle Hidden | Out-Null
      [void]$log.AppendLine('fallback relaunched old binary')
    }
    return @{ stdout = $log.ToString(); stderr = ''; exitCode = $exit; artifacts = @() }
  }

  $postMtime = (Get-Item $EXE).LastWriteTime
  [void]$log.AppendLine('post-build mtime: ' + $postMtime)
  if ($preMtime -and ($postMtime -le $preMtime)) {
    [void]$log.AppendLine('WARN: exe mtime did not advance')
  }

  Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','start','""',('"' + $EXE + '"') -WorkingDirectory $CLIENT_ROOT -WindowStyle Hidden | Out-Null
  $ok = $false
  for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Seconds 1
    try {
      $r = Invoke-WebRequest -Uri 'http://127.0.0.1:9400/api/v1/status' -UseBasicParsing -TimeoutSec 2
      if ($r.StatusCode -eq 200) { $ok = $true; [void]$log.AppendLine('HTTP 200 after ' + ($i + 1) + 's'); break }
    } catch { }
  }
  if (-not $ok) { [void]$log.AppendLine('HTTP not reachable after 20s') }
  return @{ stdout = $log.ToString(); stderr = ''; exitCode = if ($ok) { 0 } else { 1 }; artifacts = @() }
}

# ---------- Main loop ----------

Write-Host '[dev-watch] entering main loop'
while ($true) {
  try {
    if (((Get-Date) - $script:LAST_HEARTBEAT).TotalSeconds -ge 5) { Update-Heartbeat }
  } catch {
    Write-Host ('[dev-watch] heartbeat error: ' + $_)
  }

  $reqs = Get-ChildItem -Path $INBOX -Filter '*.req.json' -EA SilentlyContinue | Sort-Object Name
  foreach ($req in $reqs) {
    $started = Get-Date
    $id = $req.BaseName -replace '\.req$', ''
    $body = $null
    try {
      $body = Get-Content $req.FullName -Raw -EA Stop | ConvertFrom-Json
    } catch {
      $resp = Build-Response $id 99 '' ('bad request json: ' + $_) @() 0
      Set-Content -Path (Join-Path $OUTBOX ($id + '.res.json')) -Value $resp -Force
      Move-Item $req.FullName (Join-Path $PROCESSED $req.Name) -Force
      continue
    }

    $cmd = [string]$body.cmd
    $cmdArgs = $body.args
    Write-Host ('[dev-watch] ' + $id + ' cmd=' + $cmd)

    $result = $null
    try {
      switch ($cmd) {
        'status'   { $result = Cmd-Status   $id $cmdArgs }
        'relaunch' { $result = Cmd-Relaunch $id $cmdArgs }
        'rebuild'  { $result = Cmd-Rebuild  $id $cmdArgs }
        default    { $result = @{ stdout = ''; stderr = ('unknown cmd: ' + $cmd); exitCode = 98; artifacts = @() } }
      }
    } catch {
      $result = @{ stdout = ''; stderr = ($_ | Out-String); exitCode = 97; artifacts = @() }
    }

    $elapsed = ((Get-Date) - $started).TotalMilliseconds
    $resp = Build-Response $id $result.exitCode $result.stdout $result.stderr $result.artifacts $elapsed
    Set-Content -Path (Join-Path $OUTBOX ($id + '.res.json')) -Value $resp -Force
    Move-Item $req.FullName (Join-Path $PROCESSED $req.Name) -Force

    Write-Host ('[dev-watch] ' + $id + ' done exit=' + $result.exitCode + ' (' + ('{0:N0}' -f $elapsed) + 'ms)')
  }
  Start-Sleep -Milliseconds 300
}
Stop-Transcript -EA SilentlyContinue | Out-Null
