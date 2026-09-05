# Core: a friss közös agent-contract átvétele

Created-at: 2026-09-05 20:04 +02:00. Tulajdonos: `hymeko/core`, consumer szerep.
Munkafa: `D:/hnnwork/distributed/core`, ág:
`audit/core-distributed-semantics-2026-09-05`; migrációs alap: `5887d07`.

A felhasználó kérte a friss `hymeko/hymeko` alkalmazását. A végső provider-revízió
**`d02aa614bc06a348dbcbe3936dcd3b9285b8bdeb`**. A kezdeti `5517e6b` ellenőrzése
közben a távoli main frissült; a 20:01-es sikeres fetch után a saját, tiszta sibling
checkout fast-forwarddal frissült. Az új, feladatfolytonossági és befejezési szabályokat
teljes egészükben beolvastam. A provider forrását nem módosítottam.

## Átvett működés

- A Core saját `AGENTS.md` belépési pontot kapott, kötelező tényleges fájlolvasással.
- A `CLAUDE.md` elején valódi `@AGENTS.md` és
  `@../hymeko/docs/agent-contract/global.md` import szerepel. A tárgymodulok közvetlenül
  a providerre hivatkoznak. A módosítás előtti fetch és a meglévő munkafák újrahasználata
  a közös szabály része.
- A közös preambulum, persona és betöltési útmutató frissült; a Core saját védelmi,
  erőforrás- és authoring-szabályai megmaradtak. A helyi régi közös modulok történeti
  snapshotok, amit a README egyértelműen jelöl.
- A meglévő Git-hookok, CI-sablon, toolchain dokumentum és új Claude Stop-konfiguráció
  ugyanazt a teljes provider-revíziót használják. A tooling továbbra is a providerben
  él; a consumer nem kapott saját implementációt vagy provider-only self-checket.
- A telepített régi hookok egyezését ellenőriztem az átírás előtt. A meglévő installer
  telepítette a végső revíziót; mindkét hook `installed, current`, a Python-disztribúció
  `direct_url.json` bejegyzése ugyanazt a commitot igazolja.

A Core-t és a providert 19:53-kor sikeresen fetcheltem. A Core ekkor 24 committal
járt a friss `origin/main` (`d6be4ea7`) előtt, lemaradás nélkül. A saját provider
checkoutot `D:/hnnwork/distributed/provider` helyről a szabványos sibling
`D:/hnnwork/distributed/hymeko` névre mozgattam, tisztaság- és útvonalhatár-ellenőrzés
után. Nem jött létre újabb munkafa. A korábbi fejlesztői checkout fájljai érintetlenek;
a Core worktree-k közös Git-hook könyvtárát az installer frissítette.

## Ellenőrzés

[Végső gépi jegyzőkönyv](contract-provider-refresh/final-verification.json): pontos
parancsok, kilépési kódok, nyers kimenetek és telepítési proveniencia. A korábbi
`5517e6b` próbák külön [közbenső jegyzőkönyvben](contract-provider-refresh/verification.json)
maradtak, nem lettek az új verzió bizonyítékaként átnevezve.

| Ellenőrzés | Eredmény | Végső diagnosztikai idő |
|---|---|---:|
| Rögzített verziójú symbol check | PASS, 5 csoport / 614 szimbólum | 1,092 s |
| Rögzített verziójú duplication gate | PASS | 2,083 s |
| Commitüzenet `DISCOVERY:` nélkül | elutasítva, várt exit 1 | 3,188 s |
| Commitüzenet indokolt `DISCOVERY:` sorral | engedve, exit 0 | 1,558 s |
| Teljes pre-commit | funkcionális PASS | 3,517 s |
| Telepített hookok státusza | mindkettő current | 0,242 s |
| Persona: érvényes / hibás / ismétlésvédett bemenet | 3/3 várt döntés | nem benchmark |
| Startup fejléc, UTF-8, importcélok, JSON, egyező pinek | PASS | statikus ellenőrzés |

Clone-delta: `duplication (tokens): 402 clone(s), 7769 cloned line(s) across
188 file(s); baseline 403 / 7800`. A baseline nem változott. Az indexet az előző
`5887d07` bizonyítékcommit generálta újra: a virtuális Cargo-workspace nem kap hamis
csomagtulajdont, a két tényleges Rust crate megmarad, a reports Python és Rust
csoportja különválik. A `docs`/`reports` inferred csoportoknak nincs csomagmanifestjük
vagy charterük; ez nem új kiadható crate-ek létrehozása. A Core 418, a WGPU 195,
a reports Python 1 szimbólumot tartalmaz. A portable gate-források a két provider
revízió között byteazonosak; a legújabb változás a közös szabály és provider self-check.

