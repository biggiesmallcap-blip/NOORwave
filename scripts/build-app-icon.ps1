# scripts/build-app-icon.ps1
# Builds a multi-resolution Windows .ico for the NOORwave taskbar/window icon.
# Each layer is rendered natively at its target size with stroke widths tuned for
# legibility — bicubic-downscaling a single 256x256 master loses contrast at 16/32 px.
#
# Run from repo root: powershell -File scripts\build-app-icon.ps1

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

# Brand palette
$bgColor   = [System.Drawing.ColorTranslator]::FromHtml("#0B1220")
$ringColor = [System.Drawing.ColorTranslator]::FromHtml("#F5F7F9")

function New-AppIconPng {
    param([int]$Size, [string]$OutPath)

    $bmp = [System.Drawing.Bitmap]::new($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g   = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode    = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode  = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

    # Background: rounded square (corner radius ~22% — matches kit's 224/1024)
    $cornerRadius = [Math]::Max(2, [Math]::Round($Size * 0.22))
    $d = $cornerRadius * 2
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc(0, 0, $d, $d, 180, 90)
    $path.AddArc($Size - $d, 0, $d, $d, 270, 90)
    $path.AddArc($Size - $d, $Size - $d, $d, $d, 0, 90)
    $path.AddArc(0, $Size - $d, $d, $d, 90, 90)
    $path.CloseFigure()

    $bgBrush = [System.Drawing.SolidBrush]::new($bgColor)
    $g.FillPath($bgBrush, $path)
    $bgBrush.Dispose()
    $path.Dispose()

    if ($Size -ge 24) {
        # Standard OO geometry: two outer rings + two inner dots
        $cy        = $Size / 2.0
        $cxLeft    = $Size * 0.375
        $cxRight   = $Size * 0.625
        $rOuter    = $Size * 0.176
        $rInner    = $Size * 0.072

        # Stroke width tuned per size so the ring stays legible (>=1.5 px on screen)
        $stroke = if ($Size -ge 128) { $Size * 0.014 }   # 14/1024 — original master proportion
                  elseif ($Size -ge 64)  { $Size * 0.045 }
                  elseif ($Size -ge 32)  { $Size * 0.07 }
                  else                   { $Size * 0.10 }

        $pen   = [System.Drawing.Pen]::new($ringColor, [single]$stroke)
        $brush = [System.Drawing.SolidBrush]::new($ringColor)

        foreach ($cx in @($cxLeft, $cxRight)) {
            $g.DrawEllipse($pen, [single]($cx - $rOuter), [single]($cy - $rOuter), [single]($rOuter * 2), [single]($rOuter * 2))
            $g.FillEllipse($brush, [single]($cx - $rInner), [single]($cy - $rInner), [single]($rInner * 2), [single]($rInner * 2))
        }

        $pen.Dispose()
        $brush.Dispose()
    } else {
        # At 16 px the OO rings are sub-pixel — render two solid dots only.
        $brush   = [System.Drawing.SolidBrush]::new($ringColor)
        $cy      = $Size / 2.0
        $r       = [Math]::Max(2, $Size * 0.18)
        $cxLeft  = $Size * 0.32
        $cxRight = $Size * 0.68
        $g.FillEllipse($brush, [single]($cxLeft - $r),  [single]($cy - $r), [single]($r * 2), [single]($r * 2))
        $g.FillEllipse($brush, [single]($cxRight - $r), [single]($cy - $r), [single]($r * 2), [single]($r * 2))
        $brush.Dispose()
    }

    $g.Dispose()
    $bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "Rendered $OutPath ($Size x $Size)"
}

# Render each layer to a temp dir
$tempDir = Join-Path $env:TEMP "noorwave-ico-build"
if (Test-Path $tempDir) { Remove-Item -Recurse -Force $tempDir }
New-Item -ItemType Directory -Path $tempDir | Out-Null

$sizes = @(16, 32, 48, 64, 128, 256)
$pngs  = @()
foreach ($s in $sizes) {
    $p = Join-Path $tempDir "layer-$s.png"
    New-AppIconPng -Size $s -OutPath $p
    $pngs += @{ Size = $s; Bytes = [System.IO.File]::ReadAllBytes($p) }
}

# Pack PNG layers into a single multi-resolution .ico
$icoPath = "noor-app\icons\noorwave.ico"
$out     = [System.IO.File]::Open($icoPath, [System.IO.FileMode]::Create)
$writer  = [System.IO.BinaryWriter]::new($out)

# ICO header: reserved(2) type(2)=1 count(2)
$writer.Write([UInt16]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]$sizes.Count)

$dataOffset = 6 + $sizes.Count * 16
foreach ($f in $pngs) {
    $w = if ($f.Size -ge 256) { 0 } else { $f.Size }
    $h = if ($f.Size -ge 256) { 0 } else { $f.Size }
    $writer.Write([byte]$w)
    $writer.Write([byte]$h)
    $writer.Write([byte]0)              # palette (0 = no palette)
    $writer.Write([byte]0)              # reserved
    $writer.Write([UInt16]1)            # color planes
    $writer.Write([UInt16]32)           # bits per pixel
    $writer.Write([UInt32]$f.Bytes.Length)
    $writer.Write([UInt32]$dataOffset)
    $dataOffset += $f.Bytes.Length
}
foreach ($f in $pngs) {
    $writer.Write($f.Bytes)
}

$writer.Close()
$out.Close()

Remove-Item -Recurse -Force $tempDir

Write-Host "`nWrote $icoPath"
Get-Item $icoPath | Select-Object FullName, Length
