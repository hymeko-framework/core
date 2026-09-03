param(
    [string]$OutputDir = (Join-Path $PSScriptRoot '..\deliverables\ifac_geoff_chase')
)

$ErrorActionPreference = 'Stop'
$out = [System.IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Force -Path $out | Out-Null

$script:pp = New-Object -ComObject PowerPoint.Application
$script:pp.Visible = -1

function Rgb([int]$r, [int]$g, [int]$b) { return $r + 256 * $g + 65536 * $b }

$navy = Rgb 7 22 52
$blue = Rgb 37 99 235
$cyan = Rgb 6 182 212
$violet = Rgb 124 58 237
$purple = Rgb 168 85 247
$white = Rgb 255 255 255
$ink = Rgb 20 34 58
$muted = Rgb 78 96 123
$pale = Rgb 241 249 255
$ice = Rgb 236 254 255
$lavender = Rgb 245 243 255
$line = Rgb 191 219 254
$deepPanel = Rgb 14 38 76
$gomb = 'G' + [char]0x00F6 + 'mb'
$gombUpper = 'G' + [char]0x00D6 + 'MB'

function Add-Box($slide, [float]$x, [float]$y, [float]$w, [float]$h, [int]$fill, [int]$stroke, [float]$radius = 0) {
    $kind = if ($radius -gt 0) { 5 } else { 1 }
    $s = $slide.Shapes.AddShape($kind, $x, $y, $w, $h)
    $s.Fill.ForeColor.RGB = $fill
    $s.Line.ForeColor.RGB = $stroke
    $s.Line.Weight = 1
    return $s
}

function Add-Text($slide, [string]$text, [float]$x, [float]$y, [float]$w, [float]$h,
                  [float]$size, [int]$color, [bool]$bold = $false, [string]$font = 'Aptos',
                  [int]$align = 1) {
    $t = $slide.Shapes.AddTextbox(1, $x, $y, $w, $h)
    $t.TextFrame.MarginLeft = 0
    $t.TextFrame.MarginRight = 0
    $t.TextFrame.MarginTop = 0
    $t.TextFrame.MarginBottom = 0
    $t.TextFrame.WordWrap = -1
    $t.TextFrame.AutoSize = 0
    $t.TextFrame.TextRange.Text = $text
    $t.TextFrame.TextRange.Font.Name = $font
    $t.TextFrame.TextRange.Font.Size = $size
    $t.TextFrame.TextRange.Font.Bold = if ($bold) { -1 } else { 0 }
    $t.TextFrame.TextRange.Font.Color.RGB = $color
    $t.TextFrame.TextRange.ParagraphFormat.Alignment = $align
    $t.TextFrame.AutoSize = 0
    $t.TextFrame2.AutoSize = 0
    $t.Left = $x
    $t.Top = $y
    $t.Width = $w
    $t.Height = $h
    return $t
}

function Add-Line($slide, [float]$x1, [float]$y1, [float]$x2, [float]$y2, [int]$color, [float]$weight = 1.5, [bool]$arrow = $false) {
    $l = $slide.Shapes.AddLine($x1, $y1, $x2, $y2)
    $l.Line.ForeColor.RGB = $color
    $l.Line.Weight = $weight
    if ($arrow) { $l.Line.EndArrowheadStyle = 3 }
    return $l
}

function Add-Node($slide, [float]$x, [float]$y, [float]$d, [int]$fill, [string]$label) {
    $n = $slide.Shapes.AddShape(9, $x, $y, $d, $d)
    $n.Fill.ForeColor.RGB = $fill
    $n.Line.ForeColor.RGB = $white
    $n.Line.Weight = 1.5
    Add-Text $slide $label $x ($y + $d * 0.24) $d ($d * 0.5) ($d * 0.23) $white $true 'Aptos' 2 | Out-Null
}

function Add-HypergraphGlyph($slide, [float]$x, [float]$y, [float]$w, [float]$h, [bool]$dark = $false) {
    # Bipartite incidence drawing: circular vertices connect to diamond-shaped
    # k-ary factors. Three overlapping factors make the multi-way structure
    # visually distinct from an ordinary pairwise graph.
    $vx = @(
        ($x + $w * 0.08), ($x + $w * 0.23), ($x + $w * 0.44), ($x + $w * 0.72),
        ($x + $w * 0.91), ($x + $w * 0.76), ($x + $w * 0.43), ($x + $w * 0.11)
    )
    $vy = @(
        ($y + $h * 0.22), ($y + $h * 0.06), ($y + $h * 0.20), ($y + $h * 0.08),
        ($y + $h * 0.38), ($y + $h * 0.69), ($y + $h * 0.86), ($y + $h * 0.67)
    )
    $fx = @(($x + $w * 0.28), ($x + $w * 0.64), ($x + $w * 0.43))
    $fy = @(($y + $h * 0.39), ($y + $h * 0.39), ($y + $h * 0.64))
    $factorColors = @($cyan, $violet, $purple)
    $links = @(
        @(0,0),@(1,0),@(2,0),
        @(2,1),@(3,1),@(4,1),@(5,1),
        @(0,2),@(2,2),@(4,2),@(6,2),@(7,2)
    )
    foreach($link in $links){
        $vi=$link[0]; $fi=$link[1]
        Add-Line $slide $vx[$vi] $vy[$vi] $fx[$fi] $fy[$fi] $factorColors[$fi] 1.7 | Out-Null
    }
    $d = [math]::Max(6, [math]::Min($w,$h) * 0.072)
    $fd = $d * 1.32
    $outline = if($dark){$white}else{$navy}
    for($i=0;$i -lt 3;$i++){
        $f=$slide.Shapes.AddShape(4, $fx[$i]-$fd/2, $fy[$i]-$fd/2, $fd, $fd)
        $f.Fill.ForeColor.RGB=$factorColors[$i]
        $f.Line.ForeColor.RGB=$outline
        $f.Line.Weight=1.0
    }
    for($i=0;$i -lt $vx.Count;$i++){
        $n=$slide.Shapes.AddShape(9, $vx[$i]-$d/2, $vy[$i]-$d/2, $d, $d)
        $n.Fill.ForeColor.RGB=if($i%2 -eq 0){$blue}else{$cyan}
        $n.Line.ForeColor.RGB=$outline
        $n.Line.Weight=1.0
    }
}

function Add-HumanoidGlyph($slide, [float]$x, [float]$y, [float]$w, [float]$h) {
    $cx=$x+$w*0.5
    $joint=$w*0.035
    $head=$slide.Shapes.AddShape(9,$cx-$w*0.07,$y,$w*0.14,$w*0.14)
    $head.Fill.ForeColor.RGB=$cyan; $head.Line.ForeColor.RGB=$navy; $head.Line.Weight=1.5
    $torso=Add-Box $slide ($x+$w*0.34) ($y+$h*0.18) ($w*0.32) ($h*0.32) $blue $navy 1
    $torso.Line.Weight=1.5
    $segments=@(
      @($cx,($y+$h*0.14),$cx,($y+$h*0.18)),
      @(($x+$w*0.36),($y+$h*0.24),($x+$w*0.18),($y+$h*0.43)),
      @(($x+$w*0.18),($y+$h*0.43),($x+$w*0.08),($y+$h*0.64)),
      @(($x+$w*0.64),($y+$h*0.24),($x+$w*0.82),($y+$h*0.43)),
      @(($x+$w*0.82),($y+$h*0.43),($x+$w*0.92),($y+$h*0.64)),
      @(($x+$w*0.43),($y+$h*0.49),($x+$w*0.36),($y+$h*0.70)),
      @(($x+$w*0.36),($y+$h*0.70),($x+$w*0.31),($y+$h*0.96)),
      @(($x+$w*0.57),($y+$h*0.49),($x+$w*0.64),($y+$h*0.70)),
      @(($x+$w*0.64),($y+$h*0.70),($x+$w*0.69),($y+$h*0.96))
    )
    foreach($seg in $segments){Add-Line $slide $seg[0] $seg[1] $seg[2] $seg[3] $violet 7 | Out-Null}
    $joints=@(
      @(($x+$w*0.36),($y+$h*0.24)),@(($x+$w*0.18),($y+$h*0.43)),@(($x+$w*0.08),($y+$h*0.64)),
      @(($x+$w*0.64),($y+$h*0.24)),@(($x+$w*0.82),($y+$h*0.43)),@(($x+$w*0.92),($y+$h*0.64)),
      @(($x+$w*0.43),($y+$h*0.49)),@(($x+$w*0.36),($y+$h*0.70)),@(($x+$w*0.31),($y+$h*0.96)),
      @(($x+$w*0.57),($y+$h*0.49)),@(($x+$w*0.64),($y+$h*0.70)),@(($x+$w*0.69),($y+$h*0.96))
    )
    foreach($pt in $joints){
      $n=$slide.Shapes.AddShape(9,$pt[0]-$joint/2,$pt[1]-$joint/2,$joint,$joint)
      $n.Fill.ForeColor.RGB=$cyan; $n.Line.ForeColor.RGB=$navy; $n.Line.Weight=1
    }
    $f1=$slide.Shapes.AddShape(4,$cx-$joint*0.8,$y+$h*0.29,$joint*1.6,$joint*1.6)
    $f1.Fill.ForeColor.RGB=$purple; $f1.Line.ForeColor.RGB=$white
    $f2=$slide.Shapes.AddShape(4,$cx-$joint*0.8,$y+$h*0.52,$joint*1.6,$joint*1.6)
    $f2.Fill.ForeColor.RGB=$cyan; $f2.Line.ForeColor.RGB=$white
}

function Add-MetricBar($slide, [string]$label, [float]$value, [string]$valueText,
                       [float]$x, [float]$y, [float]$w, [int]$color, [float]$maxValue = 15) {
    Add-Text $slide $label $x $y 125 22 13 $ink $true | Out-Null
    Add-Box $slide ($x+128) ($y+4) ($w-210) 16 $ice $line 1 | Out-Null
    $barW=[math]::Max(4,($w-210)*[math]::Min($value,$maxValue)/$maxValue)
    Add-Box $slide ($x+128) ($y+4) $barW 16 $color $color 1 | Out-Null
    Add-Text $slide $valueText ($x+$w-76) $y 76 22 13 $color $true 'Aptos' 3 | Out-Null
}

function Add-PictureFit($slide, [string]$path, [float]$x, [float]$y, [float]$w, [float]$h) {
    $pic = $slide.Shapes.AddPicture($path, 0, -1, $x, $y, -1, -1)
    $pic.LockAspectRatio = -1
    $widthScale = $w / $pic.Width
    $heightScale = $h / $pic.Height
    if ($widthScale -le $heightScale) {
        $pic.Width = $w
    } else {
        $pic.Height = $h
    }
    $pic.Left = $x + ($w - $pic.Width) / 2
    $pic.Top = $y + ($h - $pic.Height) / 2
    return $pic
}

function Add-PictureWatermark($slide, [string]$path, [float]$x, [float]$y, [float]$w, [float]$h,
                              [float]$transparency = 0.55) {
    # A picture fill supports whole-object transparency in PowerPoint COM,
    # unlike a normal picture shape. This keeps the HyMeKo mandala visible
    # as low-contrast page structure without competing with body copy.
    $shape = $slide.Shapes.AddShape(1, $x, $y, $w, $h)
    $shape.Fill.UserPicture($path)
    $shape.Fill.Transparency = $transparency
    $shape.Line.Visible = 0
    return $shape
}

function Save-Presentation($pres, [string]$stem) {
    $safe = 12.0
    foreach ($slide in $pres.Slides) {
        foreach ($shape in $slide.Shapes) {
            if ($shape.Left -lt -0.5 -or $shape.Top -lt -0.5 -or
                ($shape.Left + $shape.Width) -gt ($pres.PageSetup.SlideWidth + 0.5) -or
                ($shape.Top + $shape.Height) -gt ($pres.PageSetup.SlideHeight + 0.5)) {
                throw "Out-of-bounds shape '$($shape.Name)' in $stem"
            }
            $fullBleed = ($shape.Left -le 0.5 -and $shape.Top -le 0.5 -and
                          $shape.Width -ge ($pres.PageSetup.SlideWidth - 0.5))
            if (-not $fullBleed -and
                ($shape.Left -lt $safe -or $shape.Top -lt $safe -or
                 ($shape.Left + $shape.Width) -gt ($pres.PageSetup.SlideWidth - $safe) -or
                 ($shape.Top + $shape.Height) -gt ($pres.PageSetup.SlideHeight - $safe))) {
                throw "Shape '$($shape.Name)' violates the $safe-point safe margin in ${stem}: L=$($shape.Left) T=$($shape.Top) R=$($shape.Left+$shape.Width) B=$($shape.Top+$shape.Height) page=$($pres.PageSetup.SlideWidth)x$($pres.PageSetup.SlideHeight)"
            }
            if ($shape.HasTextFrame -and $shape.TextFrame.HasText) {
                $boundHeight = $shape.TextFrame2.TextRange.BoundHeight
                if ($boundHeight -gt ($shape.Height + 0.5)) {
                    $snippet = $shape.TextFrame.TextRange.Text.Replace("`r", ' ').Replace("`n", ' ')
                    if ($snippet.Length -gt 80) { $snippet = $snippet.Substring(0, 80) }
                    throw "Text overflow in '$($shape.Name)' for ${stem}: text=$boundHeight box=$($shape.Height) content='$snippet'"
                }
            }
        }
    }
    $pptx = Join-Path $out ($stem + '.pptx')
    $pdf = Join-Path $out ($stem + '.pdf')
    $pres.SaveAs($pptx, 24)
    $pres.SaveAs($pdf, 32)
    $pres.Close()
}

function Build-Poster {
    $p = $script:pp.Presentations.Add()
    $p.PageSetup.SlideWidth = 2383.2  # A1 landscape, points
    $p.PageSetup.SlideHeight = 1684.8
    $s = $p.Slides.Add(1, 12)
    $s.Background.Fill.ForeColor.RGB = $pale

    Add-Box $s 0 0 2383.2 210 $navy $navy | Out-Null
    Add-Text $s 'HyMeKo - Structure-Aware Embodied AI' 70 30 1720 62 46 $white $true | Out-Null
    Add-Text $s 'Verified relational features, local learning and robot control from one engineering model' 73 103 1760 38 24 $cyan $true | Out-Null
    Add-Text $s 'Embodied-AI and structured-control line with Prof. Shohei Kato (NITech / Katolab)' 73 162 1760 25 17 $white | Out-Null
    Add-Text $s "Csaba Hajdu | Adam Balazs Csapo | Katalin Kovacs`nSzechenyi Istvan University | Corvinus University / HUN-REN" 1835 38 470 108 17 $white $false 'Aptos' 3 | Out-Null

    Add-Text $s 'ONE MACHINE-CHECKABLE PATH: MODEL -> RELATIONAL IR -> LEARNING -> CONTROL -> VIEWS' 70 235 2240 32 23 $navy $true | Out-Null
    $stageCards = @(
      @('MODEL','Z:\experiments\2026_07_06_19_22_pick_place_dagger_hsikan\gifs\pick_place_dagger_hsikan_s1.gif','physics, geometry, actuation','FANUC task model',$blue),
      @('HYPERGRAPH IR','Z:\reports\figures\topology_zoo\arity_matching.png','typed relations + signed incidence','matched topology wins 9/9',$cyan),
      @('LEARNING','Z:\code_backup\nagare_github\reports\figures\hsikan_signed_orientation.png','sign, arity and orientation become features','0.993 / 0.997 vs ~0.54 ablations',$violet),
      @('CONTROL','Z:\reports\2026-08-12-delivery-viewer\coin_td3_K6-True_18.4mm.gif','learned policy under explicit monitors','22/24 sustained target-zone deliveries',$purple),
      @('VIEWS + PROOFS','Z:\reports\2026-08-06-r11-6c-demo-video\stills\05_montage_delivery.png','simulator, plots and certificates','44/44 covered targets',$blue)
    )
    for($i=0;$i -lt $stageCards.Count;$i++){
      $x=70+$i*452
      $c=$stageCards[$i]
      Add-Box $s $x 275 430 365 $white $line 1 | Out-Null
      Add-Text $s $c[0] ($x+18) 294 394 26 17 $c[4] $true | Out-Null
      if($i -eq 1 -or $i -eq 2){
        Add-PictureFit $s $c[1] ($x+18) 334 262 200 | Out-Null
        Add-HypergraphGlyph $s ($x+292) 354 116 158
        Add-Text $s 'k=3 / 4 / 5' ($x+288) 517 124 16 9 $violet $true 'Aptos' 2 | Out-Null
      } else {
        Add-PictureFit $s $c[1] ($x+18) 334 394 210 | Out-Null
      }
      Add-Text $s $c[2] ($x+18) 557 394 37 14 $ink $true 'Aptos' 2 | Out-Null
      Add-Text $s $c[3] ($x+18) 603 394 20 13 $c[4] $true 'Aptos' 2 | Out-Null
      if($i -lt 4){ Add-Line $s ($x+432) 455 ($x+449) 455 $violet 3 $true | Out-Null }
    }

    Add-Text $s 'MEASURED RESULTS' 70 675 900 32 24 $navy $true | Out-Null

    Add-Box $s 70 715 700 515 $white $line 1 | Out-Null
    Add-Text $s 'EMBODIED AI ON ROBOTS' 95 738 650 28 20 $blue $true | Out-Null
    Add-Text $s 'FANUC + DAGGER' 95 774 305 20 13 $blue $true | Out-Null
    Add-Text $s 'TEACHER-FREE TD3' 430 774 305 20 13 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_06_19_22_pick_place_dagger_hsikan\gifs\pick_place_dagger_hsikan_s1.gif' 95 800 305 215 | Out-Null
    Add-PictureFit $s 'Z:\reports\2026-08-12-delivery-viewer\coin_td3_K6-True_18.4mm.gif' 430 800 305 215 | Out-Null
    Add-Text $s '0.792 median place' 95 1027 305 31 21 $blue $true | Out-Null
    Add-Text $s '22 / 24 sustained' 430 1027 305 31 21 $purple $true | Out-Null
    Add-Text $s "mean 0.833 | BC start 0.542`nexpert ceiling 0.875 | 3 seeds, 24 evaluations" 95 1067 305 65 14.5 $ink | Out-Null
    Add-Text $s "settled inside the 20 mm target zone`n0 safety violations | closest verified distance 7.96 mm" 430 1067 305 65 14.5 $ink | Out-Null
    Add-Box $s 95 1144 640 57 $ice $line 1 | Out-Null
    Add-Text $s 'Closed-loop intelligence here means: structured world state -> learned action -> physical outcome -> explicit success and safety checks.' 112 1152 606 42 14.5 $ink $true 'Aptos' 2 | Out-Null

    Add-Box $s 795 715 760 515 $white $line 1 | Out-Null
    Add-Text $s 'HSIKAN - RELATIONAL AI FEATURES' 820 738 710 28 20 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\hsikan_signed_orientation.png' 820 780 535 310 | Out-Null
    Add-HypergraphGlyph $s 1370 810 145 240
    Add-Text $s 'signed multi-arity incidence' 1368 1062 150 22 10.5 $violet $true 'Aptos' 2 | Out-Null
    Add-Text $s 'AI features = signed cycles | interaction arity | incidence/orientation | typed physics | monitor outcomes' 820 1098 710 18 11.2 $ink $true 'Aptos' 2 | Out-Null
    $hsx=@(820,996,1172,1348)
    $hst=@(
      @('0.993 / 0.997','signed orientation recovered',$violet),
      @('0.963 AUC','Bitcoin Alpha | +0.064 vs best baseline',$blue),
      @('0.917 vs 0.486','true vs scrambled incidence',$purple),
      @('arity adapts','k=3 Bitcoin; k=5 Slashdot',$cyan)
    )
    for($i=0;$i -lt 4;$i++){
      Add-Box $s $hsx[$i] 1130 164 72 $lavender $line 1 | Out-Null
      Add-Text $s $hst[$i][0] ($hsx[$i]+8) 1140 148 24 15.5 $hst[$i][2] $true 'Aptos' 2 | Out-Null
      Add-Text $s $hst[$i][1] ($hsx[$i]+8) 1167 148 27 9.5 $ink $true 'Aptos' 2 | Out-Null
    }

    Add-Box $s 1580 715 728 515 $white $line 1 | Out-Null
    Add-Text $s 'NAGARE - LOCAL HSiKAN EXECUTION' 1605 738 678 28 20 $cyan $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\hsikan-bench.png' 1605 782 520 166 | Out-Null
    Add-HypergraphGlyph $s 2140 790 125 145
    Add-Text $s 'parallel k-ary work' 2137 939 132 17 9 $cyan $true 'Aptos' 2 | Out-Null
    Add-Text $s '7.3x scaling @16t | ~5x faster | ~3x less memory than PyTorch' 1605 955 678 27 17 $cyan $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-26-nagare-methods-bench.png' 1605 990 405 168 | Out-Null
    Add-Text $s 'AUROC 1.000' 2025 995 242 27 19 $violet $true | Out-Null
    Add-Text $s "clean moons / spiral / xor`nhard spiral 0.952-0.959`n`n60 vs 2,836 parameters`n23-25x faster forward" 2025 1027 242 116 14 $ink $true | Out-Null
    Add-Box $s 1605 1168 678 36 $ice $line 1 | Out-Null
    Add-Text $s 'CPU sweep: 1.2-2.6x | memory: 160x toy -> 3.2x scale | pruning: 4x smaller, 2.7-3.3x lower latency' 1618 1178 652 18 11.5 $ink $true 'Aptos' 2 | Out-Null

    Add-Text $s 'SCENARIO PORTFOLIO - ROBOTS, NETWORKS AND AUDIT EVIDENCE ON ONE POSTER' 70 1248 2238 25 19.5 $navy $true | Out-Null

    Add-Box $s 70 1280 440 315 $white $line 1 | Out-Null
    Add-Text $s 'PARAMETERIZED HUMANOID' 90 1300 400 22 16.5 $blue $true | Out-Null
    Add-HumanoidGlyph $s 95 1340 165 190
    Add-Text $s '68 LINKS' 278 1341 210 32 24 $blue $true | Out-Null
    Add-Text $s "six views: 1.29 ms`n5 fingers / hand bundle`n2.433 vs 566.5 ms`n233x speed-up" 278 1383 210 91 14 $ink $true | Out-Null
    Add-Box $s 90 1545 400 31 $ice $line 1 | Out-Null
    Add-Text $s 'One relational IR -> URDF + SDF + MJCF.' 102 1553 376 16 11.5 $ink $true 'Aptos' 2 | Out-Null

    Add-Box $s 525 1280 440 315 $white $line 1 | Out-Null
    Add-Text $s 'SONY AIBO ERS-1000' 545 1300 400 22 16.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_07_18_28_quadruped_stand_dagger\gifs\quadruped_stand_dagger_s0.gif' 545 1340 230 175 | Out-Null
    Add-Text $s '0.958' 792 1345 150 34 25 $purple $true | Out-Null
    Add-Text $s "DAgger standing`nBC 0.458`n22 DOF | 69 tests`n+0.38 m scripted trot" 792 1390 150 92 13.5 $ink $true | Out-Null
    Add-Box $s 545 1545 400 31 $lavender $line 1 | Out-Null
    Add-Text $s 'RL walking remains active research.' 557 1553 376 16 11.5 $purple $true 'Aptos' 2 | Out-Null

    Add-Box $s 980 1280 710 315 $white $line 1 | Out-Null
    Add-Text $s 'REAL SIGNED NETWORKS - SAME MACHINE' 1000 1300 670 22 16.5 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-29-nagare-signed-networks.png' 1000 1340 445 184 | Out-Null
    Add-Text $s 'BITCOIN OTC' 1460 1344 205 20 14.5 $cyan $true | Out-Null
    Add-Text $s "35,592 edges`nNAGARE 0.926`nHSiKAN 0.883`nunsigned 0.866" 1460 1371 205 74 12.5 $ink $true | Out-Null
    Add-Text $s 'EPINIONS' 1460 1460 205 20 14.5 $purple $true | Out-Null
    Add-Text $s "841,372 edges`nNAGARE 0.950 | HSiKAN 0.876`nsign-conditioning gain +0.015" 1460 1487 205 59 12.5 $ink $true | Out-Null
    Add-Text $s 'Frozen hyperparameters; linear holonomy wins this comparison.' 1000 1555 665 17 10.5 $muted $true 'Aptos' 2 | Out-Null

    Add-Box $s 1705 1280 603 315 $white $line 1 | Out-Null
    Add-Text $s 'LEAKAGE + CROSS-DATASET CHECKS' 1725 1300 563 22 16.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\leakage-audit.png' 1725 1340 300 170 | Out-Null
    Add-Text $s 'STRICT L' 2040 1344 245 18 13.5 $cyan $true | Out-Null
    Add-Text $s "OTC 0.000 | Epinions 0.028`nshuffle returns to chance" 2040 1368 245 43 11.5 $ink $true | Out-Null
    Add-Text $s 'TRANSDUCTIVE R' 2040 1420 245 18 13.5 $purple $true | Out-Null
    Add-Text $s "OTC 0.818 | Epinions 0.846`n75-86% retained; query edge remains" 2040 1444 245 48 11.5 $ink $true | Out-Null
    Add-Box $s 1725 1525 563 51 $navy $navy 1 | Out-Null
    Add-Text $s "NAGARE / PyTorch HSiKAN: Alpha 11.33x | Bitcoin OTC 12.07x`nSlashdot 10.81x | Epinions 9.04x | max framework delta-AUROC 0.0038" 1740 1533 533 35 10.5 $white $true 'Aptos' 2 | Out-Null

    Add-Text $s 'Evidence boundary: verified infrastructure and measured prototypes; learned-control claims remain limited to each labelled setup.' 70 1622 2238 26 15 $muted $false 'Aptos' 2 | Out-Null
    Save-Presentation $p 'hymeko_ai_poster_A1_landscape'
}

function Build-OnePagerEvidence {
    $p = $script:pp.Presentations.Add()
    $p.PageSetup.SlideWidth = 595.28
    $p.PageSetup.SlideHeight = 841.89
    $s = $p.Slides.Add(1, 12)
    $s.Background.Fill.ForeColor.RGB = $white

    Add-PictureWatermark $s 'Z:\code_backup\hakiko_ai_ws\knowledge\HyMeKo\hypergraphs\fano.png' 75 205 445 377 0.55 | Out-Null

    Add-Box $s 0 0 595.28 98 $navy $navy | Out-Null
    Add-Text $s 'HyMeKo - Structure-Aware Embodied AI' 35 13 525 32 24.5 $white $true | Out-Null
    Add-Text $s 'One model, relational intelligence, multiple verified views' 35 50 525 18 12.5 $cyan $true | Out-Null
    Add-Text $s 'Csaba Hajdu | Adam Balazs Csapo | Katalin Kovacs | with Prof. Shohei Kato (NITech / Katolab)' 35 77 525 11 7.8 $white | Out-Null

    $card=Add-Box $s 35 108 525 54 $lavender $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'ABSTRACT' 48 118 72 14 9.2 $violet $true | Out-Null
    Add-Text $s ("HyMeKo treats structure as part of intelligence. A single typed engineering model becomes robot, learning and evidence views; hypergraph features preserve multi-way relations; HSiKAN and $gomb learn from them; and cross-view checks connect model quality to physical outcomes, runtime and leakage.") 126 114 421 38 7.6 $ink | Out-Null

    Add-Text $s 'FOUR IDEAS IN PLAIN LANGUAGE' 35 173 300 13 10.4 $navy $true | Out-Null
    $concepts=@(
      @('SINGLE MODEL','One typed source holds geometry, physics, roles and constraints. Robot files, learners and evidence views reuse the same identities.',$blue),
      @('HYPERGRAPH','One relation may join three or more entities. Arity, sign, role and orientation stay explicit instead of becoming anonymous pairwise edges.',$cyan),
      @('CROSS-VIEW','The same entity and topology are projected into URDF, SDF, MJCF, simulation and plots. Agreement exposes drift between representations.',$violet),
      @('EMBODIED AI','AI closes the physical loop: structured world state -> learned action -> robot response. Success includes task outcome, settling and safety.',$purple)
    )
    for($i=0;$i -lt $concepts.Count;$i++){
      $x=35+$i*132
      $card=Add-Box $s $x 192 126 83 $white $line 1; $card.Fill.Transparency=0.10
      Add-Text $s $concepts[$i][0] ($x+9) 201 108 11 8.1 $concepts[$i][2] $true | Out-Null
      Add-Line $s ($x+9) 216 ($x+117) 216 $concepts[$i][2] 1.5 | Out-Null
      Add-Text $s $concepts[$i][1] ($x+9) 221 108 46 6.75 $ink | Out-Null
    }

    Add-Text $s 'MEASURED ROBOT RESULTS' 35 286 300 13 10.4 $navy $true | Out-Null
    $card=Add-Box $s 35 304 250 89 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'FANUC + DAGGER' 48 313 224 11 8.5 $blue $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_06_19_22_pick_place_dagger_hsikan\gifs\pick_place_dagger_hsikan_s1.gif' 48 331 78 50 | Out-Null
    Add-Text $s '0.792' 137 330 132 20 16 $blue $true | Out-Null
    Add-Text $s "median place | mean 0.833`nBC 0.542 | expert 0.875`n3 seeds / 24 evaluations" 137 355 132 28 7.1 $ink $true | Out-Null

    $card=Add-Box $s 310 304 250 89 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'TEACHER-FREE TD3' 323 313 224 11 8.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\reports\2026-08-12-delivery-viewer\coin_td3_K6-True_18.4mm.gif' 323 331 78 50 | Out-Null
    Add-Text $s '22 / 24' 412 330 132 20 16 $purple $true | Out-Null
    Add-Text $s "sustained inside 20 mm`n0 safety violations`n7.96 mm closest" 412 355 132 28 7.1 $ink $true | Out-Null

    $card=Add-Box $s 35 404 525 128 $lavender $line 1; $card.Fill.Transparency=0.08
    Add-Text $s ("AI MODEL FAMILIES: HSiKAN + $gombUpper") 48 413 499 13 9.3 $violet $true | Out-Null
    Add-Text $s 'HSIKAN' 48 434 235 11 8.5 $cyan $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\hsikan_signed_orientation.png' 48 450 118 63 | Out-Null
    Add-Text $s 'Hypergraph Spline KAN learns smooth, interpretable transforms on signed incidence and mixes interaction arities. Signed orientation reaches 0.993 / 0.997; Bitcoin Alpha reaches 0.963 AUC.' 174 449 109 62 6.9 $ink | Out-Null
    Add-Line $s 294 434 294 517 $line 1.2 | Out-Null
    Add-Text $s $gombUpper 305 434 242 11 8.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\reports\2026-08-19-gomb-cross-dataset.png' 305 450 126 63 | Out-Null
    Add-Text $s ("$gomb uses nested outer/inner representation banks to capture structural scales. On four signed graphs it beats HSiKAN by +0.0216 to +0.0364 held-out AUROC, in 5/5 paired seeds.") 439 449 108 62 6.9 $ink | Out-Null
    Add-Text $s ("HSiKAN explains relation transforms; $gomb adds multi-scale representation capacity.") 48 516 499 9 6.8 $violet $true 'Aptos' 2 | Out-Null

    $card=Add-Box $s 35 543 525 61 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'NAGARE EXECUTION LAYER' 48 551 150 11 8.4 $cyan $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\hsikan-bench.png' 48 567 145 27 | Out-Null
    Add-HypergraphGlyph $s 202 561 42 35
    Add-Text $s ("Local, hypergraph-native execution: HSiKAN scales 7.3x at 16 threads and is 9-12x faster than PyTorch across four signed graphs; $gomb is about 2.2x faster after the discarded-gradient fix.") 257 558 290 35 7.1 $ink $true | Out-Null

    Add-Text $s 'SCENARIO PORTFOLIO' 35 615 300 13 10.4 $navy $true | Out-Null
    $card=Add-Box $s 35 633 250 80 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'HUMANOID' 47 641 226 10 8.1 $blue $true | Out-Null
    Add-HumanoidGlyph $s 49 656 54 45
    Add-Text $s '68 links' 113 656 155 16 12.2 $blue $true | Out-Null
    Add-Text $s "six views 1.29 ms`nURDF+SDF+MJCF | 233x" 113 676 155 24 7.1 $ink $true | Out-Null

    $card=Add-Box $s 310 633 250 80 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'SONY AIBO ERS-1000' 322 641 226 10 8.1 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_07_18_28_quadruped_stand_dagger\gifs\quadruped_stand_dagger_s0.gif' 322 657 100 44 | Out-Null
    Add-Text $s '0.958' 432 657 112 15 12.2 $purple $true | Out-Null
    Add-Text $s "DAgger vs BC 0.458`n22 DOF | 69 tests | RL walk active" 432 676 112 25 6.8 $ink $true | Out-Null

    $card=Add-Box $s 35 722 250 80 $lavender $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'REAL SIGNED NETWORKS' 47 730 226 10 8.1 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-29-nagare-signed-networks.png' 47 746 105 44 | Out-Null
    Add-Text $s 'BITCOIN OTC 0.926' 163 746 110 12 8.3 $cyan $true | Out-Null
    Add-Text $s "Epinions 0.950`nsame-machine NAGARE AUROC" 163 763 110 25 6.8 $ink $true | Out-Null

    $card=Add-Box $s 310 722 250 80 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'LEAKAGE + CROSS-DATASET' 322 730 226 10 8.1 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\leakage-audit.png' 322 746 105 44 | Out-Null
    Add-Text $s 'STRICT L: 0 / 0.028' 438 745 109 11 7.8 $cyan $true | Out-Null
    Add-Text $s "R: 0.818 / 0.846`nNAGARE speed: 12.07x / 9.04x" 438 761 109 27 6.7 $ink $true | Out-Null

    Add-Text $s 'Protocols remain separated; cross-protocol ranking is not claimed. | hajdu.csaba@ga.sze.hu' 35 814 525 9 6.3 $muted $true 'Aptos' 2 | Out-Null

    Save-Presentation $p 'hymeko_ai_onepager_A4'
}

function Build-OnePager {
    $p = $script:pp.Presentations.Add()
    $p.PageSetup.SlideWidth = 595.28
    $p.PageSetup.SlideHeight = 841.89
    $s = $p.Slides.Add(1, 12)
    $s.Background.Fill.ForeColor.RGB = $white

    Add-PictureWatermark $s 'Z:\code_backup\hakiko_ai_ws\knowledge\HyMeKo\hypergraphs\fano.png' 75 205 445 377 0.58 | Out-Null

    Add-Box $s 0 0 595.28 98 $navy $navy | Out-Null
    Add-Text $s 'Csaba Hajdu - HyMeKo AI Research' 35 13 525 30 23.5 $white $true | Out-Null
    Add-Text $s 'Structure-aware and embodied intelligence from one verifiable model' 35 48 525 17 12.1 $cyan $true | Out-Null
    Add-Text $s "Computer Science PhD | Szechenyi Istvan University | ORCID 0000-0002-1058-7525`nwith Adam B. Csapo | embodied-AI line with Prof. Shohei Kato (NITech / Katolab)" 35 72 525 18 7.2 $white | Out-Null

    $card=Add-Box $s 35 108 525 50 $lavender $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'PROFILE' 48 118 68 12 8.8 $violet $true | Out-Null
    Add-Text $s ("I build structure-aware AI that keeps models, learning, control and evidence connected. HyMeKo uses one typed hypergraph model as a machine-checkable source; HSiKAN and $gomb learn relational features; NAGARE executes them locally; and embodied experiments test whether the representation survives contact with a robot.") 122 114 425 36 7.5 $ink | Out-Null

    $card=Add-Box $s 35 168 525 75 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'PHD IN BRIEF' 48 177 92 11 8.6 $blue $true | Out-Null
    Add-Text $s 'Hypergraph-Based Semantic Models in Cognitive and Robotic Systems' 145 177 402 11 8.3 $violet $true | Out-Null
    Add-Text $s 'My dissertation introduced HyMeKo as a concise hypergraph language and formalism for many-to-many semantics in cognitive and robotic systems. It established unique, universal and polynomial-time tensor mappings via star and clique expansion; generated domain views and executable artifacts from one source; and explored fuzzy signatures, structured LLM prompts and structural-entropy feedback for neural-network training. The goal was a reusable information source spanning physical, computational and cognitive views.' 48 196 499 39 7.05 $ink | Out-Null

    Add-Text $s 'AI RESEARCH I WOULD PURSUE' 35 254 340 13 10.4 $navy $true | Out-Null
    $card=Add-Box $s 35 272 525 135 $white $line 1; $card.Fill.Transparency=0.08
    $directions=@(
      @('VERIFIED NEURO-SYMBOLIC AI','Use HyMeKo as a constrained IR for LLM-generated models, code and RAG; verify topology, types and temporal rules before execution.',$blue),
      @('EMBODIED AI + SAFE CONTROL','Learn robot policies from typed world state while monitors expose task success, safety, settling and provenance; extend imitation/RL toward sim-to-real.',$purple),
      @('STRUCTURE-AWARE LEARNING',("Develop HSiKAN, $gomb and topology-selection methods for signed, mixed-arity and time-varying relational data."),$violet),
      @('EFFICIENT LOCAL AI','Fuse sparse hypergraph operators in NAGARE across CPU/GPU; study compact models, pruning and energy-aware edge deployment.',$cyan),
      @('CROSS-VIEW DIGITAL TWINS','Keep URDF, SDF, MJCF, simulators, telemetry and evidence synchronized from one model; quantify and repair cross-view drift.',$violet),
      @('COGNITIVE MULTI-AGENT SYSTEMS','Treat heterogeneous agents, robots and humans as a shared hypergraph memory with typed communication, planning and belief updates.',$blue)
    )
    for($i=0;$i -lt $directions.Count;$i++){
      $col=if($i -lt 3){0}else{1}; $row=if($i -lt 3){$i}else{$i-3}
      $x=48+$col*258; $y=282+$row*39
      $mini=Add-Box $s $x $y 241 34 $(if($row%2 -eq 0){$ice}else{$lavender}) $line 1; $mini.Fill.Transparency=0.04
      Add-Text $s $directions[$i][0] ($x+8) ($y+5) 225 9 7.6 $directions[$i][2] $true | Out-Null
      Add-Text $s $directions[$i][1] ($x+8) ($y+15) 225 16 6.35 $ink | Out-Null
    }

    Add-Text $s 'CORE TERMS' 35 417 200 13 10.2 $navy $true | Out-Null
    $terms=@(
      @('SINGLE MODEL','one typed source -> generated models, code and evidence',$blue),
      @('HYPERGRAPH','one relation can bind 3+ typed participants without flattening',$cyan),
      @('CROSS-VIEW','the same identity and topology checked across formats and tools',$violet),
      @('EMBODIED AI','learned action judged by physical outcome, settling and safety',$purple)
    )
    for($i=0;$i -lt $terms.Count;$i++){
      $x=35+$i*132
      $card=Add-Box $s $x 435 126 60 $white $line 1; $card.Fill.Transparency=0.10
      Add-Text $s $terms[$i][0] ($x+8) 443 110 10 7.7 $terms[$i][2] $true | Out-Null
      Add-Line $s ($x+8) 456 ($x+118) 456 $terms[$i][2] 1.4 | Out-Null
      Add-Text $s $terms[$i][1] ($x+8) 461 110 27 6.45 $ink | Out-Null
    }

    Add-Text $s 'EVIDENCE ACROSS SCENARIOS' 35 505 300 13 10.2 $navy $true | Out-Null
    $card=Add-Box $s 35 523 250 94 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'EMBODIED ROBOTS' 47 531 226 10 8.0 $blue $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_06_19_22_pick_place_dagger_hsikan\gifs\pick_place_dagger_hsikan_s1.gif' 47 547 100 43 | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_07_18_28_quadruped_stand_dagger\gifs\quadruped_stand_dagger_s0.gif' 160 547 100 43 | Out-Null
    Add-Text $s "FANUC DAgger 0.792 | TD3 22/24, 0 violations`nAibo DAgger 0.958 | 22 DOF, 69 tests" 47 594 226 16 6.8 $ink $true 'Aptos' 2 | Out-Null

    $card=Add-Box $s 310 523 250 94 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'SINGLE MODEL / CROSS-VIEW' 322 531 226 10 8.0 $cyan $true | Out-Null
    Add-HumanoidGlyph $s 324 546 58 50
    Add-HypergraphGlyph $s 394 548 68 47
    Add-Text $s '68 links' 474 547 73 13 10.5 $blue $true | Out-Null
    Add-Text $s "six views 1.29 ms`nURDF+SDF+MJCF 233x`n44/44 retrieval | topology 9/9" 474 564 73 35 6.6 $ink $true | Out-Null

    $card=Add-Box $s 35 625 250 94 $lavender $line 1; $card.Fill.Transparency=0.08
    Add-Text $s ("HSiKAN + $gombUpper") 47 633 226 10 8.0 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\hsikan_signed_orientation.png' 47 649 100 44 | Out-Null
    Add-PictureFit $s 'Z:\reports\2026-08-19-gomb-cross-dataset.png' 160 649 100 44 | Out-Null
    Add-Text $s ("HSiKAN orientation 0.993 / 0.997`n${gomb}: +0.0216 to +0.0364 vs HSiKAN; 5/5 paired seeds") 47 697 226 16 6.75 $ink $true 'Aptos' 2 | Out-Null

    $card=Add-Box $s 310 625 250 94 $white $line 1; $card.Fill.Transparency=0.08
    Add-Text $s 'REAL GRAPHS + AUDIT' 322 633 226 10 8.0 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-29-nagare-signed-networks.png' 322 649 100 44 | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\leakage-audit.png' 435 649 100 44 | Out-Null
    Add-Text $s "Bitcoin OTC / Epinions: 0.926 / 0.950`nstrict L: 0 / 0.028 | transductive R: 0.818 / 0.846 | speed: 12.07x / 9.04x" 322 697 226 16 6.55 $ink $true 'Aptos' 2 | Out-Null

    Add-Text $s 'CURRENT INTERESTS / COLLABORATION' 35 730 350 13 10.2 $navy $true | Out-Null
    Add-Box $s 35 748 525 75 $navy $navy 1 | Out-Null
    Add-Text $s 'interpretable + neuro-symbolic AI  |  cognitive robotics  |  graph / hypergraph ML  |  safe RL + imitation' 48 758 499 11 7.5 $cyan $true 'Aptos' 2 | Out-Null
    Add-Text $s 'formal verification  |  model-driven digital twins  |  local / HPC learning  |  LLM engineering' 48 774 499 11 7.5 $purple $true 'Aptos' 2 | Out-Null
    Add-Text $s 'I am especially interested in collaborations where structure, learning and control must remain explainable, efficient and testable.' 48 790 499 12 7.2 $white $true 'Aptos' 2 | Out-Null
    Add-Text $s 'ORCID 0000-0002-1058-7525  |  hajdu.csaba@ga.sze.hu' 48 805 499 8 6.8 $cyan $true 'Aptos' 2 | Out-Null
    Add-Text $s 'Dissertation: mmtdi.sze.hu/hajdu-csaba' 48 815 499 8 6.8 $cyan $true 'Aptos' 2 | Out-Null

    Save-Presentation $p 'hymeko_ai_onepager_A4'
}

function Build-ScenarioMatrix {
    $p = $script:pp.Presentations.Add()
    $p.PageSetup.SlideWidth = 2383.2
    $p.PageSetup.SlideHeight = 1684.8
    $s = $p.Slides.Add(1, 12)
    $s.Background.Fill.ForeColor.RGB = $pale

    Add-Box $s 0 0 2383.2 210 $navy $navy | Out-Null
    Add-Text $s 'HyMeKo - Scenario + Audit Matrix' 70 30 1750 62 45 $white $true | Out-Null
    Add-Text $s 'Embodied robots, real signed networks, cross-dataset comparisons and protocol-sensitive leakage' 73 103 1760 38 23 $cyan $true | Out-Null
    Add-Text $s 'Results are grouped by protocol so strict no-leak scores are not mixed with transductive edge-in-cycle results.' 73 162 1760 25 16 $white | Out-Null
    Add-Text $s "Csaba Hajdu | with Prof. Shohei Kato`nSzechenyi Istvan University" 1880 44 425 72 17 $white $false 'Aptos' 3 | Out-Null

    Add-Text $s 'ROBOT EMBODIMENT' 70 235 900 30 23 $navy $true | Out-Null
    Add-Box $s 70 275 425 545 $white $line 1 | Out-Null
    Add-Text $s 'PARAMETERIZED HUMANOID' 95 298 375 26 19 $blue $true | Out-Null
    Add-HumanoidGlyph $s 125 337 315 250
    Add-Text $s '28 / 44 / 68 links' 95 600 375 37 26 $blue $true 'Aptos' 2 | Out-Null
    Add-Text $s 'Atlas-class | baseline -> 5 fingers / hand' 95 643 375 24 15 $ink $true 'Aptos' 2 | Out-Null
    Add-Text $s "Full URDF + SDF + MJCF bundle`n1.170 / 1.691 / 2.433 ms`n397x / 298x / 233x vs converter chain" 95 680 375 92 17 $ink $true 'Aptos' 2 | Out-Null
    Add-Text $s 'Separate six-view run: 68 links in 1.29 ms.' 95 785 375 18 12.5 $muted $true 'Aptos' 2 | Out-Null

    Add-Box $s 520 275 450 545 $white $line 1 | Out-Null
    Add-Text $s 'SONY AIBO ERS-1000' 545 298 400 26 19 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_07_18_28_quadruped_stand_dagger\gifs\quadruped_stand_dagger_s0.gif' 545 337 400 245 | Out-Null
    Add-Text $s 'DAgger 0.958' 545 598 400 37 26 $purple $true 'Aptos' 2 | Out-Null
    Add-Text $s 'BC 0.458 | TD3+BC 0.000 | PD hold 1.000' 545 642 400 24 15 $ink $true 'Aptos' 2 | Out-Null
    Add-Text $s "22 DOF | 69 committed tests | 3-seed standing result`nscripted trot: +0.38 m / 700 steps, upright and finite" 545 680 400 70 16 $ink $true 'Aptos' 2 | Out-Null
    Add-Box $s 545 762 400 40 $lavender $line 1 | Out-Null
    Add-Text $s 'RL walking remains active research - not claimed as solved.' 558 773 374 18 12.5 $purple $true 'Aptos' 2 | Out-Null

    Add-Box $s 995 235 1313 585 $white $line 1 | Out-Null
    Add-Text $s 'REAL SIGNED NETWORKS - SAME-MACHINE, TRAIN-ONLY FEATURES' 1020 258 1263 28 21 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-29-nagare-signed-networks.png' 1020 306 880 450 | Out-Null
    Add-Text $s 'BITCOIN OTC' 1930 310 345 24 17 $cyan $true | Out-Null
    Add-Text $s "35,592 edges`npure NAGARE 0.926`nHSiKAN spline 0.883`nunsigned 0.866`nsign gain +0.017" 1930 342 345 114 15 $ink $true | Out-Null
    Add-Text $s 'EPINIONS' 1930 474 345 24 17 $purple $true | Out-Null
    Add-Text $s "841,372 edges`npure NAGARE 0.950`nHSiKAN spline 0.876`nunsigned 0.861`nsign gain +0.015" 1930 506 345 114 15 $ink $true | Out-Null
    Add-Box $s 1930 638 345 143 $lavender $line 1 | Out-Null
    Add-Text $s 'TRANSDUCTIVE PAPER RESULT' 1945 651 315 22 13 $violet $true | Out-Null
    Add-Text $s "Bitcoin OTC: HSiKAN 0.939`nSiGAT 0.934 | SGCN 0.901`n`nQuery edge participates in cycle incidence; not comparable to strict no-leak scores." 1945 679 315 89 12.5 $ink | Out-Null

    Add-Box $s 70 850 1450 720 $white $line 1 | Out-Null
    Add-Text $s 'LEAKAGE AUDIT - WHAT SURVIVES A TRAIN-LABEL SHUFFLE?' 95 875 1400 28 21 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\leakage-audit.png' 95 920 950 500 | Out-Null
    Add-Text $s 'TWO DIFFERENT AUDIT NUMBERS' 1070 920 420 24 16 $navy $true | Out-Null
    Add-Text $s "Strict residual leak:`nL = max(0, AUROCstrict,shuffle - 0.5)`n`nTransductive retention:`nR = (AUROCtransd,shuffle - 0.5) / (AUROCtransd,real - 0.5)" 1070 952 420 122 14 $ink $true | Out-Null
    Add-Text $s '5-SEED MEDIAN PANEL' 1070 1092 420 22 13 $cyan $true | Out-Null
    Add-Text $s 'dataset' 1070 1120 130 20 12 $muted $true | Out-Null
    Add-Text $s 'shuffle' 1200 1120 90 20 12 $muted $true 'Aptos' 2 | Out-Null
    Add-Text $s 'L' 1295 1120 70 20 12 $muted $true 'Aptos' 2 | Out-Null
    Add-Text $s 'R' 1370 1120 100 20 12 $muted $true 'Aptos' 2 | Out-Null
    $leakRows=@(
      @('BTC Alpha','0.503','0.003','0.823'),
      @('BTC OTC','0.481','0.000','0.818'),
      @('Slashdot','0.494','0.000','0.753'),
      @('Epinions','0.528','0.028','0.846'),
      @('Reddit-body','0.489','0.000','0.861')
    )
    for($i=0;$i -lt $leakRows.Count;$i++){
      $yy=1147+$i*41
      $fill=if($i%2 -eq 0){$ice}else{$lavender}
      Add-Box $s 1070 $yy 400 33 $fill $line 1 | Out-Null
      Add-Text $s $leakRows[$i][0] 1080 ($yy+7) 120 17 11.5 $ink $true | Out-Null
      Add-Text $s $leakRows[$i][1] 1200 ($yy+7) 90 17 11.5 $ink $true 'Aptos' 2 | Out-Null
      Add-Text $s $leakRows[$i][2] 1295 ($yy+7) 70 17 11.5 $purple $true 'Aptos' 2 | Out-Null
      Add-Text $s $leakRows[$i][3] 1370 ($yy+7) 90 17 11.5 $blue $true 'Aptos' 2 | Out-Null
    }
    Add-Box $s 1070 1362 400 174 $navy $navy 1 | Out-Null
    Add-Text $s 'INTERPRETATION' 1087 1378 366 21 14 $cyan $true | Out-Null
    Add-Text $s "Strict shuffle returns to chance: the train-only structural signal is legitimate.`n`nTransductive shuffle retains 75-86% of above-chance performance when the query edge remains inside its cycle incidence.`n`nAcross 7 methods x 5 datasets, baseline L=0.015-0.045; Reddit-body carries the unresolved residual." 1087 1408 366 126 12.5 $white | Out-Null

    Add-Box $s 1545 850 763 720 $white $line 1 | Out-Null
    Add-Text $s 'CROSS-DATASET FRAMEWORK COMPARISON' 1570 875 713 28 20 $blue $true | Out-Null
    Add-Text $s 'Nagare / PyTorch wall-time ratio (>1 = Nagare faster)' 1570 914 713 20 13.5 $muted $true | Out-Null
    Add-Text $s 'SIGNED-GRAPH HSiKAN' 1570 947 713 21 14 $cyan $true | Out-Null
    Add-MetricBar $s 'BTC Alpha' 11.33 '11.33x' 1570 978 690 $cyan
    Add-MetricBar $s 'BTC OTC' 12.07 '12.07x' 1570 1022 690 $cyan
    Add-MetricBar $s 'Slashdot*' 10.81 '10.81x' 1570 1066 690 $cyan
    Add-MetricBar $s 'Epinions*' 9.04 '9.04x' 1570 1110 690 $cyan
    Add-Text $s 'TABULAR KAN' 1570 1164 713 21 14 $purple $true | Out-Null
    Add-MetricBar $s 'Iris' 14.8 '14.8x' 1570 1195 690 $purple
    Add-MetricBar $s 'California' 6.3 '6.3x' 1570 1239 690 $purple
    Add-Box $s 1570 1294 690 242 $navy $navy 1 | Out-Null
    Add-Text $s 'SCOPE + RESULT' 1588 1310 654 22 14 $cyan $true | Out-Null
    Add-Text $s "Signed graphs: identical exported arrays, 8 threads, 300 steps, seed 0. Slashdot/Epinions use BFS-subsampled 40k-edge graphs. AUROC agrees across frameworks; largest difference 0.0038.`n`nHSiKAN stays 9-12x faster on all four graphs. Gomb is ~2.2x after a discarded-gradient fix; the wider dataset sweep exposed a 14.14x Epinions defect.`n`nIris: accuracy parity at 0.967. California: R2 0.722 vs 0.735, so 6.3x speed comes with slightly lower fit." 1588 1342 654 173 12.5 $white | Out-Null

    Add-Text $s 'Evidence boundary: robot figures, signed-link scores and leakage metrics come from separate labelled protocols; cross-protocol ranking is not claimed.' 70 1622 2238 26 15 $muted $false 'Aptos' 2 | Out-Null
    Save-Presentation $p 'hymeko_ai_scenario_matrix_A1_landscape'
}

function Build-ScenarioBrief {
    $p = $script:pp.Presentations.Add()
    $p.PageSetup.SlideWidth = 595.28
    $p.PageSetup.SlideHeight = 841.89
    $s = $p.Slides.Add(1, 12)
    $s.Background.Fill.ForeColor.RGB = $white

    Add-Box $s 0 0 595.28 108 $navy $navy | Out-Null
    Add-Text $s 'HyMeKo - Scenario + Audit Brief' 35 17 525 34 25 $white $true | Out-Null
    Add-Text $s 'Robots, signed networks and leakage-aware comparison' 35 57 525 20 13 $cyan $true | Out-Null
    Add-Text $s 'Strict and transductive results remain explicitly separated.' 35 86 525 12 8.5 $white | Out-Null

    Add-Text $s 'ROBOT SCENARIOS' 35 121 250 16 11.5 $navy $true | Out-Null
    Add-Box $s 35 145 250 176 $white $line 1 | Out-Null
    Add-Text $s 'HUMANOID' 48 157 224 14 9.5 $blue $true | Out-Null
    Add-HumanoidGlyph $s 55 180 103 113
    Add-Text $s '68 links' 170 181 100 22 16 $blue $true | Out-Null
    Add-Text $s "six views: 1.29 ms`n5 fingers/hand bundle:`n2.433 vs 566.5 ms`n233x speed-up" 170 210 100 70 8.5 $ink $true | Out-Null
    Add-Text $s 'URDF + SDF + MJCF from one IR' 48 299 224 12 7.8 $muted $true 'Aptos' 2 | Out-Null

    Add-Box $s 310 145 250 176 $white $line 1 | Out-Null
    Add-Text $s 'AIBO ERS-1000' 323 157 224 14 9.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\experiments\2026_07_07_18_28_quadruped_stand_dagger\gifs\quadruped_stand_dagger_s0.gif' 323 180 120 95 | Out-Null
    Add-Text $s '0.958' 454 181 92 22 16 $purple $true | Out-Null
    Add-Text $s "DAgger`nBC 0.458`nTD3+BC 0.000`n22 DOF | 69 tests" 454 210 92 61 8.5 $ink $true | Out-Null
    Add-Text $s '+0.38 m / 700 scripted steps; RL walk not claimed.' 323 290 224 20 7.8 $muted $true 'Aptos' 2 | Out-Null

    Add-Box $s 35 336 525 190 $lavender $line 1 | Out-Null
    Add-Text $s 'REAL SIGNED NETWORKS - SAME MACHINE' 48 348 499 14 9.5 $violet $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\2026-07-29-nagare-signed-networks.png' 48 370 335 143 | Out-Null
    Add-Text $s 'BITCOIN OTC' 396 372 148 13 8.5 $cyan $true | Out-Null
    Add-Text $s "35,592 edges`nNAGARE 0.926`nHSiKAN 0.883`nunsigned 0.866" 396 390 148 52 8 $ink $true | Out-Null
    Add-Text $s 'EPINIONS' 396 449 148 13 8.5 $purple $true | Out-Null
    Add-Text $s "841,372 edges`nNAGARE 0.950`nHSiKAN 0.876`nunsigned 0.861" 396 467 148 48 8 $ink $true | Out-Null

    Add-Box $s 35 540 525 170 $white $line 1 | Out-Null
    Add-Text $s 'LEAKAGE AUDIT' 48 552 499 14 9.5 $purple $true | Out-Null
    Add-PictureFit $s 'Z:\code_backup\nagare_github\reports\figures\leakage-audit.png' 48 575 320 120 | Out-Null
    Add-Text $s 'STRICT L' 382 575 162 13 8.5 $cyan $true | Out-Null
    Add-Text $s "max(0, shuffle - 0.5)`nOTC 0.000 | Epinions 0.028`nstrict shuffle returns to chance" 382 594 162 43 7.8 $ink $true | Out-Null
    Add-Text $s 'TRANSDUCTIVE R' 382 645 162 13 8.5 $purple $true | Out-Null
    Add-Text $s "retained above-chance signal`nOTC 0.818 | Epinions 0.846`nquery edge remains in cycle incidence" 382 664 162 36 7.6 $ink $true | Out-Null

    Add-Box $s 35 724 525 76 $navy $navy 1 | Out-Null
    Add-Text $s 'NAGARE / PYTORCH SPEED, IDENTICAL ARRAYS' 48 735 499 13 8.8 $cyan $true | Out-Null
    Add-Text $s "HSiKAN: Alpha 11.33x | OTC 12.07x | Slashdot* 10.81x | Epinions* 9.04x`nIris KAN 14.8x at accuracy parity | California KAN 6.3x at slightly lower R2" 48 754 499 28 8.0 $white $true 'Aptos' 2 | Out-Null
    Add-Text $s '* Slashdot/Epinions: BFS 40k-edge subgraphs; seed 0, 8 threads, 300 steps. | hajdu.csaba@ga.sze.hu' 35 814 525 12 7.0 $muted $true 'Aptos' 2 | Out-Null

    Save-Presentation $p 'hymeko_ai_scenario_audit_A4'
}

try {
    Build-Poster
    Build-OnePager
    Build-ScenarioMatrix
    Build-ScenarioBrief
} finally {
    $script:pp.Quit()
    [System.Runtime.InteropServices.Marshal]::ReleaseComObject($script:pp) | Out-Null
}

Write-Output $out
