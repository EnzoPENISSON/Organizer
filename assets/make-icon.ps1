# Rebuilds organizer.ico from logo.svg (40 px and up), logo-small.svg (24-32 px) and
# logo-16.svg (16-20 px).
# Needs Microsoft Edge to render the SVGs. Run from anywhere: powershell -File assets\make-icon.ps1
# The .ico is committed, so a normal build does not need this script.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$dir = $PSScriptRoot
$tmp = Join-Path ([IO.Path]::GetTempPath()) 'organizer-icon'
New-Item -ItemType Directory -Force $tmp | Out-Null

$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
if (-not (Test-Path $edge)) { $edge = "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe" }

function Render($svg) {
    $png = Join-Path $tmp ([IO.Path]::GetFileNameWithoutExtension($svg) + '.png')
    if (Test-Path $png) { Remove-Item $png }
    $url = 'file:///' + ((Join-Path $dir $svg) -replace '\\', '/')
    $ErrorActionPreference = 'Continue'  # Edge reports progress on stderr
    & $edge --headless --disable-gpu --hide-scrollbars --window-size=256,256 `
        --default-background-color=00000000 --screenshot="$png" $url 2>$null | Out-Null
    $ErrorActionPreference = 'Stop'
    for ($i = 0; $i -lt 50 -and -not (Test-Path $png); $i++) { Start-Sleep -Milliseconds 100 }
    [Drawing.Bitmap]::new($png)
}

function Scale($src, $n) {
    $bmp = [Drawing.Bitmap]::new($n, $n, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $g.CompositingMode = 'SourceCopy'
    $g.DrawImage($src, 0, 0, $n, $n)
    $g.Dispose()
    $bmp
}

# 32-bit BMP icon image: header with doubled height, bottom-up BGRA rows, empty AND mask.
function ToDib($bmp) {
    $n = $bmp.Width
    $ms = [IO.MemoryStream]::new()
    $w = [IO.BinaryWriter]::new($ms)
    $w.Write([int]40); $w.Write([int]$n); $w.Write([int]($n * 2)); $w.Write([int16]1); $w.Write([int16]32)
    $w.Write([int]0); $w.Write([int]0); $w.Write([int]0); $w.Write([int]0); $w.Write([int]0); $w.Write([int]0)
    $rect = [Drawing.Rectangle]::new(0, 0, $n, $n)
    $data = $bmp.LockBits($rect, 'ReadOnly', [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $bytes = [byte[]]::new($n * $n * 4)
    [Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
    $bmp.UnlockBits($data)
    for ($y = $n - 1; $y -ge 0; $y--) { $w.Write($bytes, $y * $n * 4, $n * 4) }
    $w.Write([byte[]]::new([math]::Ceiling($n / 32) * 4 * $n))
    $w.Flush()
    , $ms.ToArray()  # the comma stops PowerShell unrolling the array into object[]
}

$large = Render 'logo.svg'
$small = Render 'logo-small.svg'
$tiny = Render 'logo-16.svg'

$images = foreach ($n in 16, 20, 24, 32, 40, 48, 64, 128, 256) {
    $src = if ($n -le 20) { $tiny } elseif ($n -le 32) { $small } else { $large }
    $bmp = Scale $src $n
    if ($n -eq 256) {
        $ms = [IO.MemoryStream]::new(); $bmp.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
        $bytes = $ms.ToArray()
        if ($n -eq 256) { $bmp.Save((Join-Path $dir 'logo.png'), [Drawing.Imaging.ImageFormat]::Png) }
    } else {
        $bytes = ToDib $bmp
    }
    $bmp.Dispose()
    [pscustomobject]@{ Size = $n; Bytes = $bytes }
}
$large.Dispose(); $small.Dispose(); $tiny.Dispose()

$ms = [IO.MemoryStream]::new()
$w = [IO.BinaryWriter]::new($ms)
$w.Write([int16]0); $w.Write([int16]1); $w.Write([int16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($img in $images) {
    $s = if ($img.Size -ge 256) { 0 } else { $img.Size }
    $w.Write([byte]$s); $w.Write([byte]$s); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([int16]1); $w.Write([int16]32); $w.Write([int]$img.Bytes.Length); $w.Write([int]$offset)
    $offset += $img.Bytes.Length
}
foreach ($img in $images) { $w.Write($img.Bytes) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $dir 'organizer.ico'), $ms.ToArray())
"Wrote $(Join-Path $dir 'organizer.ico') ($($images.Count) sizes)"
