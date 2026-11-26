use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecResult {
    pub ok: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

fn truncate_output(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        s.truncate(MAX_OUTPUT_BYTES);
        s.push_str("\n...[output truncated to 32KB]");
    }
    s
}

/// Redacts tokens, API keys, passwords, and JWT-like secrets before logging or sending to AI models (AI-03).
pub fn redact_sensitive_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let lower = line.to_lowercase();
        let mut rewritten = line.to_string();

        for prefix in ["bearer ", "gho_", "ghp_", "sk-proj-", "sk-"] {
            if let Some(pos) = rewritten.to_lowercase().find(prefix) {
                let tail_start = pos + prefix.len();
                let mut end = tail_start;
                for ch in rewritten[tail_start..].chars() {
                    if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                        end += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                if end > tail_start + 4 {
                    rewritten.replace_range(pos..end, "[REDACTED_TOKEN]");
                }
            }
        }

        if lower.contains("password=")
            || lower.contains("password:")
            || lower.contains("pwd=")
            || lower.contains("secret=")
            || lower.contains("api_key=")
            || lower.contains("token=")
        {
            if let Some(eq_idx) = rewritten.find(['=', ':']) {
                let head = &rewritten[..=eq_idx];
                rewritten = format!("{head} [REDACTED]");
            }
        }

        out.push_str(&rewritten);
        out.push('\n');
    }
    if !input.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

pub fn execute_shell_command(shell: &str, command: &str, timeout_secs: u64) -> CommandExecResult {
    let t0 = Instant::now();
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandExecResult {
            ok: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: "empty command".into(),
            duration_ms: 0,
        };
    }

    #[cfg(windows)]
    let mut cmd = if shell.eq_ignore_ascii_case("cmd") {
        let mut c = Command::new("cmd.exe");
        c.args(["/C", trimmed]);
        c
    } else {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", trimmed]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let _ = shell;
        let mut c = Command::new("sh");
        c.args(["-c", trimmed]);
        c
    };

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandExecResult {
                ok: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("spawn error: {e}"),
                duration_ms: t0.elapsed().as_millis() as u64,
            };
        }
    };

    let deadline = Instant::now() + Duration::from_secs(timeout_secs.clamp(1, 60));
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = match child.wait_with_output() {
                    Ok(o) => o,
                    Err(e) => {
                        return CommandExecResult {
                            ok: false,
                            exit_code: -1,
                            stdout: String::new(),
                            stderr: format!("wait output error: {e}"),
                            duration_ms: t0.elapsed().as_millis() as u64,
                        };
                    }
                };
                let code = output.status.code().unwrap_or(-1);
                return CommandExecResult {
                    ok: output.status.success(),
                    exit_code: code,
                    stdout: truncate_output(String::from_utf8_lossy(&output.stdout).into_owned()),
                    stderr: truncate_output(String::from_utf8_lossy(&output.stderr).into_owned()),
                    duration_ms: t0.elapsed().as_millis() as u64,
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CommandExecResult {
                        ok: false,
                        exit_code: -2,
                        stdout: String::new(),
                        stderr: format!("command timed out after {timeout_secs}s"),
                        duration_ms: t0.elapsed().as_millis() as u64,
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return CommandExecResult {
                    ok: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("try_wait error: {e}"),
                    duration_ms: t0.elapsed().as_millis() as u64,
                };
            }
        }
    }
}

fn run_ps_json(script: &str) -> serde_json::Value {
    let res = execute_shell_command("powershell", script, 15);
    if !res.ok {
        return json!({
            "error": res.stderr,
            "exit_code": res.exit_code,
        });
    }
    serde_json::from_str::<serde_json::Value>(res.stdout.trim()).unwrap_or_else(|_| {
        json!({
            "raw": res.stdout.trim()
        })
    })
}

