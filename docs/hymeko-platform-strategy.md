# HyMeKo platformstratégia

**Stratégiaazonosító:** `HYMEKO-PLATFORM-2026-09-01-v1.1`
**Állapot:** irányadó, verziózott architektúra- és végrehajtási keret  
**Bizonyítékhatár:** 2026-09-01 18:11 CEDT  
**Részletes terv:** `docs/plans/2026-09-01-hymeko-platform-strategy/`

Ez a dokumentum a HyMeKo platform közös iránytűje. A későbbi feladatpromptok
erre a fájlra és a fenti stratégiaazonosítóra hivatkozzanak. Nem helyettesíti a
`CLAUDE.md`, `CORE.YAML` és `tools.yaml` szabályait, és nem tesz egy történeti
riportot aktuális forrásigazsággá. Konfliktus esetén a jelenlegi kód, a tesztek
és a lockdown-szabályok döntik el, mi van ténylegesen implementálva.

## 1. Célállapot

A HyMeKo egy nagy teljesítményű, kis memóriaigényű, párhuzamosítható,
hipergráf-natív platform legyen, amelyben:

1. egyetlen kanonikus szemantikai modell van: `hymeko_core::ir::Ir`;
2. a `.hymeko` forrás a parseren és loweringon át ebbe az IR-be kerül;
3. a gráfkeresés, algebra, tensor/HNN, emitter, Python/WebXR és MCU-végpont
   ugyanennek az IR-nek a szemantikáját használja;
4. az incidencia előjele, súlya, slot-sorrendje és multiplicitása veszteség nélkül
   megmarad;
5. a gyorsító struktúrák ritka, visszaállítható, verziózott indexek vagy numerikus
   projekciók, nem második gráfmodellek;
6. a dinamikus változtatás tranzakciós, atomi és elosztottan visszajátszható;
7. CPU, Rayon, WGPU, később CUDA/FPGA és MCU ugyanazon szemantikai szerződés
   különböző végrehajtási profiljai;
8. a terméknek végül egy felhasználói CLI-je legyen, ne könyvtáranként külön
   belépési pontja.

Röviden: **egy kanonikus hipergráf, több ellenőrzött végrehajtási és
megjelenítési felület, nulla párhuzamos igazságforrás.**

## 2. Nem alkuképes architekturális szabályok

### S1 — Egyetlen kanonikus IR

Az `Ir` az egyetlen módosítható szemantikai állapot. Editor-IR, scene DTO,
`SignedGraph`, külön HNN-gráf vagy MCU-specifikus modell nem válhat második
igazságforrássá. Egy végpont készíthet borrowed lekérdezési felületet,
verziózott indexet vagy packed numerikus projekciót, de az eldobható és az
`Ir`-ből determinisztikusan újraépíthető.

### S2 — Hipergráf-natív működés

A standard gráfalgoritmusok a kanonikus IR-en és annak pontos
`IncidenceIndex<'ir>` indexén dolgoznak. A node → `(HyperArcId, slot)` →
incidencia útvonal megőrzi a jelet, súlyt, annotációt, sorrendet és duplikációt.

Tilos standard végrehajtási útként:

- pairwise gráffá lapítani a hipergráfot;
- clique/2-section expanziót materializálni;
- egy második, önállóan módosítható star-gráfot tárolni;
- az indexet lekérdezésenként újraépíteni.

### S3 — A star ritka incidenciaoperátor

A star szerkezet elsődleges matematikai alakja az incidenciaoperátor `B`, nem
egy sűrű gráf:

- `Bx`: node → hyperarc;
- `B^T y`: hyperarc → node;
- `B^T Bx`: star-operátor.

COO/CSR megengedett mint ritka, célhoz kötött numerikus projekció — például GPU
uploadhoz vagy SpMM-hez —, de nem szemantikai modell és nem állapotgazda. A
clique csak explicit összehasonlító kontroll vagy olyan algoritmus bemenete
lehet, amely matematikailag ténylegesen azt kéri.

