# Generates the KeyFlag brand logo as a multi-resolution .ico (PNG-compressed entries)
# plus PNGs for the READMEs, and converts the existing DeskFlag .ico to a README PNG.
Add-Type -AssemblyName System.Drawing

function Add-RoundRect([System.Drawing.Drawing2D.GraphicsPath]$p, [single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
}

function New-KeyFlagBitmap([int]$size) {
    [single]$s = $size / 256.0
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.TextRenderingHint = 'AntiAliasGridFit'
    $g.Clear([System.Drawing.Color]::Transparent)

    # Indigo gradient rounded square (sibling to DeskFlag's blue square).
    $bg = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundRect $bg (8*$s) (8*$s) (240*$s) (240*$s) (56*$s)
    $rect = [System.Drawing.RectangleF]::new(8*$s, 8*$s, 240*$s, 240*$s)
    $brush = [System.Drawing.Drawing2D.LinearGradientBrush]::new($rect,
        [System.Drawing.Color]::FromArgb(255,99,102,241),
        [System.Drawing.Color]::FromArgb(255,67,56,202),
        45.0)
    $g.FillPath($brush, $bg)
    $brush.Dispose(); $bg.Dispose()

    # Keycap "side" (depth) — a lighter indigo rounded rect offset down a touch.
    $side = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundRect $side (62*$s) (66*$s) (132*$s) (132*$s) (30*$s)
    $sb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255,199,210,254))
    $g.FillPath($sb, $side); $sb.Dispose(); $side.Dispose()

    # Keycap top face — white rounded rect.
    $cap = [System.Drawing.Drawing2D.GraphicsPath]::new()
    Add-RoundRect $cap (62*$s) (58*$s) (132*$s) (132*$s) (30*$s)
    $wb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::White)
    $g.FillPath($wb, $cap); $wb.Dispose(); $cap.Dispose()

    # Bold "K" in indigo, centered on the keycap.
    [single]$fontSize = 92.0 * $s
    $font = [System.Drawing.Font]::new("Segoe UI", $fontSize, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $fmt = [System.Drawing.StringFormat]::new()
    $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
    $tb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255,67,56,202))
    $tr = [System.Drawing.RectangleF]::new(62*$s, 56*$s, 132*$s, 132*$s)
    $g.DrawString("K", $font, $tb, $tr, $fmt)
    $tb.Dispose(); $font.Dispose()

    $g.Dispose()
    return $bmp
}

function Save-Ico([int[]]$sizes, [string]$path) {
    $pngs = @()
    foreach ($sz in $sizes) {
        $bmp = New-KeyFlagBitmap $sz
        $ms = [System.IO.MemoryStream]::new()
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $pngs += ,@($sz, $ms.ToArray())
        $bmp.Dispose(); $ms.Dispose()
    }
    $out = [System.IO.MemoryStream]::new()
    $bw = [System.IO.BinaryWriter]::new($out)
    $bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$pngs.Count) # ICONDIR
    $offset = 6 + 16 * $pngs.Count
    foreach ($e in $pngs) {
        $sz = $e[0]; $bytes = $e[1]
        $bw.Write([byte]($(if ($sz -ge 256) {0} else {$sz})))   # width
        $bw.Write([byte]($(if ($sz -ge 256) {0} else {$sz})))   # height
        $bw.Write([byte]0); $bw.Write([byte]0)                  # colors, reserved
        $bw.Write([uint16]1); $bw.Write([uint16]32)             # planes, bitcount
        $bw.Write([uint32]$bytes.Length)                        # bytesInRes
        $bw.Write([uint32]$offset)                              # imageOffset
        $offset += $bytes.Length
    }
    foreach ($e in $pngs) { $bw.Write([byte[]]$e[1]) }
    $bw.Flush()
    [System.IO.File]::WriteAllBytes($path, $out.ToArray())
    $bw.Dispose(); $out.Dispose()
    Write-Host "wrote $path ($((Get-Item $path).Length) bytes, $($pngs.Count) sizes)"
}

$assets = Join-Path $PSScriptRoot "..\rs\assets"
New-Item -ItemType Directory -Force -Path $assets | Out-Null

# KeyFlag .ico (embedded + shortcut/installer icon) and README png.
Save-Ico @(16,24,32,48,64,128,256) (Join-Path $assets "keyflag.ico")
$png = New-KeyFlagBitmap 512
$png.Save((Join-Path $assets "logo.png"), [System.Drawing.Imaging.ImageFormat]::Png); $png.Dispose()
Write-Host "wrote logo.png (512)"

# DeskFlag README png from its existing .ico (reuse its current brand mark).
$dfIco = "C:\Users\User\Music\desk_flag\rs\assets\deskflag.ico"
if (Test-Path $dfIco) {
    $ico = [System.Drawing.Icon]::new($dfIco, 256, 256)
    $b = $ico.ToBitmap()
    $b.Save("C:\Users\User\Music\desk_flag\rs\assets\logo.png", [System.Drawing.Imaging.ImageFormat]::Png)
    $b.Dispose(); $ico.Dispose()
    Write-Host "wrote desk_flag logo.png"
}