pub fn collect_inventory() -> serde_json::Value {
    #[cfg(windows)]
    {
        let ps = r#"
$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$disks = Get-CimInstance Win32_LogicalDisk -Filter "DriveType=3" | ForEach-Object {
  [ordered]@{
    drive = $_.DeviceID
    total_gb = [math]::Round($_.Size / 1GB, 2)
    free_gb = [math]::Round($_.FreeSpace / 1GB, 2)
  }
}
$ips = (Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { $_.IPAddress -notlike '127.*' -and $_.IPAddress -notlike '169.254.*' } | Select-Object -ExpandProperty IPAddress)
[ordered]@{
  hostname = $env:COMPUTERNAME
  os_caption = $os.Caption
  os_version = $os.Version
  arch = $os.OSArchitecture
  cpu = $cpu.Name
  cores = $cpu.NumberOfCores
  logical_processors = $cpu.NumberOfLogicalProcessors
  total_ram_gb = [math]::Round($os.TotalVisibleMemorySize / 1MB, 2)
  uptime_hours = [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 1)
  ipv4 = @($ips)
  disks = @($disks)
} | ConvertTo-Json -Depth 4 -Compress
"#;
        run_ps_json(ps)
    }
    #[cfg(not(windows))]
    {
        let host = hostname::get()
            .ok()
            .and_then(|s| s.into_string().ok())
            .unwrap_or_else(|| "unknown".into());
        json!({
            "hostname": host,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH
        })
    }
}

pub fn collect_metrics() -> serde_json::Value {
    #[cfg(windows)]
    {
        let ps = r#"
$os = Get-CimInstance Win32_OperatingSystem
$cpuLoad = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
$totalMb = [math]::Round($os.TotalVisibleMemorySize / 1024, 1)
$freeMb = [math]::Round($os.FreePhysicalMemory / 1024, 1)
$usedMb = [math]::Round($totalMb - $freeMb, 1)
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
$diskFreePct = if ($disk -and $disk.Size -gt 0) { [math]::Round(($disk.FreeSpace / $disk.Size) * 100, 1) } else { 0 }
$topProcs = Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 6 | ForEach-Object {
  [ordered]@{ name = $_.ProcessName; pid = $_.Id; ram_mb = [math]::Round($_.WorkingSet64 / 1MB, 1) }
}
[ordered]@{
  hostname = $env:COMPUTERNAME
  cpu_load_pct = $cpuLoad
  ram_total_mb = $totalMb
  ram_used_mb = $usedMb
  ram_free_mb = $freeMb
  c_drive_free_pct = $diskFreePct
  process_count = (Get-Process).Count
  top_processes = @($topProcs)
} | ConvertTo-Json -Depth 4 -Compress
"#;
        run_ps_json(ps)
    }
    #[cfg(not(windows))]
    {
        json!({ "os": std::env::consts::OS, "status": "ok" })
    }
}

pub fn collect_services() -> serde_json::Value {
    #[cfg(windows)]
    {
        let ps = r#"
$interesting = Get-Service | Where-Object { $_.Name -match 'AgentDesk|PostgreSQL|docker|WinRM|Spooler|wuauserv|BITS' } | Select-Object -First 15 Name, DisplayName, Status, StartType
$stoppedAuto = Get-CimInstance Win32_Service -Filter "StartMode='Auto' AND State!='Running'" | Select-Object -First 10 Name, DisplayName, State
[ordered]@{
  key_services = @($interesting | ForEach-Object { [ordered]@{ name = $_.Name; display = $_.DisplayName; status = "$($_.Status)"; start = "$($_.StartType)" } })
  stopped_auto_services = @($stoppedAuto | ForEach-Object { [ordered]@{ name = $_.Name; display = $_.DisplayName; state = $_.State } })
} | ConvertTo-Json -Depth 4 -Compress
"#;
        run_ps_json(ps)
    }
    #[cfg(not(windows))]
    {
        json!({ "key_services": [] })
    }
}

pub fn collect_recent_events() -> serde_json::Value {
    #[cfg(windows)]
    {
        let ps = r#"
$events = Get-WinEvent -FilterHashtable @{ LogName = 'System','Application'; Level = 1,2,3 } -MaxEvents 12 -ErrorAction SilentlyContinue | ForEach-Object {
  [ordered]@{
    time = $_.TimeCreated.ToString('o')
    log = $_.LogName
    level = $_.LevelDisplayName
    provider = $_.ProviderName
    id = $_.Id
    message = (($_.Message -replace '\s+', ' ').Substring(0, [math]::Min(180, ($_.Message -replace '\s+', ' ').Length)))
  }
}
[ordered]@{
  recent_warnings_errors = @($events)
} | ConvertTo-Json -Depth 4 -Compress
"#;
        let raw = run_ps_json(ps);
        let redacted_str = redact_sensitive_text(&raw.to_string());
        serde_json::from_str(&redacted_str).unwrap_or(raw)
    }
    #[cfg(not(windows))]
    {
        json!({ "recent_warnings_errors": [] })
    }
}