### S4 — Gyorsító index csak bizonyítható provenienciával

Egy derived indexnek:

- kölcsönöznie vagy verzióval azonosítania kell a forrás-IR-t;
- pontos bijekciót kell igazolnia az incidencia-koordinátákkal;
- nem deduplikálhat és nem rendezhet át szemantikát;
- invalidálhatónak vagy inkrementálisan frissíthetőnek kell lennie;
- nem kerülhet be önálló kanonikus wire állapotként.

A lapos, cache-barát target/sign/weight tömb csak ilyen index részeként
engedélyezett. Ez teljesítmény-cache, nem „sűrű gráfnézet”.

### S5 — Egy wire-formátum

Az MCU és a disztribúciós út alapja a meglévő `ir_stream` envelope + section
formátum:

- little-endian, fix szélességű mezők;
- nincs `usize`, pointer, Rust enum vagy natív alignment a wire-on;
- ellenőrzött offset/length aritmetika;
- BLAKE3 integritás;
- skippelhető szekciók és tranzakciós framing.

A jelenlegi CBOR-artefaktum nem működő codec, hanem nem bekötött teljes másolat.
CBOR nem főirány és nem második séma. Csak konkrét, külső RFC 8949
interoperabilitási igény igazolhatja később, packed sectionöket hordozó
borítékként.

### S6 — Egy végrehajtási szerződés, több backend

A CPU serial, Rayon, WGPU, CUDA-feasibility és későbbi FPGA/MCU implementációk
ugyanazt a műveleti és numerikus szerződést teljesítik. A backend nem birtokol
új szemantikát. A resident tensor, pipeline cache és arena végrehajtási állapot,
nem IR-állapot.

### S7 — Egy CLI, könyvtári tesztek

A könyvtáraknak legyenek teljes API-, integrációs, property/metamorf és
benchmark tesztjeik. Ne hozzunk létre minden PoW-hoz új termék-CLI-t vagy
tesztkeretrendszert. A végső CLI egy downstream façade legyen, hogy ne hozzon
létre `core → consumer → core` ciklust.

## 3. Tulajdonosi térkép

| Terület | Tulajdonos | Mit nem birtokolhat |
|---|---|---|
| kanonikus IR, ID-k, tranzakció, snapshot, `ir_stream` | `hymeko_core` | renderer- vagy backend-handle |
| szintaxis és AST | `parser` | rezidens szemantikai modell |
| hipergráf-algoritmusok | `hymeko_graph`, `&Ir` + pontos ritka index felett | saját kanonikus Graph/HyperGraph állapot |
| HNN és tanulási réteg | Nagare | core-ba visszamásolt HNN, saját szemantikai gráf |
| sparse numerikus kernel | core tensor API + backendek | szemantikai állapot |
| WGPU | `hymeko_wgpu` | IR-mutáció és gráfszemantika |
| emitter/transformation | fogyasztó az `Ir` felett | editor-IR bridge mint állandó modell |
| WebXR szemantika | `transformation/hymeko_webxr`, az `&Ir` fogyasztója; browser binding downstream | WebXR vocabulary/validator/query a core-ban; `XRSession`/`XRFrame`/GPU/DOM handle az IR-ben |
| MCU C/C++ | ugyanazon `ir_stream` wire read-only/dynamic view-je | külön C vagy C++ gráfmodell |
| végső CLI | downstream façade | új szemantikai réteg |

Mielőtt egy feladat új típust, crate-et, generátort, DTO-t, scratch-workspace-t,
codec-et vagy CLI-t ad, kötelező a meglévő tulajdonos és az összes ekvivalens
implementáció keresése. Az új absztrakció csak akkor jogos, ha legalább két
valós fogyasztót egyszerűsít vagy egy bizonyított invariánst tesz szerkezetivé.

## 4. Dinamikus IR és elosztott frissítés

### 4.1 Célfolyam

