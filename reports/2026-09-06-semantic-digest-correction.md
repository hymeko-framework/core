# Core SemanticModelDigestV2 — alkalmazott és ellenőrzött javítás

Created-at: 2026-09-06 00:55 +02:00. Updated-at: 2026-09-06 02:29 +02:00.

A négy reprodukált digest-hiba javítva. A végleges Core-forrás Amaterasun
**29/29 célzott tesztet és 395 teljes Core-tesztet teljesített**, hét változatlan
ignored teszt mellett. A CBOR corpus, CLI, fmt, workspace Clippy és CBOR
benchmark-smoke is sikeres. **A Nagare célzott fogyasztói kapuja is teljesült:
14 PASS, 0 FAIL, 0 ignored**, a javított Core/WGPU `125472e` revízióval.
A mainre feltöltött roadmap-registryt beolvastuk; a korábbi hiányjelzést
helyesbítettük, lásd lent.

## Jóváhagyás és végrehajtási határ

A felhasználó ebben a beszélgetésben, az alkalmazás előtt megadta:

`APPROVED-CORE-EDIT: semantic-digest-canonical-coverage`

A chat-turn azonosító nem áll rendelkezésre. A token szerepel a frissített,
lefordított [tervben](../docs/plans/2026-09-06-semantic-digest-correction/plan.pdf),
a `CORE.YAML` bejegyzésében és a javítócommit footerében.

Alap: `f35c34fc887f706c24e449276ed59e85887f6da7`.
Kizárólag a `112a2543264e302839fd7f2cf04858fd098c8f5c` digest-változása kerül
be a javítással együtt, egy koherens commitban. Az eredeti import külön
publikálása ismert hibás köztes providert eredményezne. A portable-ID és a
robotfixture-őscommitok nem részei ennek a javításnak.

## Viselkedés és megőrzött szerződések

- Minden véges beágyazott érték teljes, iteratív kódolást kap; a 64-es mélységnél
  korábban elvágott levél módosítása most megváltoztatja a digestet.
- A névútvonal és az owner első arc-slotja adja a kanonikus identitást.
  A deklarációk és ArcRec rekordok globális tárolási sorrendje nem része ennek.
- Minden konkrét node-, edge- és arc-rekord tartalma bekerül, külön deklarációs
  kötés nélkül is. A record-only ellenpéldákat a produkciós CBOR encode/decode
  fogadja el; ezért nem lehet őket hibás bemenetként figyelmen kívül hagyni.
- Az arc-slotok sorrendje, multiplicitása és megosztása megmarad: azonos payload
  mellett is különbözik `[A,A,B]` és `[A,B,B]`. Az anonim névhelykitöltő kimarad.
- Ha a kanonikus identitás előfeltételei hiányoznak, a teljes modell külön raw-ID
  domaint használ. Ott is teljes a payload-lefedettség, de nincs átindexelési
  invarianciaígéret. Érvénytelen symbol/record index továbbra is pánikolhat;
  ez a modul nem új IR-validátor.

A bejövő V2 publikus szignatúrái változatlanok. Schema **2**, algoritmus **2**
(korábban 1); a régi V2 cache/plan azonosítók újraszámítandók. A strukturális V1,
a deprecated Merkle-függvény törzse, `Ir::decl_hash`, a CBOR wire, a Parser-lock,
a corpus-fixture-ök és a distributed admission forrása nem módosult.
A Core javításához új vagy frissített függőség nincs; a külön fogyasztói
ellenőrzés ideiglenes Nagare-lockjának eltérését lent rögzítjük.

A korábbi 19 Lean admission-tétel forrása byte-azonos maradt. Ez a javítás
nem bővíti az ottani, opaque identitásokra vonatkozó bizonyítást: új formális
digest- vagy kriptográfiai bizonyítást nem állítunk, Lean-újrafuttatás nem történt.

## Végleges ellenőrzés és provenance

