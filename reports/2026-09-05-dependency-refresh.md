# Függőségfrissítés: konkrét következő változás

Created-at: 2026-09-05 20:11 +02:00. Állapot: **Core dependency-jóváhagyásra kész
javaslat, a patch nincs alkalmazva**. Aktív Core: `D:/hnnwork/distributed/core`,
`audit/core-distributed-semantics-2026-09-05`, alap `7186b55`. A provider frissítése
elkészült `d02aa614`-re; [külön jelentés](2026-09-05-contract-provider-refresh.md).

## A legkisebb védett változás

Egy fájl: `Cargo.lock`, a Parser Git-source commitja:

- régi: `e28a205147c48273980e63aa3338ca286974a6ed`;
- friss `origin/master`: `284c490d68232973a006bcba974279fed6f89ce8`.

[Alkalmazható patch](dependency-refresh/core-parser.patch),
[előzetes terv](../docs/plans/2026-09-05-dependency-refresh/plan.pdf).
A Parser két commitjának diffje kizárólag saját `Cargo.lock` hozzáadása, 987 sor.
A Parser forrása és manifestje byteazonos; a consumer továbbra is saját lockból
old fel. A Core root `Cargo.toml` meglévő `branch = "master"` deklarációja megfelelő,
és az URL is megmarad. Nincs grammar-, API-, IR-, wire- vagy fixture-változás.

Az eredeti feladat „Védett Core-határ” része kifejezetten előírja az alkalmazható
literal tokent a dependency-felületre; a [Core manifest](../CORE.YAML) `approval`
szakasza és Parser-bejegyzése ugyanezt rögzíti. Az admission-javítás korábbi
engedélye erre nem terjed ki. A javasolt token, amely még nem érkezett meg:

`APPROVED-CORE-EDIT: core-parser-latest-revision`

Jóváhagyás után Cargo pontos `--precise` frissítése, az egyetlen várt sorváltozás
ellenőrzése, majd az Amaterasu meglévő tiszta Core SDK `all` ellenőrzése következik.
Elvárt: a teljes Core-suite, kilenc admission-regresszió, CBOR corpus, fmt, Clippy
és benchmark smoke sikeres; egy Parser-source a feloldott gráfban. 16 GiB,
hozzáadott swap nélkül, 6 CPU. A Lean-forrás változatlan, a meglévő 19 tétel
bizonyítéka továbbra is használható. Rollback: csak az új dependency-commit
visszavonása az előző Cargo.lock revízióra. A patch előzetes `git apply --check`
ellenőrzése nem helyettesíti a majdani buildet és teszteket.

Előzetes `git apply --check`: PASS; a Core lockfile változatlan. A terv PDF-je
sikeresen lefordult. [Ellenőrzési adatok](dependency-refresh/verification.json),
[teljes előkészítő fájllista és sorváltozások](dependency-refresh/artifact-manifest.json).
A manifest a `7186b55` alaphoz képest tartalmazza e jóváhagyási csomagot, önmaga
kivételével. A függőségfrissítés buildje és fogyasztói tesztjei még nem futottak.

## Friss tulajdonosi állapot és fogyasztói sorrend

A read-only, független leltár sikeres fetchjei 19:39:42–46 között történtek;
Parser újra 20:09-kor, Core 20:10-kor fetchelve. Egyik felsorolt távoli repo sem
hirdetett taget. Az alábbiak megfigyelt revíziók, nem már végrehajtott migrációk.

| Tulajdonos | Friss default branch | Meglévő auditállapot |
|---|---|---|
| Parser | master `284c490d` | azonos |
| Tensor | main `b1d869d2` | azonos |
| Core | main `d6be4ea7` | `7186b55`, előtte 26, mögötte 0 |
| Graph | main `b234c487` | `94efe354`, előtte 2, mögötte 0 |
| Transformation | main `0d78d403` | `1d7b813b`, előtte 4, mögötte 0 |
| Hotaru | main `21017e9b` | azonos |
| Network | main `8643a5a1` | azonos |
| VSCode/LSP | main `19b82690` | azonos |