```text
.hymeko / validált snapshot
  -> kanonikus Ir snapshot vN
  -> Transaction(base=vN, operations...)
  -> teljes validáció
  -> atomi commit vN+1 vagy változatlan vN + typed error
  -> immutable snapshot publikáció
  -> verziózott sparse index/tensor frissítés
  -> ir_stream delta vagy snapshot
  -> replika idempotens apply
  -> azonos konvergencia-hash
```

Az elvárt commit-osztályok:

- érték/súly/feature/annotáció: 100 Hz tartós, 250 Hz burst;
- topológia: 10 Hz tartós, 30 Hz burst;
- inference reader nem blokkolhat mutációra;
- teljes derived rebuild csak topológiaváltozásnál indokolt.

### 4.2 ID-stratégia: nyitott döntés, mért korláttal

A generációs aréna jelenleg **nem elfogadott produkciós megoldás**. Az
append-only `Vec`-hez képest a legjobb prototípus +56% valid lookup regressziót
mutatott, miközben törlés még nem létezik. A wire-forma és stale-ID invariánsok
értékesek, de a hot-path adó most indokolatlan.

A következő döntés nem „generáció igen/nem”, hanem egy deletion-capable storage
tanulmány. Legalább ezeket kell ugyanazon churn workloadon összevetni:

1. generációs slotok;
2. monoton stabil semantic ID + pozíciótábla;
3. immutable snapshot + tombstone/kompakció;
4. szegmentált, nem újrahasznált ID-k és epoch-validáció;
5. boundary-validált handle + olcsó snapshot-belső hot lookup.

A baseline egy törölni képes rendszer legyen, ne a mai append-only `Vec`. A
választás feltétele: stale/ABA hiba lehetetlen vagy typed, commit-budget teljesül,
lookup/memória/churn mérve, és a kanonikus hash független a fizikai slottól.

### 4.3 Tranzakció és publikáció

Az `IRDelta` szemantika egyetlen core API-ba kerül. A `Batch` valódi tranzakció:
se részleges alkalmazás, se kívülről megfigyelhető félállapot. A commitnak két
identitása van:

- O(delta), history-sensitive commit-chain digest minden commitnál;
- history-independent canonical semantic hash checkpoint/audit/convergence
  célra, nem a 100 Hz hot pathon.

Value-only commit a topológiai indexet nem építheti újra. Topológiai commit az
incidenciaindexet atomi módon frissíti vagy új verziót épít. A régi snapshot és
GPU buffer csak a hozzá tartozó submission/version lezárulta után szabadítható.

## 5. Teljesítménystratégia

### 5.1 Mért kiindulópontok

Az alábbiak 2026-09-01-i bizonyítékok, nem örök konstansok:

| Terület | Kiindulópont | Minősítés |
|---|---:|---|
| `Bx`, 1M incidencia | 5.2–5.9 ms | quiet-host mérés |
| `B^T y`, 1M incidencia | 17.6–28.9 ms; 3.4–4.9× `Bx` | quiet-host mérés |
| `B^T Bx`, 1M incidencia | 22.8–35.3 ms | quiet-host mérés |
| direct-IR walk műveletek | 13–17 ns/incidencia | provisional, egy host |
| `s_connected_components(s=1)` | 2.7 ns/incidencia | provisional |
| Rayon batch ×16 | 2.8×–10.8× | méretfüggő |
| WebXR validáció | 0.31 µs/entity; 31 ms/100k | lineáris |
| reverse-incidence index | 12.4–14.8 B/incidencia | valós struct |
| CSR SpMV Rayon crossover | 81 920 nnz | publikált baseline |

### 5.2 Optimalizálási sorrend

1. **`B^T y` sign/weight locality.** Az exact `(arc,slot)` koordináta mellé
   veszteségmentes, derived signed-scalar cache; szemantika és FP-sorrend marad.
2. **Direct-IR traversal locality.** Az `Ir`/`IncidenceIndex` sparse, lapos
   hozzáférési útjának javítása. Nem külön Graph modell; a kontrollként mért flat
   sparse-star előnye mutatja a layout-célt.
3. **Batch párhuzamosítás.** Mért küszöb alapján Rayon csak ott alapértelmezett,
   ahol a fixed cost amortizálódik.