A tesztelt forrásfa: `8a1cad6b64614a2dbff4dbe445b074eaf15b2b1c`.
Az archívum SHA256 értéke:
`5b62ba59ef3a808e3a1e1c9281ea317e8f54c8e3e881b33e0d4af31ecbfee0c3`.
Az [execution.json](semantic-digest-correction/execution.json) a nyolc
alkalmazási fájl pontos hashét, a parancsokat hordozó konténerállapotot,
tesztpopulációkat, ignored neveket és erőforrásadatokat rögzíti.
Az archívum az alap plusz e nyolc staged fájl; a későbbi jelentés/PDF-változás
nem a fordított forrás része. A végső commit fájljai ezekkel ellenőrizendők.

| Kapu | Eredmény | Falidő, fordítással együtt | Legnagyobb process RSS |
|---|---|---:|---:|
| Digest regressziók | 29 PASS, 0 FAIL | 8,25 s | 428 612 KiB |
| Teljes Core | 395 PASS, 7 ignored | 21,62 s | 3 068 524 KiB |
| CBOR corpus | 1 PASS | 8,29 s | 429 812 KiB |
| CLI smoke | PASS | 3,82 s | 376 964 KiB |
| fmt | PASS | 0,33 s | 52 952 KiB |
| workspace/all-targets Clippy, `-D warnings` | PASS | 1,90 s | 261 732 KiB |
| CBOR Criterion smoke | 6 encode/decode cella PASS | 4,57 s | 370 916 KiB |

Forrás: [végső futási napló](semantic-digest-correction/semantic-digest-final.log),
[konténer inspect](semantic-digest-correction/semantic-digest-final-container.json).
SDK: `hymeko-core:1886f17a42d7a3430e4044129f81955615efd583`,
image ID `sha256:c825ade5a9b80d45648ef5175398707cffe5b028b251015d69c39b9abc8a1308`;
Rust/Cargo 1.97.1, Linux x86_64, release/locked/offline, hat CPU, 16 GiB limit,
swap nélkül. A változott forrás a konténer tiszta `/tmp/core` könyvtárába került;
az SDK image címkéje a buildkörnyezetet azonosítja, nem a javított forrást.
Végső cgroup-csúcs: **3 300 356 096 byte (3,07 GiB)**; exit 0, OOM nélkül.

Ezek diagnosztikai ellenőrzési idők, nem digest-latencia- vagy gyorsulásmérések.
A korábbi 84 s-os Core-kapu eltérő cache-állapotú volt; a falidőket nem
kezeljük teljesítmény-összehasonlításként. Nincs új benchmark- vagy coverage-% állítás.
A memóriaigény `O(R + P + depth)`: P a tárolt névútvonal-szegmensek összmérete,
ami mély névláncon négyzetes is lehet. A rekordaggregáció rendezése `O(R log R)`.

## Regressziók, konszolidáció és indítási hibák

A 18 incoming teszt és a négy korábbi RED-kontroll elvárása megmaradt; csak az
explicit algoritmusverzió 1→2. Hét további kontroll teszteli a helykitöltőt,
arc-megosztást, árva rekordokat, fallbacket és a 0/1/63/64/65/128/256 mélységeket.
Mind a 29 tesztnév megmaradt a fixture-konszolidáció során. A közös node/edge
fejléc a meglévő kódolókból épül, a signed atom elérése a meglévő `atom()` API-t
használja. A tesztfájl duplikált sorai 186→0-ra csökkentek.

`contract-symbols`: **619 szimbólum, PASS**, a jegyzék generált.
`contract-duplication`: **403 clone / 7770 sor**, baseline **403 / 7800**,
**PASS, változatlan baseline**. `git diff --check`: PASS.
Új error-handling waiver nincs: az egyetlen `expect` előtt dokumentált
konstruktor-invariáns garantálja az owner-slot létezését. A tesztassertionök
nem helyettesítik a produkciós validátort.

Két környezeti indítási hiba megőrizve: a `bash -lc` PATH-ja nem találta a
`rustc`-t; a javított launcher `bash -c`. Utána a forrásból indított `core.sh`
saját, nem executable példányát próbálta meghívni. A végső futás az SDK
byte-azonos, executable `hymeko-core-check` belépőjét használta.
A forrás scriptjét nem módosítottuk, kaput nem kerültünk meg. A kezdeti
konténerek és logok a bizonyítéktárban maradtak; az első valódi futás már
29/29-et, a következő teljes futás 395/7-et adott. A konszolidáció után a fenti
végleges forrásra újra lefutott a teljes tervezett ellenőrzés.

