# Reusable TSV -> SVG chart generator.
#
# One generator for every chart in the repository rather than a bespoke script
# per figure: the axes, scaling, legend and grid are the same problem each time,
# and a second copy would drift from the first.
#
# Input: tab-separated `series<TAB>x<TAB>y`, one point per line, no header.
# Output: a standalone SVG on stdout.
#
# Variables (-v):
#   title, xlabel, ylabel   axis and chart labels
#   mode                    "line" (default) or "bar"
#   ymin, ymax              override the y range; otherwise taken from the data
#   w, h                    canvas size (default 900 x 460)
#
# Usage:
#   awk -f scripts/tsv_to_svg.awk -v title="..." -v xlabel="MiB" \
#       -v ylabel="GB/s" data.tsv > figure.svg

BEGIN {
    FS = "\t"
    if (w == 0) w = 900
    if (h == 0) h = 460
    ml = 78; mr = 210; mt = 54; mb = 62          # margins: right leaves room for the legend
    if (mode == "") mode = "line"
    npt = 0; nser = 0
    xlo = 1e308; xhi = -1e308; ylo = 1e308; yhi = -1e308
    # Colour-blind-safe qualitative palette (Okabe-Ito).
    pal[0]="#0072B2"; pal[1]="#D55E00"; pal[2]="#009E73"; pal[3]="#CC79A7"
    pal[4]="#E69F00"; pal[5]="#56B4E9"; pal[6]="#000000"; pal[7]="#F0E442"
}

/^[[:space:]]*$/ || /^#/ { next }

{
    s = $1; x = $2 + 0; y = $3 + 0
    if (!(s in sidx)) { sidx[s] = nser; sname[nser] = s; nser++ }
    npt++
    ps[npt] = sidx[s]; px[npt] = x; py[npt] = y
    if (x < xlo) xlo = x
    if (x > xhi) xhi = x
    if (y < ylo) ylo = y
    if (y > yhi) yhi = y
}

function sx(v) { return ml + (xhi == xlo ? 0 : (v - xlo) / (xhi - xlo) * (w - ml - mr)) }
function sy(v) { return h - mb - (yhi2 == ylo2 ? 0 : (v - ylo2) / (yhi2 - ylo2) * (h - mt - mb)) }
function esc(s) { gsub(/&/, "\\&amp;", s); gsub(/</, "\\&lt;", s); gsub(/>/, "\\&gt;", s); return s }
function fmt(v) { return (v >= 100 ? sprintf("%.0f", v) : (v >= 10 ? sprintf("%.1f", v) : sprintf("%.2f", v))) }