pub fn collect_hardware_resources() -> serde_json::Value {
    #[cfg(windows)]
    {
        let ps = r#"
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1 Name, NumberOfCores, NumberOfLogicalProcessors, LoadPercentage, MaxClockSpeed
$os = Get-CimInstance Win32_OperatingSystem | Select-Object CSName, Caption, TotalVisibleMemorySize, FreePhysicalMemory, LastBootUpTime
$totalRamGb = if ($os.TotalVisibleMemorySize) { [math]::Round($os.TotalVisibleMemorySize / 1MB, 2) } else { 0 }
$freeRamGb = if ($os.FreePhysicalMemory) { [math]::Round($os.FreePhysicalMemory / 1MB, 2) } else { 0 }
$usedRamGb = [math]::Round([math]::Max(0, $totalRamGb - $freeRamGb), 2)
$ramPct = if ($totalRamGb -gt 0) { [math]::Round(($usedRamGb / $totalRamGb) * 100.0, 1) } else { 0 }
$uptimeHours = if ($os.LastBootUpTime) { [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 1) } else { 0 }

$disks = Get-CimInstance Win32_LogicalDisk -Filter "DriveType=3" | ForEach-Object {
  $tot = if ($_.Size) { [math]::Round($_.Size / 1GB, 1) } else { 0 }
  $free = if ($_.FreeSpace) { [math]::Round($_.FreeSpace / 1GB, 1) } else { 0 }
  $usedPct = if ($tot -gt 0) { [math]::Round((($tot - $free) / $tot) * 100.0, 1) } else { 0 }
  $st = if ($usedPct -ge 92) { 'critical' } elseif ($usedPct -ge 80) { 'warn' } else { 'healthy' }
  [ordered]@{
    drive = $_.DeviceID
    label = if ($_.VolumeName) { $_.VolumeName } else { 'Local Disk' }
    total_gb = $tot
    free_gb = $free
    used_pct = $usedPct
    status = $st
  }
}

$gpuLoad = $null
try {
  if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    $nv = (nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits | Select-Object -First 1)
    if ($nv -match '^\s*(\d+)') { $gpuLoad = [double]$Matches[1] }
  }
  if ($null -eq $gpuLoad) {
    $sum = (Get-Counter '\GPU Engine(*engtype_3D)\Utilization Percentage' -ErrorAction Stop).CounterSamples | Measure-Object CookedValue -Sum | Select-Object -ExpandProperty Sum
    if ($null -ne $sum) { $gpuLoad = [math]::Round([math]::Min(100, [double]$sum), 1) }
  }
} catch {}

$gpus = Get-CimInstance Win32_VideoController | ForEach-Object {
  $vramGb = if ($_.AdapterRAM -and $_.AdapterRAM -gt 0) { [math]::Round($_.AdapterRAM / 1GB, 2) } else { $null }
  [ordered]@{
    name = $_.Name
    vram_gb = $vramGb
    driver = $_.DriverVersion
    status = $_.Status
    load_pct = $gpuLoad
  }
}

$users = Get-CimInstance Win32_UserAccount -Filter "LocalAccount=True" -ErrorAction SilentlyContinue | Select-Object -First 25 | ForEach-Object {
  [ordered]@{
    name = $_.Name
    full_name = if ($_.FullName) { $_.FullName } else { '' }
    enabled = (-not $_.Disabled)
  }
}

$sessions = @()
try {
  $qLines = quser 2>$null
  if ($qLines) {
    foreach ($line in ($qLines | Select-Object -Skip 1)) {
      $clean = ($line -replace '^>', ' ').Trim()
      if ($clean -match '^(\S+)\s+(?:(\S+)\s+)?(\d+)\s+(\S+)') {
        $sessions += [ordered]@{
          username = $Matches[1]
          session_name = if ($Matches[2]) { $Matches[2] } else { '' }
          id = $Matches[3]
          state = $Matches[4]
        }
      }
    }
  }
} catch {}

[ordered]@{
  hostname = $os.CSName
  os_caption = $os.Caption
  uptime_hours = $uptimeHours
  collected_at = (Get-Date).ToUniversalTime().ToString('o')
  cpu = [ordered]@{
    name = $cpu.Name
    cores = $cpu.NumberOfCores
    logical_processors = $cpu.NumberOfLogicalProcessors
    load_pct = if ($null -ne $cpu.LoadPercentage) { [double]$cpu.LoadPercentage } else { 0.0 }
    max_clock_mhz = $cpu.MaxClockSpeed
  }
  ram = [ordered]@{
    total_gb = $totalRamGb
    used_gb = $usedRamGb
    free_gb = $freeRamGb
    load_pct = $ramPct
  }
  disks = @($disks)
  gpus = @($gpus)
  windows_users = @($users)
  windows_sessions = @($sessions)
} | ConvertTo-Json -Depth 6 -Compress
"#;
        run_ps_json(ps)
    }
    #[cfg(not(windows))]
    {
        json!({
            "hostname": "unknown",
            "cpu": { "name": "unknown", "cores": 0, "logical_processors": 0, "load_pct": 0.0 },
            "ram": { "total_gb": 0.0, "used_gb": 0.0, "free_gb": 0.0, "load_pct": 0.0 },
            "disks": [],
            "gpus": [],
            "windows_users": [],
            "windows_sessions": []
        })
    }
}

