# scripts/extract-brand-pngs.ps1
# Extracts PNG layers from multi-resolution .ico files for use as Tauri tray icons,
# Tauri bundle PNG, and frontend favicon/manifest icons.
#
# Run from repo root: pwsh -File scripts/extract-brand-pngs.ps1

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

function Save-IcoLayer {
    param(
        [string]$IcoPath,
        [int]$Size,
        [string]$OutPath
    )
    $bytes = [System.IO.File]::ReadAllBytes($IcoPath)
    $count = [BitConverter]::ToUInt16($bytes, 4)
    for ($i = 0; $i -lt $count; $i++) {
        $off = 6 + $i * 16
        $w = $bytes[$off]; if ($w -eq 0) { $w = 256 }
        $h = $bytes[$off + 1]; if ($h -eq 0) { $h = 256 }
        if ($w -eq $Size -and $h -eq $Size) {
            $byteCount = [BitConverter]::ToUInt32($bytes, $off + 8)
            $byteOffset = [BitConverter]::ToUInt32($bytes, $off + 12)
            $imgBytes = $bytes[$byteOffset..($byteOffset + $byteCount - 1)]
            # If the layer starts with the PNG signature, write directly.
            if ($imgBytes[0] -eq 0x89 -and $imgBytes[1] -eq 0x50 -and $imgBytes[2] -eq 0x4E -and $imgBytes[3] -eq 0x47) {
                [System.IO.File]::WriteAllBytes($OutPath, $imgBytes)
            } else {
                # BMP/DIB layer — round-trip through System.Drawing.Icon.
                $stream = [System.IO.MemoryStream]::new()
                $stream.Write($bytes, 0, $bytes.Length)
                $stream.Position = 0
                $icon = [System.Drawing.Icon]::new($stream, $Size, $Size)
                $bmp = $icon.ToBitmap()
                $bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
                $bmp.Dispose(); $icon.Dispose(); $stream.Dispose()
            }
            Write-Host "Wrote $OutPath ($Size x $Size)"
            return
        }
    }
    throw "No ${Size}x${Size} layer in $IcoPath"
}

function Save-Resized {
    param(
        [string]$SourcePngPath,
        [int]$TargetSize,
        [string]$OutPath
    )
    $src = [System.Drawing.Image]::FromFile((Resolve-Path $SourcePngPath))
    $bmp = [System.Drawing.Bitmap]::new($TargetSize, $TargetSize)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.DrawImage($src, 0, 0, $TargetSize, $TargetSize)
    $bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose(); $src.Dispose()
    Write-Host "Wrote $OutPath ($TargetSize x $TargetSize, resized from $SourcePngPath)"
}

# 256x256 master from the Windows ICO -> Tauri bundle PNG
Save-IcoLayer -IcoPath "noor-app\icons\noorwave.ico" -Size 256 -OutPath "noor-app\icons\icon.png"

# Resized variants for PWA + Apple touch icon
Save-Resized -SourcePngPath "noor-app\icons\icon.png" -TargetSize 192 -OutPath "frontend\static\icon-192.png"
Save-Resized -SourcePngPath "noor-app\icons\icon.png" -TargetSize 180 -OutPath "frontend\static\apple-touch-icon-180.png"

# 32x32 tray icons (theme-aware: black for light Windows themes, white for dark)
Save-IcoLayer -IcoPath "noor-app\icons\noorwave-tray-black.ico" -Size 32 -OutPath "noor-app\icons\tray-black-32.png"
Save-IcoLayer -IcoPath "noor-app\icons\noorwave-tray-white.ico" -Size 32 -OutPath "noor-app\icons\tray-white-32.png"

Write-Host "`nDone. Generated PNGs:"
Get-ChildItem noor-app\icons\icon.png, noor-app\icons\tray-black-32.png, noor-app\icons\tray-white-32.png, frontend\static\icon-192.png, frontend\static\apple-touch-icon-180.png | Format-Table FullName, Length -AutoSize