## Érintett fájlok és rollback

| Alkalmazási fájl | Hozzáadott / törölt sor |
|---|---:|
| `CORE.YAML` | 55 / 0 |
| `SYMBOLS.md` | 6 / 1 |
| `hymeko_core/src/ir/hash_pass.rs` | 20 / 0 |
| `hymeko_core/src/ir/mod.rs` | 2 / 0 |
| `hymeko_core/src/ir/semantic_digest.rs` | 425 / 0 |
| `hymeko_core/src/module_store/module_store.rs` | 9 / 0 |
| `hymeko_core/tests/unified_ir/verification/mod.rs` | 1 / 0 |
| `hymeko_core/tests/unified_ir/verification/semantic_digest.rs` | 678 / 0 |

További változások: a meglévő terv `.tex` és `.pdf` állománya, ez a jelentés,
valamint a `reports/semantic-digest-correction/` végrehajtási bizonyítékai.
A korábbi review-patch és `.rs.txt` mellékletek történeti javaslatok;
a `proposal.json` az **f35c34f javaslatcommit** állapotát és bájtjait írja le.
A mostani végrehajtást az `execution.json` írja le.

Rollback: a javítócommit célzott visszavonása helyreállítja az auditág korábbi
Core API-ját; későbbi consumer-pinfrissítéseket előbb fordított függőségi
sorrendben kell visszavonni. A választott algoritmushoz a V2 cache újraszámítandó.
A korábbi audit-tagek nem mozdulnak; main merge vagy release nem történt.

## Nagare fogyasztói határ

A frissen ellenőrzött célref `f3660cc8f738cccc69c2a011a943ce811347815e`.
A `merkle_digest_soundness` család 14 tesztje közül négy V2-t használ; a fixture
a produkciós Parser/ModuleStore útján töltött `tests/hnn_data/mlp/mlp2.hymeko`.
A célref eredeti Core/WGPU lockja `112a254`, Graph `9655f077`, Parser `284c490d`.
A meglévő tesztcsaládot egy külön Amaterasu Git-snapshotban lefuttattuk;
a helyi Nagare `ad500ab` checkout és annak publikálatlan commitja változatlan.

A célág `AGENTS.md` §2 előírja a `../ROADMAPS.md` registry és mindkét canonical
roadmap elolvasását; hiányukra kifejezetten stopot ír elő.

**Helyesbítés, 2026-09-06 02:10 +02:00:** a korábbi munkafakeresés eredményét
tévesen kiterjesztettük a Git-tartalomra. Mindkét roadmap már a változatlan
`f3660cc` commit `docs/roadmaps/` könyvtárában is szerepel. A most feltöltött
`hymeko/hymeko` main `25d2d2ad8ed0a6a2508545b91f3c0d5c026669a4` verziója
`roadmaps/ROADMAPS.md` alatt ad hozzájuk verziózott registryt és pontos
snapshot-hivatkozásokat. A `git show` mindhárom dokumentumra sikeres; mindet
teljes egészében elolvastuk. A korábbi repószintű hiánykövetkeztetés megcáfolva.

A helyi `D:/hnnwork/distributed/hymeko` tisztán fast-forwardolt erre a mainre.
Saját provider-forrásmódosítás vagy provider-tesztújrafuttatás nem történt;
a bejövő registry jelentése tartalmazza az upstream ellenőrzést. A Nagare
fogyasztói környezet külön megtartja a szerződés által előírt exact `9d148a4`
providerét, és az új registryt a saját `25d2d2a` provenienciájával tölti be.
Ez helyreállítja a dokumentumok elérését, nem másolja vagy írja át a roadmapet.
A Core-terv kiegészült a konkrét consumer-revíziókkal és futási korlátokkal,
PDF-fordítása sikeres.