4. **Dinamikus commit és incremental refresh.** Külön benchmark value-only és
   topology utakra.
5. **Resident accelerator chains.** Upload/readback ne keveredjen kernelidővel;
   a köztes eredmény maradjon eszközön.

Minden optimalizáció előtt profil vagy diszkrimináló benchmark kell. Egy hosszú
függvény rövidítése önmagában nem cél; a hot numerikus magok FP-sorrendje és a
wire validator első-hiba sorrendje szerződés lehet.

### 5.3 Benchmark-szerződés

- egyetlen közös workload-definíció tulajdonosonként;
- kicsi/közepes példák valódi `.hymeko` forrásként, parseren át;
- nagy példák determinisztikus generátorból, azonos formulás oracle-lal;
- Steiner, sunflower, Mycielski, random uniform/degree/hub, számítógépes hálózat,
  humanoid, hexapod és biomedikus/chicken családok;
- cold, prepared, resident és end-to-end fázis nem keverhető;
- medián, IQR/CI, worst cell, A/A null és ambient load kötelező;
- peak RSS, retained bytes, allocation count és bytes/decl/incidence külön mérve;
- két host, ha a toolchain rendelkezésre áll; külső hostra telepítés csak külön
  engedéllyel;
- egyetlen 3%-os regressziós állítás sem alapulhat a host saját null-zajánál
  kisebb eltérésen.

## 6. MCU, C és C++ irány

Három profil marad:

- **MCU-RO:** flashből nyitott, nulla-heap read-only sparse view;
- **MCU-DYNAMIC:** caller-provided fix arena, staging, atomi A/B snapshot vagy
  journal;
- **MCU-HOSTED:** allocator és több workspace megengedett.

Az első implementációs PoW hoston fordított C11 view legyen:

1. ugyanazt a frozen `ir_stream` KAT byte-sort olvassa Rust és C;
2. nincs dinamikus allokáció;
3. minden offset/add/multiply checked;
4. unaligned adatot byte assembly vagy `memcpy` olvas, soha nem pointer-cast;
5. malformed corpus, ASan és UBSan zöld;
6. a C++ réteg csak vékony RAII/span wrapper ugyanarra a C view-ra;
7. csak ezután következik Cortex-M/ESP32 cross-compile.

Nem kerül parser az első MCU PoW-ba, és nem készül C-ben második IR.

## 7. WebXR és más végpontok

A WebXR szemantikai PoW viselkedése helyes, de az eredeti core-ownership hibás volt:
`meta_spatial` + `meta_webxr`, a production validator és a borrowed query a
`transformation/hymeko_webxr` domainprojekció része. A core csak a kanonikus `Ir`-t
és annak általános accessorait szolgáltatja. A browser binding
runtime handle-táblája endpoint állapot, és csak tartós felhasználói döntést küld
vissza jövőbeli IR-tranzakcióként; per-frame pose nem commitolódik az IR-be.

Ugyanez a minta érvényes Pythonra, Hotarura, emittersre és hálózati végpontokra:
borrowed lekérdezés vagy explicit derived sparse projection, majd typed
tranzakciós visszaút.

## 8. Verziózott végrehajtási terv

Egy prompt egyszerre egy fázist kapjon, és a fázis végén HARD STOP következzen.
Az „állapot” oszlop a célállapotot jelöli, nem automatikus engedélyt.

