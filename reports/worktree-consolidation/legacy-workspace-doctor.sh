# HISTORICAL EVIDENCE: original 2026-08-30 registry branch, superseded.
# Active repository ownership lives in hymeko/hymeko ecosystem.hymeko.
# Retained for this branch integration; not an active workspace configuration.
#!/usr/bin/env bash
# workspace doctor — verify the local checkout against the canonical WORKSPACE.yaml registry.
#
# Written 2026-08-30, after twelve of fourteen local checkouts turned out to be two-file stubs
# pointing at a dead host, which made an entire framework (hymeko/core: unified IR, hardware-tested
# WGPU backend) invisible to a session that then started designing a replacement for it.
#
# The doctor's job is to make that state IMPOSSIBLE TO MISS, not merely detectable.
#
# Exit codes
#   0  every check ran and passed  (and a WORKSPACE-DOCTOR-RAN receipt was printed)
#   1  a check ran and failed
#   2  the doctor could not run at all — missing manifest, no yq/python, unreadable registry
#
# There is no soft success: a check that did not run is exit 2, never 0. That distinction is the
# whole lesson of `check_contract_sync.sh`, which for eleven days reported "drift" for
# "could not resolve" and was green for both.
set -uo pipefail

# Usage: workspace_doctor.sh [MANIFEST] [--root WORKSPACE_ROOT]
#
# The manifest lives in TWO places by design -- `core/WORKSPACE.yaml` (canonical, tracked) and
# `<root>/WORKSPACE.yaml` (local mirror) -- so the workspace root cannot be derived from the
# manifest's own directory. It is resolved explicitly, or probed and VALIDATED below; a wrong root
# would report every repository missing, which reads like catastrophe and means nothing.
MANIFEST="${1:-$(dirname "${BASH_SOURCE[0]}")/../WORKSPACE.yaml}"
EXPLICIT_ROOT=""
[[ "${2:-}" == "--root" ]] && EXPLICIT_ROOT="${3:-}"
FORBIDDEN_HOST='192.168.89.249'
CANON_HOST='192.168.1.228'

die() { echo "workspace-doctor: $*" >&2; exit 2; }
command -v python3 >/dev/null || die "python3 is required to read the manifest"
[[ -f "$MANIFEST" ]] || die "manifest not found: $MANIFEST"

MDIR="$(cd "$(dirname "$MANIFEST")" && pwd -P)"
if [[ -n "$EXPLICIT_ROOT" ]]; then
  [[ -d "$EXPLICIT_ROOT" ]] || die "--root $EXPLICIT_ROOT is not a directory"
  ROOT="$(cd "$EXPLICIT_ROOT" && pwd -P)"
else
  # Probe the manifest's directory and its parent; take whichever actually contains the
  # repositories, and fail loudly if neither does rather than reporting a phantom empty workspace.
  ROOT=""
  for cand in "$MDIR" "$(dirname "$MDIR")"; do
    n=0
    for probe in core tensor nagare; do [[ -d "$cand/$probe/.git" ]] && n=$((n+1)); done
    if [[ "$n" -ge 2 ]]; then ROOT="$cand"; break; fi
  done
  [[ -n "$ROOT" ]] || die "cannot locate the workspace root from $MDIR; pass --root explicitly"