| Fogyasztói komponens | Ellenőrzött Git-revízió |
|---|---|
| Nagare teszt és fixture | `f3660cc8f738cccc69c2a011a943ce811347815e` |
| Core és WGPU | `125472ebfd16c9beaa96ffe2c1b00e145e2c15dd` |
| Graph | `9655f07755e271c0f2ee4361a526b14e0205af8c` |
| Parser | `284c490d68232973a006bcba974279fed6f89ce8` |
| Tensor | `b1d869d24fb563ed7042f38a1272c1736fdfbea4` |

Parancs a Nagare gyökerében:

```sh
cargo test --release --locked --offline -p hymeko_nagare --test merkle_digest_soundness -- --nocapture
```

**14 PASS, 0 FAIL, 0 ignored**. A négy V2-követelmény: első hívástól idempotencia,
stale `decl_hash` figyelmen kívül hagyása, incidencia-cél és visszafelé mutató
`ValueR::Ref` célváltozásának megkülönböztetése. A további tíz teszt a megőrzött
legacy viselkedést jellemzi vagy ellenőrzi. A teljes család a változatlan
produkciós Parser/ModuleStore-fixture-t használja, új teszt vagy forrásmódosítás nélkül.
Ez lezárja a javítás tervezett downstream digest-kapuját; a Nagare teljes
`scripts/check.sh` ellenőrzését és egy általános Nagare-release megfelelőségét
nem állítja.

A teszt fordítással együtt **49,83 s**, legnagyobb process RSS **1 041 416 KiB**,
exit **0**. A meglévő SDK/cache, a Nagare SSE4.1 flagje, `debug=true` és
`lto="thin"` release-profilja megmaradt; limit 16 GiB, hat CPU, hozzáadott swap nélkül.
A cgroup-csúcs **2 998 603 776 byte (2,79 GiB)**; a konténer exit 0-val,
OOM nélkül, összesen **55,49 s** után leállt. Ezek diagnosztikai adatok,
nem teljesítmény-összehasonlítások.

A scratch lockban a Core/WGPU együtt frissült `112a254`→`125472e` revízióra;
az előtte/utána Cargo metadata és tree egy-egy Core/WGPU/Graph identitást mutat,
Core/Graph path override nélkül. A Git-letöltés a pontos commitokat tartalmazó
helyi mirrorokon ment át, az eredeti canonical Cargo source-ID megőrzésével.
A Cargo emellett hét meglévő Windows-függőségi él verzióhivatkozását módosította;
csomagverzió hozzáadás vagy eltávolítás nincs. A teljes eltérés archivált, és a
lock hash-e a teszt előtt/után azonos. Tartós Nagare-lockfrissítés nem történt.
Egyik Linuxra szűrt resolve gráfban sincs `windows-sys` csomópont;
a hét Windows-él végrehajtását ez a Linux-futás nem ellenőrzi.

Bizonyíték: [tesztnapló](semantic-digest-correction/nagare-test.log),
[lock-diff](semantic-digest-correction/nagare-lock.diff),
[végső metadata](semantic-digest-correction/nagare-metadata-after.json),
[konténerállapot](semantic-digest-correction/nagare-container-final.json),
[fogyasztói manifest](semantic-digest-correction/nagare-execution.json).
A manifest mind a 33 hivatkozott artifactjának SHA256-ellenőrzése sikeres.
Az első launcher argumentumhibája nem indított Cargo-t; két további próbában
a Git upload-pack tulajdonosellenőrzése állította meg a metadata-lekérést,
még teszt előtt. Ezek külön naplózva maradtak. A három pontos mirror `.git`
útját a disposable konténer saját Git-configjában engedélyezve a kapu lefutott;
közös Git-config vagy SSH-trust változtatására nem volt szükség.

A Core produkciós forrása továbbra is byte-azonos a Gitben tesztelt `125472e`
változattal; a korábbi 17 Core-artifact és nyolc forrás hash-ellenőrzése sikeres.
A mostani folytatás a tervet, jelentést és consumer-bizonyítékokat bővíti.
A generált consumer-manifest LF-sortörést használ. Az `.gitattributes` meglévő
patch-szabályának `.diff` megfelelője megőrzi a nyers lock-diff szándékos
kontextusszóközeit; az alkalmazási források whitespace-kapuja változatlan.