| ID | Fázis | Kimenet és elfogadási kapu | Függőség |
|---|---|---|---|
| **P0** | bizonyíték- és branch-konszolidáció | SHÁ-k, ancestry, teljes gate; semmi silent merge; történeti ellentmondások jelölve | most |
| **P1** | benchmark-harness konszolidáció | SparseSpec/LCG/topológia-generátor/CPU oracle duplikációk felszámolva vagy egyértelműen tulajdonoshoz rendelve; minden régi KAT és eredmény azonos | P0 |
| **P2** | incidenciaoperátor precompute | `B^T y` gyorsulás diszkrimináló A/B-ben; exact coordinate, sign/weight és FP-oracle; memóriahatás kimérve | P1 |
| **P3** | direct-IR sparse locality + batch | graph ops továbbra is `&Ir` + exact sparse index; nincs új gráfmodell; warm/cold/build/memory külön; serial=Rayon | P1, lehet P2 után |
| **P4** | deletion-capable storage study | öt jelölt azonos churn modellen; stale/ABA, lookup, iteration, commit, clone/snapshot, memória; döntés vagy őszinte deferral | P1 |
| **P5** | canonical transactions | natív delta API, atomi Batch, typed reject, no partial state; value/topology út külön; sok dinamikus és fault-injection teszt | P4 + core token |
| **P6** | immutable snapshots + dynamic sparse refresh | lock-free reader publication; versionelt index; value patch ≡ rebuild, topology rebuild ≡ after-source oracle | P5 + core token |
| **P7** | `ir_stream` canonical apply | idempotens replay, conflict/gap/resync, második replika konvergencia; egy wire séma | P6 + core token |
| **P8** | MCU C wire-view PoW | C11/Rust frozen-byte parity, zero heap, malformed corpus, sanitizers; majd cross-compile | P0, P1-től függetlenül is mehet |
| **P9** | WebXR projekció + dinamikus resync | transformation-owned borrowed query API; browser binding külön; durable event → P5 transaction | read-only rész P0; write-back P5–P7 |
| **P10** | accelerator sparse operators | WGPU `B`/`B^T`, resident chain; CUDA kontroll; FPGA streaming interface; CPU oracle | P2–P3 |
| **P11** | egyetlen CLI façade | source/load/query/emit/benchmark/endpoint parancsok egy downstream CLI-ben; library API marad elsődleges | P7/P9/P10 stabil felületei |

### Ajánlott következő sorrend

1. P0: a már igazolt core/performance és graph ágak kontrollált konszolidációs
   térképe; a WebXR domainprojekció a transformation rétegbe kerül;
2. P1: benchmark-harness konszolidáció, mert minden következő perf-döntés erre
   épül;
3. P2: `B^T y` precompute, a legjobban izolált mért hot spot;
4. P4: mutable storage study a tranzakciós munka előtt;
5. P5 → P7: a dinamikus és elosztott kritikus út;
6. P8 és a P9 read-only része közben függetlenül haladhat;
7. P3/P10 csak pontos baseline és memory gate mellett.

## 9. V&V és Definition of Done

Minden produkciós fázisnak legalább ezt kell teljesítenie:

1. **Szemantika:** parser → lowering → Ir tovább működik; ID, hierarchy,
   incidence order/sign/weight/duplicate invariánsok tesztelve.
2. **Negatív tesztek:** malformed, stale, dangling, cycle, overflow, NaN/Inf és
   replay/conflict pontos typed errorral.
3. **Metamorf/oracle:** független oracle, before/after source pár vagy clean
   rebuild; ugyanazt a produkciós implementációt nem használhatja oracle-ként.
4. **Skálák:** 0/1/boundary, small/medium/large, legalább egy 10^5–10^6
   incidencia-cell, ha a művelet releváns.
5. **Coverage:** workspace rate azonos nevezővel; touched production line 100%
   vagy soronként indokolt fail-closed kivétel; compile-fail szerződésekhez
   compiling twin.
6. **Teljesítmény:** cold/prepared/resident/end-to-end elválasztva; memória,
   allokáció és throughput; A/A null; 3%-os gate csak feloldható zaj mellett.
7. **Párhuzamosság:** serial/Rayon bitazonos, vagy dokumentált tolerancia és
   változatlan akkumulációs sorrend; GPU/CPU abs+rel + NaN/Inf structural.
8. **Duplikáció:** repository-wide keresés; eltávolított és szándékosan megtartott
   ismétlések listája; új framework csak bizonyított többfogyasztós indokkal.
