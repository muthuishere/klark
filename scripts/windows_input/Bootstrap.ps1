#Requires -Version 7.0
param([Parameter(Mandatory)][string] $PlanPath)
. "$PSScriptRoot/Common.ps1"
$plan = Read-GauntletJson $PlanPath
# Terminal activation can inherit another process's environment. Normalize here too.
foreach ($item in @(Get-ChildItem Env:)) {
    if ($item.Name -like 'KLARK_*' -or $item.Name -in @('SSH_CONNECTION', 'SSH_TTY')) { Remove-Item "Env:$($item.Name)" }
}
$env:KLARK_CONFIG_PATH = $plan.config
$env:XDG_CONFIG_HOME = $plan.config_home
$env:KLARK_SESSION = $plan.session
if ($plan.profile -ne 'default') { $env:KLARK_WINDOWS_INPUT_PROBE = $plan.profile }
Add-Type -Path "$PSScriptRoot/Native.cs"
$before = [KlarkInputGauntlet.ConsoleProbe]::Geometry()
$child = $null
try {
    if ($plan.path -in @('klark', 'klark-remote')) {
        $child = New-GauntletProcess $plan.exe @('--session', $plan.session) $plan
    } else {
        $child = New-GauntletProcess $plan.pwsh @('-NoProfile', '-File', "$PSScriptRoot/Probe.ps1", '-PlanPath', $PlanPath) $plan
    }
    $sequence = 0
    while (-not $child.HasExited -and (Test-GauntletLease $plan.root) -and -not (Test-Path (Join-Path $plan.work 'stop'))) {
        $sequence++
        Write-GauntletJson (Join-Path $plan.work 'outer.json') @{
            nonce = $plan.nonce; pid = $PID; child_pid = $child.Id; sequence = $sequence
            geometry = [KlarkInputGauntlet.ConsoleProbe]::Geometry(); before = $before
        }
        Start-Sleep -Milliseconds 100
    }
} finally {
    $cleanup = @()
    if ($null -ne $child) {
        # Allow normal detach / observer disposal before forcing this owned child down.
        if (-not $child.WaitForExit(5000)) {
            $cleanup += 'Child required forced termination; normal console restoration was not qualified'
            $child.Kill($true)
            if (-not $child.WaitForExit(5000)) { $cleanup += 'Child remained active after forced termination' }
        }
        $child.Dispose()
    }
    Write-GauntletJson (Join-Path $plan.work 'bootstrap-exit.json') @{
        nonce = $plan.nonce; before = $before; after = [KlarkInputGauntlet.ConsoleProbe]::Geometry(); cleanup_errors = $cleanup
    }
}
