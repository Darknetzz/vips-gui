# Generates assets/icon.ico.
#
# The icon is a placeholder: two overlapping rounded squares suggesting a
# conversion between two formats. Replace assets/icon.ico with anything you
# prefer; the build only cares that the file exists.
#
# Run from the repository root:  pwsh -File assets/make-icon.ps1

Add-Type -AssemblyName System.Drawing

$ErrorActionPreference = 'Stop'
$outDir = Join-Path $PSScriptRoot ''
$icoPath = Join-Path $outDir 'icon.ico'

# Windows picks the closest match from these, so cover the common cases.
$sizes = @(256, 128, 64, 48, 32, 16)

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.Clear([System.Drawing.Color]::Transparent)

    # Scale every measurement off the canvas so all sizes look the same.
    $u = $size / 32.0

    # Rounded-square background with a vertical gradient.
    $pad = [double]($u * 1.5)
    $bgRect = New-Object System.Drawing.RectangleF($pad, $pad, ($size - 2 * $pad), ($size - 2 * $pad))
    $radius = [double]($u * 6)

    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $radius * 2
    $path.AddArc($bgRect.X, $bgRect.Y, $d, $d, 180, 90)
    $path.AddArc($bgRect.Right - $d, $bgRect.Y, $d, $d, 270, 90)
    $path.AddArc($bgRect.Right - $d, $bgRect.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($bgRect.X, $bgRect.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()

    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF($bgRect.X, $bgRect.Y)),
        (New-Object System.Drawing.PointF($bgRect.X, $bgRect.Bottom)),
        [System.Drawing.Color]::FromArgb(255, 38, 70, 96),
        [System.Drawing.Color]::FromArgb(255, 22, 42, 60))
    $g.FillPath($brush, $path)
    $brush.Dispose()

    # Two overlapping squares: the back one pale, the front one accent-coloured.
    $sq = [double]($u * 11)
    $backX = $u * 7.5
    $backY = $u * 8.0
    $frontX = $u * 13.5
    $frontY = $u * 13.0

    $backBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(230, 226, 232, 238))
    $g.FillRectangle($backBrush, [float]$backX, [float]$backY, [float]$sq, [float]$sq)
    $backBrush.Dispose()

    # A gap around the front square so the overlap reads clearly at 16px.
    $gapBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 28, 52, 74))
    $gap = [double]($u * 1.1)
    $g.FillRectangle($gapBrush, [float]($frontX - $gap), [float]($frontY - $gap), [float]($sq + 2 * $gap), [float]($sq + 2 * $gap))
    $gapBrush.Dispose()

    $frontBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 86, 186, 160))
    $g.FillRectangle($frontBrush, [float]$frontX, [float]$frontY, [float]$sq, [float]$sq)
    $frontBrush.Dispose()

    $g.Dispose()
    $path.Dispose()
    return $bmp
}

# Render each size to PNG bytes.
$pngs = @()
foreach ($size in $sizes) {
    $bmp = New-IconBitmap $size
    $stream = New-Object System.IO.MemoryStream
    $bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs += , @{ Size = $size; Bytes = $stream.ToArray() }
    $stream.Dispose()
    $bmp.Dispose()
}

# Pack into an ICO container. PNG-compressed entries are supported by Windows
# Vista and later, which is well below anything this app targets.
$out = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($out)

$writer.Write([uint16]0)             # reserved
$writer.Write([uint16]1)             # type: 1 = icon
$writer.Write([uint16]$pngs.Count)   # number of images

# Directory entries come first, so compute where the image data starts.
$offset = 6 + (16 * $pngs.Count)
foreach ($png in $pngs) {
    # 256 is stored as 0 in a single byte.
    $dim = if ($png.Size -ge 256) { 0 } else { $png.Size }
    $writer.Write([byte]$dim)        # width
    $writer.Write([byte]$dim)        # height
    $writer.Write([byte]0)           # palette size (0 = no palette)
    $writer.Write([byte]0)           # reserved
    $writer.Write([uint16]1)         # colour planes
    $writer.Write([uint16]32)        # bits per pixel
    $writer.Write([uint32]$png.Bytes.Length)
    $writer.Write([uint32]$offset)
    $offset += $png.Bytes.Length
}
foreach ($png in $pngs) {
    $writer.Write($png.Bytes)
}

$writer.Flush()
[System.IO.File]::WriteAllBytes($icoPath, $out.ToArray())
$writer.Dispose()
$out.Dispose()

Write-Output "wrote $icoPath ($((Get-Item $icoPath).Length) bytes, $($pngs.Count) sizes)"
