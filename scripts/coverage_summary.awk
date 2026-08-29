# coverage_summary.awk — normalise and summarise an LCOV report.
#
# APPROVED-CORE-EDIT: unified-ir-m2b-vv-core-consumers (2026-08-29).
#
# One tool, two modes, so the repository does not accumulate a second coverage
# script later (CLAUDE.md §6.1). Sits beside scripts/tsv_to_svg.awk, the
# existing reusable reporting helper.
#
#   Normalise (strip machine-specific absolute paths, emit LCOV to stdout):
#     awk -v mode=normalize -f scripts/coverage_summary.awk in.lcov > out.lcov
#
#   Summarise (grouped line/function/branch totals):
#     awk -v mode=summary [-v group=<regex>] -f scripts/coverage_summary.awk in.lcov
#
# Path normalisation rewrites any absolute path to the portion at and after the
# repository root marker, so a committed artifact is reproducible on any
# machine. Backslashes become forward slashes.
#
# Branch columns are reported verbatim from the LCOV. LLVM source-based
# coverage emits BRF:0/BRH:0 unless `cargo llvm-cov --branch` is used, and that
# flag is unstable (nightly-only). On a stable toolchain the branch columns are
# therefore structurally zero, NOT "zero branches covered" — the summary prints
# `n/a` rather than 0% so the distinction is not lost.

BEGIN {
    if (mode == "") mode = "summary"
    if (root == "") root = "core"
}

function norm(p,   i, n, parts, out, seen) {
    gsub(/\\/, "/", p)
    # keep everything at and after the last occurrence of the root marker
    n = split(p, parts, "/")
    out = ""
    seen = 0
    for (i = 1; i <= n; i++) {
        if (!seen && parts[i] == root) seen = 1
        if (seen) out = (out == "" ? parts[i] : out "/" parts[i])
    }
    return (out == "" ? p : out)
}

function classify(p) {
    if (p ~ /\/tests\//)    return "test"
    if (p ~ /\/benches\//)  return "bench"
    if (p ~ /\/examples\//) return "example"
    if (p ~ /\/target\//)   return "generated"
    if (p ~ /\/vendor\//)   return "vendor"
    if (p ~ /\/src\//)      return "production"
    return "other"
}

/^SF:/ {
    raw = substr($0, 4)
    file = norm(raw)
    cls  = classify(file)
    if (mode == "normalize") print "SF:" file
    next
}

mode == "normalize" { print; next }

/^LF:/ { lf = substr($0, 4) + 0; next }
/^LH:/ { lh = substr($0, 4) + 0; next }
/^FNF:/ { fnf = substr($0, 5) + 0; next }
/^FNH:/ { fnh = substr($0, 5) + 0; next }
/^BRF:/ { brf = substr($0, 5) + 0; next }
/^BRH:/ { brh = substr($0, 5) + 0; next }

/^end_of_record/ {
    CLF[cls] += lf; CLH[cls] += lh
    CFF[cls] += fnf; CFH[cls] += fnh
    CBF[cls] += brf; CBH[cls] += brh
    classes[cls] = 1

    if (group != "" && file ~ group) {
        GLF += lf; GLH += lh; GFF += fnf; GFH += fnh; GBF += brf; GBH += brh
        gfiles++
    }
    # per-file production detail, for touched-file reporting
    if (cls == "production") {
        FLF[file] = lf; FLH[file] = lh; FFF[file] = fnf; FFH[file] = fnh
    }
    lf = lh = fnf = fnh = brf = brh = 0
    next
}

function pct(h, f) { return (f == 0 ? "n/a" : sprintf("%.2f%%", 100.0 * h / f)) }

END {
    if (mode == "normalize") exit
    printf "%-12s %8s %8s %9s  %8s %8s %9s  %8s\n",
           "CATEGORY", "LINES", "HIT", "LINE%", "FNS", "HIT", "FN%", "BRANCH%"
    for (c in classes) {
        printf "%-12s %8d %8d %9s  %8d %8d %9s  %8s\n",
               c, CLF[c], CLH[c], pct(CLH[c], CLF[c]),
               CFF[c], CFH[c], pct(CFH[c], CFF[c]),
               pct(CBH[c], CBF[c])
    }
    if (group != "") {
        printf "\n%-12s %8d %8d %9s  %8d %8d %9s  %8s   (%d files, /%s/)\n",
               "GROUP", GLF, GLH, pct(GLH, GLF), GFF, GFH, pct(GFH, GFF),
               pct(GBH, GBF), gfiles, group
    }
    if (perfile == "1") {
        printf "\n%-58s %7s %7s %9s\n", "PRODUCTION FILE", "LINES", "HIT", "LINE%"
        for (f in FLF)
            printf "%-58s %7d %7d %9s\n", f, FLF[f], FLH[f], pct(FLH[f], FLF[f])
    }
}
