# Függőségfrissítés: konkrét következő változás

Created-at: 2026-09-05 20:11 +02:00. Updated-at: 2026-09-05 20:56 +02:00.
Állapot: **a jóváhagyott Core Parser-frissítés elkészült és tiszta Docker-képben
ellenőrzött**; a fogyasztói revíziók összehangolása következik. Aktív Core: `D:/hnnwork/distributed/core`,
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
engedélye erre nem terjedt ki. A felhasználó a következő külön tokent megadta;
a jóváhagyott végrehajtás 20:45-kor folytatódott:

`APPROVED-CORE-EDIT: core-parser-latest-revision`

Cargo pontos `--precise` frissítése után az egyetlen várt sorváltozást ellenőriztem,
majd az Amaterasu meglévő tiszta Core SDK `all` ellenőrzését lefuttattam.
A teljes Core-suite, kilenc admission-regresszió, CBOR corpus, fmt, Clippy
és benchmark smoke sikeres; egy Parser-source szerepel a feloldott gráfban.
16 GiB, hozzáadott swap nélkül, 6 CPU. A Lean-forrás változatlan, a meglévő 19 tétel
bizonyítéka továbbra is használható; nem futott újabb, indokolatlan Lean-mérés.
Rollback: csak az új dependency-commit
visszavonása az előző Cargo.lock revízióra. A patch előzetes `git apply --check`
ellenőrzése nem helyettesíti a majdani buildet és teszteket.

Az előzetes `git apply --check`: PASS volt, a Core lockfile akkor változatlan. A terv PDF-je
sikeresen lefordult. [Ellenőrzési adatok](dependency-refresh/verification.json),
[teljes előkészítő fájllista és sorváltozások](dependency-refresh/artifact-manifest.json).
A manifest a `7186b55` alaphoz képest tartalmazza e jóváhagyási csomagot, önmaga
kivételével, a `0c4e40a` történeti revízióban. Az előzetes `verification.json`
ennek a jóváhagyás előtti állapotnak a bizonyítéka, nem a mostani státusz.

## Jóváhagyott végrehajtás és mérési bizonyíték

Implementáció: **`1886f17a42d7a3430e4044129f81955615efd583`**. A token a
commit footerében és az implementáció előtt frissített, lefordított tervben szerepel.
A Core `Cargo.lock` pontosan a tervezett egyetlen Parser-source sort változtatja.
A Rust források, fixture-ek és az összes registry-függőség megmaradtak.

Az első `cargo update -p parser --precise 284c490d68232973a006bcba974279fed6f89ce8`
a `winapi-util 0.1.11` `windows-sys` élét is újraválasztotta 0.61.2-ről 0.48.0-ra.
A crate manifestje mindkettőt megengedi (`>=0.48.0, <=0.61.*`). A nem kért
resolver-változást visszaállítottam az eredeti 0.61.2-re; a `--locked --offline`
fa és az új Linux-kép feloldása így is sikeres. [Kezdeti delta](dependency-refresh/cargo-update-initial.patch),
[Cargo-log](dependency-refresh/cargo-update.log),
[pontos változási ellenőrzés](dependency-refresh/implementation-check.json).

Az új kép az előző, tiszta Amaterasu checkoutot használta újra, ellenőrzött
`8a0c49d` alapról Git bundle-lel fast-forwardolva. Bundle SHA256:
`43eb5ca28149989ce3a3676d7ca13b2931a22469423dd3771dc2497204b6a380`.
Az előző kép és mérési logok megmaradtak. A build ugyanazt a korlátozott buildert
és csak a build idejére forwardolt SSH-agentet használta.

Kép: `hymeko-core:1886f17a42d7a3430e4044129f81955615efd583`;
image ID: `sha256:c825ade5a9b80d45648ef5175398707cffe5b028b251015d69c39b9abc8a1308`.
Konténer: `hymeko-core-parser-20260905`, befejezett; új, kezdetben üres target volume:
`hymeko-core-parser-target-20260905`. Nincs source bind mount vagy hálózat a futásban.
Az image lockfile SHA256-ja megegyezik a Git-forrással:
`4871f04cb0c24865c17ddf4507f83136c1c70caffeac6eaf27fc94acd075056c`.

[Teljes nyers log](dependency-refresh/core-all.log),
[gépi eredmények](dependency-refresh/execution-results.json),
[konténerállapot](dependency-refresh/container-final.json),
[image metadata](dependency-refresh/image-build.json),
[feloldott csomaggráf](dependency-refresh/image-metadata.json).

| Ellenőrzés | Eredmény | Diagnosztikai idő / peak RSS |
|---|---|---|
| Teljes Core release-suite | 366 PASS / 7 ignored | 21,31 s / 3068780 KiB |
| Kilenc admission-regresszió | 9 PASS, a teljes suite részeként | nem külön mérés |
| CBOR corpus | 1 PASS, külön smoke-futás | 15,92 s / 433996 KiB |
| Workspace fmt | PASS | 0,33 s / 52484 KiB |
| Workspace/all-target Clippy | PASS, warnings denied | 22,19 s / 816020 KiB |
| CBOR encode/decode, három méret | 6 smoke-eset PASS | 4,60 s / 382204 KiB |
| Core CLI | PASS | 19,27 s / 508400 KiB |

A teljes konténer 83,941 s alatt végzett, exit 0, OOM=false. Cgroup-memóriacsúcs:
3713777664 bájt, kb. 3,46 GiB; ez page cache-t is tartalmaz, nem process RSS.
A korábbi futás 82,223 s / 3685134336 bájt volt. Az ábra egyszeri diagnosztikai
összehasonlítás, nem benchmark vagy gyorsulási állítás.

![Core konténerellenőrzés a Parser-revízió frissítése előtt és után](dependency-refresh/resources.svg)

Az ignored esetek változatlanok: assembler taxonomy-dump; négy CBOR memory/fixture
diagnosztika; corpus-lowering és incidence-vs-clique benchmark. Pontos neveik a
nyers logban szerepelnek. WGPU hardveres teszt, optional IPC/Arrow és fogyasztói
throughput-mérés nem része ennek a forrásazonos Parser-frissítésnek.

Reprodukció: a kép `all` belépési pontja a [Core container útmutató](../docs/core-container.md)
szerint, új üres target volume-mal. Az eredeti Rust 1.97.1 Bookworm SDK, hat CPU
és 16 GiB/no-swap limit megmaradt. A metadata egyetlen Core, WGPU és Parser
azonosságot igazol; ez még nem a későbbi fogyasztói gráf ellenőrzése.

[Végrehajtási fájlmanifest](dependency-refresh/execution-manifest.json): a
`0c4e40a` jóváhagyás előtti alaphoz mért teljes fájllista, sorváltozás és SHA256,
önmaga kivételével. A korábbi előkészítő manifestet nem írtam át.

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