## Discovery

```text
DISCOVERY LEDGER — SemanticModelDigestV2
  find:    rg --files -g '*digest*' -g '*hash*' tools reports/semantic-digest-correction hymeko_core/src hymeko_core/tests
           -> 8 találat: a meglévő hash-modulok, tesztek és a jóváhagyott tervezet
  grep:    rg 'semantic_model_digest_v2|semantic_declaration_hashes|build_path_index' hymeko_core/src/ir
           -> 1 találat: canonical_hash.rs::build_path_index
  symbols: SYMBOLS.md azonos keresés -> 1 találat: build_path_index
  read:    hash_pass.rs, canonical_hash.rs, a jóváhagyott patch és terv
  verdict: EXTEND a bejövő ir::semantic_digest modult a jóváhagyott javítással;
           a V1 hash eltérő szerződését és implementációját megőrizzük.

DISCOVERY LEDGER — közös rekordfejléc
  find:    meglévő semantic_digest.rs
  grep:    fn .*header|fn .*record.*hash|fn write_signed_refs
           -> 3 találat: header, record_hashes, write_signed_refs
  symbols: nincs új publikus szimbólum
  read:    SemanticContext::record_hashes
  verdict: EXTEND SemanticContext privát rekordfejléc-segéddel;
           a meglévő header és write_signed_refs komponálásával.

DISCOVERY LEDGER — semantic digest teszt-fixture konszolidáció
  find:    rg --files hymeko_core/tests/unified_ir | rg 'semantic|builder|hash_coverage' -> 3 találat: semantic_digest.rs, support/builder.rs, hash_coverage.rs
  grep:    rg -n '^fn |^struct ' hymeko_core/tests/unified_ir/verification/semantic_digest.rs -> meglévő program, link, plain, assert_changed és CBOR-fixture helperek
  symbols: rg -n 'TestIr|semantic_digest|assert_changed' SYMBOLS.md -> 5 publikus digest-szimbólum; a privát teszthelper nincs exportálva
  read:    semantic_digest.rs, support/builder.rs, hash_coverage.rs, modules/10-discovery.md, jóváhagyott plan.tex
  verdict: EXTEND hymeko_core/tests/unified_ir/verification/semantic_digest.rs; közös privát fixture- és assertion-helperek a meglévő TestIr fölött, új fájl vagy publikus API nélkül.

DISCOVERY LEDGER — Nagare fogyasztói ellenőrzés folytatása
  find:    git ls-tree / git show: provider 25d2d2a registry, Nagare f3660cc docs/roadmaps és tests/merkle_digest_soundness.rs
  grep:    meglévő consumer-előkészítés és végrehajtási bizonyítékok; a 14 teszt közül 4 hívja a V2 API-t
  symbols: nincs új produkciós vagy publikus szimbólum
  read:    registry, mindkét roadmap, Nagare szerződés, Cargo.toml, Cargo.lock, .cargo/config.toml, meglévő tesztcsalád
  verdict: REUSE a meglévő tesztcsaládot, SDK-t és Core-terv/jelentés tulajdonost;
           a nagare-* mellékletek e konkrét futás pontos parancsai és bizonyítékai,
           nem új termék- vagy CI-belépők. A korábbi Core-log eltérő fogyasztót tesztel,
           ezért nem helyettesíti ezt a downstream eredményt.
```

## Korábbi előkészítési feljegyzés

<details>
<summary>A jóváhagyás előtti f35c34f állapot — történeti feljegyzés</summary>

# Core SemanticModelDigestV2 — javítási csomag

Created-at: 2026-09-06 00:55 +02:00. Updated-at: 2026-09-06 01:09 +02:00.

Állapot: **előkészítés; védett forrásra még nem alkalmazott javaslat**.
A felhasználó a négy reprodukált digest-hiba javítását kérte. Az alap az auditág
`ae068b98af202e6a4fda43e656b02fedce3456c8` revíziója; a beemelendő változás kizárólag
a `112a2543264e302839fd7f2cf04858fd098c8f5c` hétfájlos digest-commitja.
A portable-ID és robotfixture-őscommitok külön scope-ban maradnak.