A preflight próbái a ténylegesen hozzáadott migrációs fájlokat használták; a két
ideiglenes commitüzenet a repo mellett nem maradt meg. Nem készült dummy commit,
nem lett kikapcsolva hook és nem emelkedett a duplikációs küszöb.

## Erőforrások és korlátok

A 2 másodperces hook-cél **nem teljesült**: közbenső 2,778 s, végső 3,517 s.
Ezek eltérő méretű staged fájlkészleten futó egyszeri diagnosztikai adatok, nem
benchmark vagy bizonyított eszközregresszió. A portable források változatlanok;
ez a jelentés nem állít gyorsulást, és nem módosítja a gate-et mérés nélkül.

A meglévő `scripts/dev/measure_peak_memory.ps1` guarddal, közvetlen alap-Python
folyamaton, a telepített provider site-packages használatával: symbol check
0,414 s / 30,371 MiB; duplication 1,471 s / 93,707 MiB. Mindkettő a 2048 MiB
mintavételes határ alatt. [Symbol memóriaadat](contract-provider-refresh/symbols-direct.memory.json),
[duplication memóriaadat](contract-provider-refresh/duplication-direct.memory.json).
Az első, `*-direct` nélküli logpár csak a Windows venv-launcher kb. 4 MiB-ját mérte;
az nem a teljes Python munka memóriaadata. A korrekció oka ezért explicit.
Host: Windows, Python 3.14.7, a már meglévő uv eszközkörnyezet. A terv PDF-jét
az Amaterasu meglévő pdflatex eszköze készítette.

A külön Claude Code munkamenet `/memory`, `/status`, `/hooks` és projekt-trust
állapota nem lett ellenőrizve. A konfiguráció és a parancs valódi bemenete tesztelt,
a szemantikus prompt modelloldali aktiválása nem bizonyított. Távoli CI-futás sincs
állítva. A globális installed-entry-point fast path más repo telepítésével később
eltérhet a pintől; a telepítési proveniencia jelenleg ellenőrzött. Nem készült új
globális hooklogika e migrációban.

## Hatókör, fájlok és folytatás

Védett Core-fájl, Cargo-manifest/lock, fixture, API, IR vagy wire-szemantika nem
változott. A runtime marad a már ellenőrzött `8a0c49d`;
[Core/Lean/Docker bizonyíték](2026-09-05-distributed-admission-fix.md).
Nem kellett új runtime tesztet futtatni a dokumentum- és toolingváltozások miatt.

[Terv](../docs/plans/2026-09-05-contract-provider-refresh/plan.pdf),
[teljes fájllista, sorváltozások és SHA256](contract-provider-refresh/artifact-manifest.json).
A manifest a `5887d07` alaphoz képest az átvezetés összes fájlját sorolja, önmaga
kivételével. Az index külön, az előző commitban szerepel, a fenti indoklással.

Rollback: csak a consumer-migráció commitjának visszavonása és az előző tracked
hookok telepítése, a közös hookkönyvtár aktuális idegen változásainak egyeztetésével.
Nincs push, main-merge vagy tag. A felhasználó kért függőségfrissítési, majd tagelési
sorrendje megmarad; a Core Parser-frissítéséhez az eredeti feladat kifejezett
Core-jóváhagyási határát kell alkalmazni. A Graph távoli P34 auditágának bevonása
külön scope-döntés a friss default mainhez képest.

## Discovery

```text
DISCOVERY LEDGER — Core contract refresh
  find:    rg --files docs/plans reports tools -> meglévő migrációs jelentések és tools/hooks/
  grep:    rg -n 'contract|preflight' tools/hooks .github/workflows -> meglévő három hook és CI-konfiguráció
  symbols: rg -n 'contract|preflight|AuditSummary' SYMBOLS.md -> nincs contract-tool implementáció
  read:    CLAUDE.md, CORE.YAML, hookok, provider ROLLOUT/MIGRATION és közös modulok
  verdict: EXTEND CLAUDE.md, tools/hooks/, meglévő index és CI
           NEW AGENTS.md, .claude/settings.json; a meglévő CLAUDE.md önmagában nem ad Codex-belépési pontot vagy Stop-hookot
           NEW dátumozott terv és jelentés; a tegnapi migráció történeti bizonyíték
```