pub fn trigger_reboot(delay_secs: u64) -> CommandExecResult {
    let delay = delay_secs.clamp(2, 120);
    #[cfg(windows)]
    {
        execute_shell_command(
            "powershell",
            &format!(
                "shutdown.exe /r /t {delay} /c 'AgentDesk: reinicio remoto con auto-reconexion'"
            ),
            10,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = delay;
        CommandExecResult {
            ok: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: "reboot solo soportado en Windows".into(),
            duration_ms: 0,
        }
    }
}

pub fn lock_workstation() -> CommandExecResult {
    #[cfg(windows)]
    {
        execute_shell_command("powershell", "rundll32.exe user32.dll,LockWorkStation", 10)
    }
    #[cfg(not(windows))]
    {
        CommandExecResult {
            ok: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: "lock solo soportado en Windows".into(),
            duration_ms: 0,
        }
    }
}

pub fn sign_out_windows_user(session_id: Option<&str>) -> CommandExecResult {
    #[cfg(windows)]
    {
        let cmd = match session_id.map(str::trim).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) {
            Some(id) => format!("logoff {id}"),
            None => "shutdown.exe /l".to_string(),
        };
        execute_shell_command("powershell", &cmd, 10)
    }
    #[cfg(not(windows))]
    {
        let _ = session_id;
        CommandExecResult {
            ok: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: "signout solo soportado en Windows".into(),
            duration_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_tokens_and_passwords() {
        let src = "Auth: Bearer abcdef123456789\napi_key=sk-test123456789\npassword=MySecret123\nok line";
        let red = redact_sensitive_text(src);
        assert!(!red.contains("abcdef123456789"));
        assert!(!red.contains("sk-test123456789"));
        assert!(!red.contains("MySecret123"));
        assert!(red.contains("ok line"));
    }

    #[test]
    fn executes_local_shell_command() {
        let res = execute_shell_command("powershell", "Write-Output 'AGENTDESK_RMM_OK'", 10);
        assert!(res.ok);
        assert!(res.stdout.contains("AGENTDESK_RMM_OK"));
    }

    #[test]
    fn collect_hardware_resources_has_cpu_ram_and_disks() {
        let hw = collect_hardware_resources();
        assert!(hw.get("cpu").is_some());
        assert!(hw.get("ram").and_then(|r| r.get("total_gb")).and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0);
        assert!(hw.get("disks").and_then(|d| d.as_array()).map(|a| !a.is_empty()).unwrap_or(false));
    }
}