fi
echo "workspace root: $ROOT"
FAIL=0; CHECKS=0
ok()   { CHECKS=$((CHECKS+1)); printf '  ok    %s\n' "$*"; }
bad()  { CHECKS=$((CHECKS+1)); FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$*"; }

# The manifest is the only source of the repo list. Hardcoding it here would let the two drift.
mapfile -t REPOS < <(python3 - "$MANIFEST" <<'PY'
import sys,re
txt=open(sys.argv[1]).read()
# Deliberately a narrow reader, not a YAML engine: the doctor must not acquire a dependency to
# check that dependencies are correct.
sec=txt.split("repositories:",1)[1].split("\ndependency_edges:",1)[0]
for m in re.finditer(r"^  ([a-z0-9][a-z0-9_-]*):", sec, re.M):
    name=m.group(1)
    tail=sec[m.end():m.end()+400]
    lp=re.search(r"local_path:\s*([^\s,}]+)", tail)
    db=re.search(r"default_branch:\s*([^\s,}]+)", tail)
    ov=re.search(r"clone_path_override:\s*\"([^\"]+)\"", tail)
    print(f"{name}\t{lp.group(1) if lp else name}\t{db.group(1) if db else 'main'}\t{ov.group(1) if ov else ''}")
PY
) || die "could not parse the manifest"
[[ ${#REPOS[@]} -gt 0 ]] || die "the manifest lists no repositories; refusing to pass vacuously"

echo "== workspace doctor: ${#REPOS[@]} repositories from $MANIFEST =="

for row in "${REPOS[@]}"; do
  IFS=$'\t' read -r name path branch override <<<"$row"
  dir="$ROOT/$path"
  if [[ ! -d "$dir/.git" ]]; then bad "$name: no checkout at $path"; continue; fi
  url="$(git -C "$dir" remote get-url origin 2>/dev/null || echo '')"
  if [[ -z "$url" ]]; then bad "$name: no origin remote"; continue; fi
  if [[ "$url" == *"$FORBIDDEN_HOST"* ]]; then
    bad "$name: origin points at the DEAD host $FORBIDDEN_HOST — this is what hides whole frameworks"
    continue
  fi
  [[ "$url" == *"$CANON_HOST"* ]] || { bad "$name: origin host is not $CANON_HOST ($url)"; continue; }
  if [[ -n "$override" ]]; then
    [[ "$url" == "$override" ]] || { bad "$name: origin does not match its declared clone_path_override ($url)"; continue; }
  else
    [[ "$url" == *"/hymeko/$name.git" ]] || { bad "$name: origin path is not /hymeko/$name.git ($url)"; continue; }
  fi
  if ! git ls-remote --heads "$url" "refs/heads/$branch" 2>/dev/null | grep -q "refs/heads/$branch"; then
    # A repository with NO branches at all is an empty upstream, which is a legitimate state for a
    # declared scaffold. It is VERIFIED here rather than taken from the manifest, so the
    # declaration cannot be used to hide a repository that merely lost its branch.
    if [[ -z "$(git ls-remote --heads "$url" 2>/dev/null)" ]]; then
      ok "$name: upstream has no branches at all (empty scaffold, verified)"
      continue
    fi
    bad "$name: canonical branch '$branch' not reachable, but the upstream HAS branches"
    continue
  fi
  ok "$name: origin canonical, branch '$branch' reachable"
done

# An owner named by the manifest must actually be findable, or the manifest is fiction.
echo "== ownership spot checks =="
if [[ -d "$ROOT/core" ]]; then
  if git -C "$ROOT/core" cat-file -e origin/main:hymeko_core/src/ir/ir.rs 2>/dev/null; then
    ok "core: the IR owner (hymeko_core/src/ir/ir.rs) exists on origin/main"
  else
    bad "core: the manifest says core owns the IR, but hymeko_core/src/ir/ir.rs is not on origin/main"
  fi
  if git -C "$ROOT/core" cat-file -e origin/main:hymeko_wgpu/Cargo.toml 2>/dev/null; then
    ok "core: the GPU lowering owner (hymeko_wgpu) exists on origin/main"
  else
    bad "core: the manifest says core owns GPU lowering, but hymeko_wgpu is not on origin/main"
  fi
else
  bad "core: no checkout — the IR owner cannot be verified"
fi

# The manifest must not contradict a repository's own CORE.YAML about being core.
echo "== manifest vs CORE.YAML =="
for name in core tensor; do
  d="$ROOT/$name"
  [[ -d "$d/.git" ]] || { bad "$name: no checkout"; continue; }
  if git -C "$d" cat-file -e "origin/$( [[ $name == parser ]] && echo master || echo main ):CORE.YAML" 2>/dev/null; then
    ok "$name: declares a CORE.YAML, consistent with status: CORE"
  else
    bad "$name: manifest says status CORE but the repo has no CORE.YAML"
  fi
done

# --- exactly one package named hymeko_graph ------------------------------------------------------
# The durable property the 2026-08-30 consolidation exists to establish. Counted over the CANONICAL
# TIP of every repository, not the working tree: a duplicate that survives on the default branch is
# a duplicate a fresh clone gets.
echo "== single-owner: hymeko_graph =="
declare -a HG_OWNERS=()
for row in "${REPOS[@]}"; do
  IFS=$'\t' read -r name path branch override <<<"$row"
  dir="$ROOT/$path"
  [[ -d "$dir/.git" ]] || continue
  while read -r m; do
    [[ -z "$m" ]] && continue
    if git -C "$dir" show "origin/$branch:$m" 2>/dev/null | grep -qE '^name = "hymeko_graph"'; then
      HG_OWNERS+=("$name/${m%/Cargo.toml}")
    fi
  done < <(git -C "$dir" ls-tree -r --name-only "origin/$branch" 2>/dev/null | grep 'Cargo.toml$' | grep -v '^docs/')
done
if [[ ${#HG_OWNERS[@]} -eq 1 ]]; then
  if [[ "${HG_OWNERS[0]}" == "graph/hymeko_graph" ]]; then
    ok "exactly one hymeko_graph package, and it is graph/hymeko_graph"
  else
    bad "exactly one hymeko_graph package but it is ${HG_OWNERS[0]}, not graph/hymeko_graph"
  fi
elif [[ ${#HG_OWNERS[@]} -eq 0 ]]; then
  bad "NO package named hymeko_graph found on any canonical tip -- the census is vacuous or the owner vanished"
else
  bad "${#HG_OWNERS[@]} packages named hymeko_graph: ${HG_OWNERS[*]} (canonical is graph/hymeko_graph)"
fi

# A RENAMED duplicate is invisible to a package-name census -- the first version of this check
# passed a fixture package called `hymeko_graph_compat`, which is exactly the compatibility copy
# `graph/CLAUDE.md` forbids. Two cheaper-than-perfect detectors close it:
#   1. `[lib] name = "hymeko_graph"` under any package name -- the linkage name is what a consumer
#      actually writes in `use`, so a rename that keeps it is a duplicate in every sense that
#      matters.
#   2. the owner's module fingerprint: a crate outside graph carrying signed_tuples.rs AND spine.rs
#      AND topk_cycles.rs is the graph owner under another badge.
for row in "${REPOS[@]}"; do
  IFS=$'\t' read -r name path branch override <<<"$row"
  dir="$ROOT/$path"
  [[ -d "$dir/.git" ]] || continue
  [[ "$name" == "graph" ]] && continue
  while read -r m; do
    [[ -z "$m" ]] && continue
    if git -C "$dir" show "origin/$branch:$m" 2>/dev/null | grep -qE '^name = "hymeko_graph"$' ; then
      pkg=$(git -C "$dir" show "origin/$branch:$m" 2>/dev/null | grep -m1 -E '^name = ' | sed 's/name = //;s/"//g')
      [[ "$pkg" == "hymeko_graph" ]] || bad "$name: package '$pkg' links as hymeko_graph -- a renamed duplicate"
    fi
  done < <(git -C "$dir" ls-tree -r --name-only "origin/$branch" 2>/dev/null | grep 'Cargo.toml$' | grep -v '^docs/')
  fp=$(git -C "$dir" ls-tree -r --name-only "origin/$branch" 2>/dev/null | grep -v '^docs/' \
       | grep -cE '/(signed_tuples|spine|topk_cycles)\.rs$')
  [[ "${fp:-0}" -ge 3 ]] && bad "$name: carries the graph owner's module fingerprint (signed_tuples + spine + topk_cycles) outside graph"
done

# No consumer may still resolve the retired copy by path.
for row in "${REPOS[@]}"; do
  IFS=$'\t' read -r name path branch override <<<"$row"
  dir="$ROOT/$path"
  [[ -d "$dir/.git" ]] || continue
  # `docs/` is excluded deliberately: graph commits the two-implementation parity harness there as
  # SOURCE, not as a workspace member. It is documentation of how the comparison was run, it is
  # never built by the workspace, and counting it as a live dependency is a false positive the
  # first version of this check produced.
  hits=$(git -C "$dir" grep -lE 'path *= *"[^"]*tensor/hymeko_graph"' "origin/$branch" -- '*Cargo.toml' 2>/dev/null | grep -v ':docs/' || true)
  if [[ -n "$hits" ]]; then
    bad "$name@$branch still declares a path dependency on tensor/hymeko_graph: $(tr '\n' ' ' <<<"$hits")"
  fi
done

echo "──────────────────────────────────────"
if [[ "$CHECKS" -eq 0 ]]; then
  die "no check executed; a doctor that checks nothing must never report health"
fi
printf 'WORKSPACE-DOCTOR-RAN checks=%d failures=%d manifest=%s\n' "$CHECKS" "$FAIL" "$MANIFEST"
[[ "$FAIL" -eq 0 ]] || exit 1
exit 0