9. **Tulajdon:** dependency direction aciklikus; core nem függ consumerre.
10. **Handoff:** pontos fájllista, SHÁ-k, remote verify, saját hibák/retrakciók,
    limitationök, és a következő fázis belépési feltételei.

## 10. Egyszerűsítési backlog

Ezek külön, kis kockázatú fázisok; nem keverendők egy perf- vagy wire-változással:

- `CompiledProgram.idx` kompatibilitási mező nyugdíjazása;
- hierarchy/path coherence ellenőrzések konszolidálása az authority flip után;
- dead `tensor/decomposition.rs` és `module_store/module_view.rs` igazolt törlése;
- module-store/CBOR dead scaffolding eltávolítása;
- intern-pass és const-eval twin ágak összevonása;
- egyetlen total child iterator;
- `ir_stream` section-entry offset konstansok és golden image teszt;
- benchmark-generátorok és CPU oracle-ok közös tulajdonba rendezése.

Nem bontunk fel hash- vagy FP-hot loopot pusztán LOC miatt. A hosszú, kohézív,
sorrendérzékeny mag jobb lehet, mint egy általánosított, nehezebben mérhető
abstraction.

## 11. Történeti állítások, amelyeket ez a stratégia felülír

1. A `HyperGraphView`/star-CSR mint standard statikus gráfút történeti PoW volt;
   az irányadó út direct canonical IR + exact sparse incidence index.
2. A „CBOR a következő kötelező codec” állítás visszavonva; az `ir_stream`
   sectioned snapshot a fő wire.
3. A „generációs aréna a következő automatikus migráció” visszavonva; a mért
   lookup-regresszió miatt előbb deletion-capable storage study kell.
4. A `B^T y ≈ 10× Bx` állítás visszavonva; quiet-hoston 3.4–4.9×.
5. A clique/2-section nem „dense star”; külön kontroll, kvadratikus aritásban.

## 12. Hivatkozás későbbi promptokban

Minden új Claude/Codex feladat eleje tartalmazza ezt:

```text
GOVERNING-STRATEGY: docs/hymeko-platform-strategy.md
STRATEGY-ID: HYMEKO-PLATFORM-2026-09-01-v1.1
ROADMAP-TASK: P<n> / <rövid slug>

Olvasd végig a governing strategy-t, majd a CLAUDE.md, CORE.YAML és tools.yaml
aktuális változatát. A kód és tesztek döntik el a jelenlegi implementációt.
Ne hozz létre második IR-t, párhuzamos gráfmodellt, codec-sémát,
benchmark-frameworköt vagy termék-CLI-t. Eltérés előtt HARD STOP és
bizonyítékkal alátámasztott döntéskérés.
```

A prompt ezen felül nevezze meg a base refet, az engedélyezett repositorykat és
fájlokat, az approval tokent, a mérési cellákat, az acceptance gate-eket, a
commit/push engedélyt és a kötelező HARD STOP pontot.

## 13. Változtatási protokoll ehhez a stratégiához

- Új mérés nem írhatja át csendben a régi állítást: additív correction/retraction
  és verzióemelés kell.
- A stratégia fájlútja stabil marad; a `Strategy ID` változik (`v1.1`, `v2`).
- Architektúradöntés csak falsifierrel és mért trade-offal zárható le.
- Egy feature branch eredménye nem „shipped”, amíg a célbranch ancestryje,
  gate-je és remote refje nincs igazolva.
- Ez a dokumentum nem engedélyez core-editet; minden lockdown változás külön
  `APPROVED-CORE-EDIT` tokent igényel.

## 14. Bizonyítékjegyzék a v1-hez

| Ref | Tartalom | Állapot a bizonyítékhatárnál |
|---|---|---|
| core `70b4f36` | exact incidencia-koordináták, sparse operator | `origin/main` |
| core `fb6b783` | WebXR semantic contract, történeti implementáció | ownership szerint superseded: relocation a transformation rétegbe |
| core `584b9eb` | performance survey + MCU terv | feature branch |
| core `093eab3` | generációs aréna deferral + architektúra audit | eltérő feature branch; döntési bizonyíték |
| graph `98c6a93` | direct-IR algoritmusok | feature branch |
| graph `cbaadd4` | graph operation performance survey | feature branch |