Az [előző audit](2026-09-05-dependency-refresh.md#hátramaradt-ágak-auditja--2026-09-06)
18 meglévő PASS és négy új FAIL eredményt rögzít. A negatív kontrollok és az
eltérő corpus-fixture kontroll változatlan történeti bizonyítékok.

## Konkrét változtatás

Egy privát kódolási kontextus használja a Core meglévő `build_path_index`
eredményét. Az anonim arc azonosítója az owner névútvonala és az owner rendezett
arc-listájának első megfelelő pozíciója. Minden arc teljes tartalma bekerül akkor
is, ha nincs külön deklarációja. Az edge slotok megőrzik az arcok azonosságát,
sorrendjét, multiplicitását és megosztását; azonos payload nem jelent azonos arcot.

A modell-aggregáció rekordtípusonként rendezett hasheket használ; a diagnosztikai
deklarációtábla DeclId-sorrendű marad. A konkrét rekordtáblák teljes lefedése
megelőzi az arc-hiba megfelelőjét a dispatchből el nem érhető node/edge rekordoknál.
Az értékbejárás iteratív és teljes: nincs `TRUNC`, megmarad a listák sorrendje,
az option/length/sign kódolás és a lebegőpontos bitminta. Az anonim névhelykitöltő
kimarad, az annotáció és parent-identitás megmarad.

Ha nincs egyértelmű kanonikus owner-slot azonosság, az egész modell explicit
raw-ID fallback domaint kap. Ott is minden rekordtartalom és slot szerepel,
de átindexelési invarianciát nem állítunk. A hibás szimbólum-/rekordindexekre
vonatkozó korábbi „total malformed input” ígéret helyett pontos előfeltételek és
`Panics` dokumentáció szükséges. Ez nem új CBOR-validátor.

Publikus függvényszignatúra és függőség nem változik. Schema 2 marad,
`SEMANTIC_DIGEST_ALGO_VERSION` 1→2. A V1 hash, a deprecated Merkle-függvény törzse,
az `Ir::decl_hash`, a CBOR wire, a Parser-lock és a distributed admission változatlan.
A régi V2 digestből képzett cache/plan azonosság újraszámítandó.

## Elkészült review-patch és ellenőrzések

[Alkalmazható patch](semantic-digest-correction/proposed-core-edit.patch),
[javított modul tervezete](semantic-digest-correction/semantic_digest.rs.txt),
[további tesztek](semantic-digest-correction/additional_tests.rs.txt),
[pontos forrás- és célhash-ek](semantic-digest-correction/proposal.json).
A `.rs.txt` fájlok kizárólag review-mellékletek; nem szerepelnek produkciós vagy
tesztmodul-regisztrációban. A patch a meglévő Core-tulajdonosban hozza létre a
bejövő modult, nincs második implementációs út.

`git apply --check`: **PASS**, alkalmazás nélkül. A jelölt modul és az összeállított
tesztmodul `rustfmt --edition 2024 --emit stdout` parser-/formázási ellenőrzése:
**PASS**. Ez nem Rust típusellenőrzés vagy build. A független statikus review egy
érték szerinti `PathKey::iter()` elem téves dereferálását és egy előfeltétel
pontatlan leírását találta; mindkettő javítva a javaslatban.

Az összeállított tesztmodul 18 eredeti tesztet, a négy változatlan követelményű
auditkontrollt és hét további tesztet tartalmaz (összesen 29). A korábbi négy FAIL
már futtatott negatív bizonyíték; a javított kód és a hét további teszt még **nem
futott**. Teljes Core-, Nagare-, Clippy- vagy benchmarkeredményt nem állítunk.
A terv PDF-je sikeresen lefordult. A védett források, dependency-lockok és
`SYMBOLS.md` munkafadiffje üres maradt.

Az importtal együtt a review-patch fájlonkénti sorváltozásai:

| Fájl | Hozzáadott / törölt sor |
|---|---:|
| `CORE.YAML` | 55 / 0 |
| `hymeko_core/src/ir/hash_pass.rs` | 20 / 0 |
| `hymeko_core/src/ir/mod.rs` | 2 / 0 |
| `hymeko_core/src/ir/semantic_digest.rs` | 420 / 0 |
| `hymeko_core/src/module_store/module_store.rs` | 9 / 0 |
| `hymeko_core/tests/unified_ir/verification/mod.rs` | 1 / 0 |
| `hymeko_core/tests/unified_ir/verification/semantic_digest.rs` | 705 / 0 |

A `SYMBOLS.md` a jóváhagyott alkalmazás után, a provider eszközével generálandó;
kézzel írt vagy kitalált szimbólumjegyzék nincs a patchben. Az előkészítés
alkalmazhatósági/szintaxis-ellenőrzés, új futásidő- vagy memóriaeredmény nélkül.
A [validációs napló](semantic-digest-correction/patch-validation.log) rögzíti a
parancsok terjedelmét. A forrásváltozások tesztelése a terv szerinti következő lépés.

## Fájlok, tesztterv és rollback

Védett alkalmazási scope:

- `CORE.YAML`: a szűk jóváhagyás és invariánsok rögzítése;
- `hymeko_core/src/ir/semantic_digest.rs`: a bejövő modul és javított kódolás;
- `hymeko_core/src/ir/mod.rs`: a bejövő modul exportja;
- `hymeko_core/src/ir/hash_pass.rs`: a bejövő deprecation/doc változás, változatlan törzs;
- `hymeko_core/src/module_store/module_store.rs`: a bejövő lokális deprecation allow/doc változás.

Teszt- és dokumentációs scope: a meglévő
`hymeko_core/tests/unified_ir/verification/{mod.rs,semantic_digest.rs}`,
generált `SYMBOLS.md`, e jelentés, a
[tervnégyes](../docs/plans/2026-09-06-semantic-digest-correction/plan.pdf) és a
`reports/semantic-digest-correction` bizonyítéktár.
Az `.gitattributes` három új útvonalszabálya a csomag nyers bájtjait őrzi;
ez dokumentációs/bizonyítékkezelési változás, nem védett Core-implementáció.

Jóváhagyás után a négy eredeti ellenpélda változatlan követelménnyel fusson.
További kontrollok: anonim helykitöltő/interner-sorrend, azonos tartalmú arcok
`[A,A,B]`/`[A,B,B]` megosztása, árva rekordok payload-módosítása és fallback
idempotencia, mélységhatárok. A bejövő 18 tesztből csak az explicit algoritmus-
verzióassert frissülhet. Utána teljes Core-kapu, CBOR corpus, kilenc admission-
regresszió, fmt és Clippy. A Nagare `f3660cc8f738cccc69c2a011a943ce811347815e`
revíziójának `tests/merkle_digest_soundness.rs` fogyasztója a szignatúrákat már
használja; e meglévő család külön consumer-kapu. A friss fetch ezt igazolta.

Amaterasu: meglévő SDK/cache, 16 GiB, hozzáadott swap nélkül, 6 CPU,
release/locked/offline. Új teljesítményállítás nem készül. A pontos terv a
szükséges legrosszabb memória- és futásidőhatárt, valamint az ETA-t is rögzíti.

Rollback most a javaslat dokumentációs commitjának visszavonása. Alkalmazás után
az import és a javítás külön tematikus commit legyen; fogyasztói pinfrissítés
fordított függőségi sorrendben visszavonható. A korábbi audit-tagek nem mozdulnak.

## Jóváhagyási határ

A [CORE.YAML:224](../CORE.YAML:224) előírja:

> A change may proceed only after the user replies in the chat with the literal token

és: „A token applies only to the slug it names; it does not generalise.”
A korábbi Parser- és admission-tokenek nem erre a változtatásra vonatkoznak.
A szükséges új token: `APPROVED-CORE-EDIT: semantic-digest-canonical-coverage`.
Ez még **nem érkezett meg**, ezért védett forrás nem módosult; a patch ellenőrzése
jóváhagyás előtt csak alkalmazhatóságot és szintaxist jelenthet, futtatott javítást nem.

</details>
