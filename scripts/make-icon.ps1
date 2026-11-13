<#
.SYNOPSIS
  Generates res\app.ico (the app / installer / shortcut icon). Output is committed, so you only need
  to run this when changing the artwork:  powershell -File scripts\make-icon.ps1
#>
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$out  = Join-Path $root "res\app.ico"
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null

function New-Frame([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded dark tile
    $r = [single]($size * 0.22); $d = $r * 2; $s = [single]$size
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $d, $d, 180, 90); $path.AddArc($s - $d, 0, $d, $d, 270, 90)
    $path.AddArc($s - $d, $s - $d, $d, $d, 0, 90); $path.AddArc(0, $s - $d, $d, $d, 90, 90); $path.CloseFigure()
    $rect = New-Object System.Drawing.RectangleF 0, 0, $s, $s
    $bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.Color]::FromArgb(255, 44, 58, 66)), ([System.Drawing.Color]::FromArgb(255, 20, 27, 31)), 90
    $g.FillPath($bg, $path)

    # Three "usage" bars
    $cols = @([System.Drawing.Color]::FromArgb(255, 76, 194, 255), [System.Drawing.Color]::FromArgb(255, 108, 203, 95), [System.Drawing.Color]::FromArgb(255, 252, 194, 0))
    $heights = @(0.38, 0.62, 0.50)
    $bw = $s * 0.17; $gap = $s * 0.085; $x0 = ($s - (3 * $bw + 2 * $gap)) / 2; $base = $s * 0.78
    for ($i = 0; $i -lt 3; $i++) {
        $h = $s * 0.5 * ($heights[$i] / 0.62)
        $b = New-Object System.Drawing.SolidBrush $cols[$i]
        $bx = $x0 + $i * ($bw + $gap); $rr = [single]([math]::Max(1, $bw * 0.28))
        $bp = New-Object System.Drawing.Drawing2D.GraphicsPath
        $bp.AddArc($bx, $base - $h, $rr * 2, $rr * 2, 180, 90); $bp.AddArc($bx + $bw - $rr * 2, $base - $h, $rr * 2, $rr * 2, 270, 90)
        $bp.AddLine($bx + $bw, $base - $h + $rr, $bx + $bw, $base); $bp.AddLine($bx + $bw, $base, $bx, $base); $bp.CloseFigure()
        $g.FillPath($b, $bp)
    }
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    return ,$ms.ToArray()
}

# ICO container with PNG-compressed frames (supported since Vista).
$sizes = 16, 24, 32, 48, 64, 128, 256
$frames = $sizes | ForEach-Object { , (New-Frame $_) }
$ms = New-Object System.IO.MemoryStream; $bw = New-Object System.IO.BinaryWriter $ms
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $sz = $sizes[$i]; $dim = if ($sz -ge 256) { 0 } else { $sz }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]$frames[$i].Length); $bw.Write([uint32]$offset)
    $offset += $frames[$i].Length
}
foreach ($f in $frames) { $bw.Write($f) }
[System.IO.File]::WriteAllBytes($out, $ms.ToArray())
"wrote $out ($((Get-Item $out).Length) bytes, $($sizes.Count) sizes)"
