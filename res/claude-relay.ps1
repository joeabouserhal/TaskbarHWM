# TaskbarHWM relay for Claude Code's status line.
#
# Claude Code runs this on every status line update and passes its session data as JSON on stdin (the documented
# statusLine feature). This script keeps the few numbers TaskbarHWM shows and writes them next to itself as
# claude-status.txt. It reads nothing else, sends nothing anywhere, and prints nothing, so it adds no text to the
# status line. TaskbarHWM > Settings > Claude installs and removes it; deleting the "statusLine" entry from
# ~/.claude/settings.json disables it too.
$ErrorActionPreference = 'SilentlyContinue'
try {
    $inv = [Globalization.CultureInfo]::InvariantCulture
    $j = [Console]::In.ReadToEnd() | ConvertFrom-Json
    $out = New-Object 'System.Collections.Generic.List[string]'
    function Num($v) { if ($null -ne $v) { ([double]$v).ToString('0.##', $inv) } }
    function Add($key, $val) { if ($null -ne $val -and "$val" -ne '') { $out.Add("$key=$val") } }

    $rl = $j.rate_limits
    Add 'five_hour_pct' (Num $rl.five_hour.used_percentage)
    Add 'five_hour_resets' $rl.five_hour.resets_at
    Add 'seven_day_pct' (Num $rl.seven_day.used_percentage)
    Add 'seven_day_resets' $rl.seven_day.resets_at
    Add 'context_pct' (Num $j.context_window.used_percentage)
    Add 'cost_usd' (Num $j.cost.total_cost_usd)
    Add 'model' $j.model.display_name
    Add 'updated' ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())

    $dir = $PSScriptRoot
    $tmp = Join-Path $dir ("claude-status." + $PID + ".tmp")
    [IO.File]::WriteAllLines($tmp, $out)
    Move-Item -LiteralPath $tmp -Destination (Join-Path $dir 'claude-status.txt') -Force
} catch { }