Ezek timestampelt bizonyítékok. Minden végrehajtási prompt a remote refeket újra
ellenőrizze; ne feltételezze, hogy a táblázat örökké aktuális.


## 15. Elfogadott teljesítmény-folytatás — 2026-09-09

Rögzítve: 2026-09-09T18:54:44+02:00. A felhasználó elfogadta az alábbi sorrendet és az első
lépés végrehajtását. Ez a jelenlegi teljesítménymunka prioritása; a korábbi
architekturális invariánsokat megtartja. Négy fő repó: Core, Graph,
Transformation, Tensor; Parser a szintaxis tulajdonosa, Python/JS/WASM vékony kötés.

| Lépés | Feladat | Elfogadási feltétel | Állapot |
|---|---|---|---|
| PERF-1 | A mért optimalizációk integrálása a kurzusjavításokkal | Rögzített Parser/Core/SDK revíziók; helyesség és hash-paritás; ugyanazon forrásverzió natív/WASM ellenőrzése; érintett teljesítménymérések; GitHub és LAN Git publikálás | folyamatban |
| PERF-2 | AST-felépítés és második bejárás költségének csökkentése | Profil által indokolt változás, azonos kimenet és diagnosztika; AST és saját tulajdonú rekordok külön mérve | tervezett |
| PERF-3 | Közös gráfbenchmark külső könyvtárral, első jelölt petgraph | Azonos algoritmus, cél, súly, eredmény; hipergráf-leképezés és indexépítés külön költség | tervezett |
| PERF-4 | CPU/WGPU/CUDA numerikus összevetés | Azonos művelet és numerikus oracle; cold setup, H2D, resident kernel, D2H és teljes idő külön | tervezett |

### PERF-1 végrehajtási keret

A már engedélyezett compiler-repeated-work és course-module-compilation munka
összevezetése. Nincs új algoritmus, új AST/API vagy GPU-kernel ebben a lépésben.
A kiadott kurzusjavítás a kontroll: Parser a528d98, Core 8623eb1, SDK b78a1bc.
A helyi teljesítményváltozások forrása és korábbi profiljai a kapcsolt riportok.
A Core LAN main külön sparse-tensor commitja (262fc0b) nem része e változásnak;
a kiadási ág frissítése nem jelent alapági merge-et.

Megőrzendő: modulonkénti aliasok, tranzitív importok, normalizált útvonalak,
UTF-8, forrásproveniencia, DeclId/NodeId különbség, incidenciasorrend, előjel,
súly, multiplicitás, strukturális és szemantikai hash, első-hiba sorrend.
A natív és WASM buildprofil eltérhet; forrásrevíziójuknak egyeznie kell.

### Mérési és közlési szabály

Generálás, I/O, parse, AST→IR, hash/validáció, indexépítés, runtime és teljes út
külön határ. AST vs JSON DOM nem azonos rekorddekódolási feladat. Nem adunk össze
külön mért mediánokat, és nem állítunk előnyt külső könyvtárral szemben belső
A*/Dijkstra arányból. QEMU nem natív ARM eredmény. A kiadott és kísérleti
változat a grafikonon azonosítható SHA-val szerepeljen.

Az aktuális fejlesztői kiindulópont: saját tulajdonú rekordoknál 1,37–3,30-szoros
JSON-idő a 2026-09-08-i natív 50k mediánokban; nested instabil. Ez nem a
2026-09-09-i kurzuskiadás teljesítményígérete. Korábbi XML/YAML számokat az új
változat eredményeként tilos feltüntetni.

Bizonyítékok: ../parser/reports/2026-09-07-format-comparison.md,
reports/2026-09-07-compiler-repeated-work.md,
reports/2026-09-09-course-module-compilation.md.
A nyers mérések és riportok gitignored állományok; minden feladat után a
közös Samba-archívumba kerülnek. A stratégia és a terv Gitben marad.