`FINAL_CORE` a Parser-frissítés után ténylegesen tesztelt Core-commit lesz. Minden
közvetlen fogyasztó ugyanazt a Git URL + `rev` kombinációt használja; a különböző
branch/rev/URL source-azonosságok közé nem kerülnek típuskonverziós kerülőutak.

| Checkout | Következő fájlváltozások |
|---|---|
| `D:/hnnwork/platform-audit/graph` | root `Cargo.toml` Core-ref, `Cargo.lock`, külön `benchmarks/Cargo.lock` |
| `D:/hnnwork/platform-audit/transformation` | root `Cargo.toml` és `Cargo.lock`; ebből `FINAL_TRANSFORMATION` |
| `D:/hakiko_ai_ws/03_implementation/hymeko_framework/hotaru` | root `Cargo.toml`, `Cargo.lock`; ebből `FINAL_HOTARU` |
| `D:/hakiko_ai_ws/03_implementation/hymeko_framework/network` | root `Cargo.toml`, `Cargo.lock`: mindhárom szolgáltatói revízió |
| `D:/hakiko_ai_ws/03_implementation/hymeko_framework/vscode-plugin/server` | `Cargo.toml`, `Cargo.lock`: ugyanaz a három szolgáltatói revízió |
| `D:/hnnwork/platform-audit/integration/work/hymeko_simulator` | root `Cargo.toml`, `Cargo.lock`: Core path/patch kiváltása, minden Transformation-crate együtt frissítve |

A Graph dependency-felülete explicit védett, de saját manifestje nem követel
literal tokenformátumot. A felhasználó konkrét „Graph és hasonló csomagok a
legfrissebbre” utasítása ehhez a dependency-only scope-hoz rögzíthető; ez nem
algoritmus-, API- vagy P34-engedély. A Core fenti külön literal feltétele megmarad.

A külön Graph benchmark-workspace lockja sem maradhat régin. Minden érintett
workspace `cargo tree --locked -d` ellenőrzést kap. Graph: `informed_search_vv`,
`direct_ir_optional` és a külön benchmark fordítása. Simulator: a meglévő
`model_ingress` suite `zenoh-ingress` feature-rel, majd a támogatott WASM build.
A Simulator/Chordex/XR integrációs snapshot repókhoz nincs remote: helyi eredményből
nem állítunk távoli elérhetőséget. A provider ecoread-frissítése külön owner-scope;
ott egy nem követett lock felvétele nem része ennek a Core-javaslatnak.

## P34 és tagek

A távoli `feat/codex-p34-independent-audit` (`83774b8b`) a Graph main előtt 49
committal jár. Nem tartalmazza teljesen a két platformjavítást: hiányzik az
`entry_independent()`/`arc_entry_g` feltétel, és eltér a lezárt frontier Depth
jelzése. A P34 saját goal-pruningját és cycle-dedupját sem szabad felülírni egy
teljes fájlcserével. Bevonása továbbra is a már feltett scope-kérdéstől függ;
nem kezelhető egyszerű dependency-bumpként.

A felhasználó kért sorrendje: friss tulajdonosok, összehangolt fogyasztók,
ellenőrzés, utána tagek. A tagek nem bizonyítják a tesztelést, és egy helyi
provider-SHA nem válik automatikusan a fogyasztó távoli forrásából elérhetővé.
Push/main-merge/release nincs végrehajtva; a kiadható revíziók elérhetőségét és a
tagneveket a konkrét, tesztelt változatsor ismeretében kell lezárni.

## Discovery

```text
DISCOVERY LEDGER — dependency-refresh jóváhagyási csomag
  find:    Test-Path docs/plans/2026-09-05-dependency-refresh és reports/2026-09-05-dependency-refresh.md -> 0 találat
  grep:    rg -n 'source =.*hymeko' Cargo.lock -> 1 Parser Git-revízió
  symbols: meglévő HymekoParser és RealParser; új publikus API nem szükséges
  read:    Core/Graph CORE.YAML, Cargo-manifestek, Parser commitdiff, Core Docker-terv
  verdict: NEW dependency-refresh terv és jelentés, alkalmazás nélküli Core-lock patch-csel
           rejected Core Docker-terv: annak scope-ja kifejezetten változatlan dependencyket ír elő
```