END {
    if (npt == 0) { print "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"; exit }

    ylo2 = (ymin != "") ? ymin + 0 : 0            # y axes start at zero unless told otherwise
    yhi2 = (ymax != "") ? ymax + 0 : yhi * 1.08
    if (yhi2 <= ylo2) yhi2 = ylo2 + 1

    printf "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" viewBox=\"0 0 %d %d\" font-family=\"DejaVu Sans, Segoe UI, sans-serif\">\n", w, h, w, h
    printf "<rect width=\"%d\" height=\"%d\" fill=\"#ffffff\"/>\n", w, h
    printf "<text x=\"%d\" y=\"30\" font-size=\"17\" font-weight=\"600\" fill=\"#111\">%s</text>\n", ml, esc(title)

    # horizontal grid + y ticks
    for (i = 0; i <= 5; i++) {
        gy = ylo2 + (yhi2 - ylo2) * i / 5
        yy = sy(gy)
        printf "<line x1=\"%d\" y1=\"%.1f\" x2=\"%d\" y2=\"%.1f\" stroke=\"#e3e3e3\" stroke-width=\"1\"/>\n", ml, yy, w - mr, yy
        printf "<text x=\"%d\" y=\"%.1f\" font-size=\"12\" fill=\"#555\" text-anchor=\"end\">%s</text>\n", ml - 9, yy + 4, fmt(gy)
    }
    # axes
    printf "<line x1=\"%d\" y1=\"%d\" x2=\"%d\" y2=\"%d\" stroke=\"#333\" stroke-width=\"1.4\"/>\n", ml, h - mb, w - mr, h - mb
    printf "<line x1=\"%d\" y1=\"%d\" x2=\"%d\" y2=\"%d\" stroke=\"#333\" stroke-width=\"1.4\"/>\n", ml, mt - 14, ml, h - mb
    printf "<text x=\"%d\" y=\"%d\" font-size=\"13\" fill=\"#333\" text-anchor=\"middle\">%s</text>\n", (ml + w - mr) / 2, h - 16, esc(xlabel)
    printf "<text x=\"18\" y=\"%d\" font-size=\"13\" fill=\"#333\" text-anchor=\"middle\" transform=\"rotate(-90 18 %d)\">%s</text>\n", (mt + h - mb) / 2, (mt + h - mb) / 2, esc(ylabel)

    # x ticks at the distinct x values actually present (these are size sweeps,
    # not continuous data, so labelling real points beats even subdivision)
    nxt = 0
    for (i = 1; i <= npt; i++) if (!(px[i] in seenx)) { seenx[px[i]] = 1; xs[nxt++] = px[i] }
    for (i = 0; i < nxt; i++)
        for (j = i + 1; j < nxt; j++)
            if (xs[j] < xs[i]) { t = xs[i]; xs[i] = xs[j]; xs[j] = t }
    step = (nxt > 12) ? int(nxt / 10) + 1 : 1
    for (i = 0; i < nxt; i += step) {
        xx = sx(xs[i])
        printf "<line x1=\"%.1f\" y1=\"%d\" x2=\"%.1f\" y2=\"%d\" stroke=\"#333\" stroke-width=\"1\"/>\n", xx, h - mb, xx, h - mb + 5
        printf "<text x=\"%.1f\" y=\"%d\" font-size=\"11\" fill=\"#555\" text-anchor=\"middle\">%s</text>\n", xx, h - mb + 19, fmt(xs[i])
    }

    if (mode == "bar") {
        bw = (w - ml - mr) / (nxt * nser + nxt)
        for (i = 1; i <= npt; i++) {
            slot = 0
            for (k = 0; k < nxt; k++) if (xs[k] == px[i]) slot = k
            bx = ml + slot * (bw * (nser + 1)) + ps[i] * bw + bw * 0.5
            printf "<rect x=\"%.1f\" y=\"%.1f\" width=\"%.1f\" height=\"%.1f\" fill=\"%s\" opacity=\"0.9\"/>\n", bx, sy(py[i]), bw * 0.92, (h - mb) - sy(py[i]), pal[ps[i] % 8]
            printf "<text x=\"%.1f\" y=\"%.1f\" font-size=\"10\" fill=\"#333\" text-anchor=\"middle\">%s</text>\n", bx + bw * 0.46, sy(py[i]) - 4, fmt(py[i])
        }
    } else {
        for (s = 0; s < nser; s++) {
            printf "<polyline fill=\"none\" stroke=\"%s\" stroke-width=\"2.4\" points=\"", pal[s % 8]
            for (i = 1; i <= npt; i++) if (ps[i] == s) printf "%.1f,%.1f ", sx(px[i]), sy(py[i])
            printf "\"/>\n"
            for (i = 1; i <= npt; i++) if (ps[i] == s)
                printf "<circle cx=\"%.1f\" cy=\"%.1f\" r=\"3.1\" fill=\"%s\"/>\n", sx(px[i]), sy(py[i]), pal[s % 8]
        }
    }

    # legend
    for (s = 0; s < nser; s++) {
        ly = mt + 6 + s * 21
        printf "<rect x=\"%d\" y=\"%d\" width=\"14\" height=\"11\" fill=\"%s\"/>\n", w - mr + 14, ly, pal[s % 8]
        printf "<text x=\"%d\" y=\"%d\" font-size=\"12\" fill=\"#222\">%s</text>\n", w - mr + 34, ly + 10, esc(sname[s])
    }
    print "</svg>"
}
