# HyMeKo platformaudit

Created-at: 2026-09-05 02:57 CEST. Állapot: az engedélyezett implementáció és mérési kampány lezárva; a külön Berge-jóváhagyás függőben.

A terv: ../docs/plans/2026-09-05-platform-audit/plan.tex. Saját worktree-k: D:/hnnwork/platform-audit/{core,graph,transformation}. Az eredeti munkafák módosítása tiltott. Védett Core-fájl nem változik.

## Vezetői álláspont

**Feltételesen folytatható; public release-re még nem kész.** A kanonikus Core IR és CBOR façade megfelelő tulajdonos marad. A Graph három, teljes workspace suite mellett is fennálló hibáját külön ellenpéldák bizonyították és a jóváhagyott javítás megszüntette. A Chordex → Core decode → Transformation → Simulator adatút valódi loopback TCP-n működik; a teszt a modell kapacitását, a kamera és fiducial adatait, a duplikációt, coalescingot és explicit reconnect-resyncet is ellenőrzi.

A korábbi Core teljesítményállítások csak részben reprodukálhatók stabilan. A közepes/nagy CBOR decode új mérései közel esnek az idézett értékekhez; a kis encode és a nagyobb canonical-hash cellák erősen ingadoznak. Ezekből nem születik új gyorsulási vagy regressziós állítás. A WGPU readback összehasonlítása azonos inputon, külön folyamatokban méri a már meglévő staging-újrafelhasználás értékét; nem új kernelgyorsítás.

## Lényegi megállapítások, súlyosság szerint

1. **Magas, javítva:** directed Dijkstra eldobott egy későbbi érvényes belépést és a duplikált node incidenciájának producer slotját. Bizonyítottan admissible/consistent A* 12 költséget adott a helyes 11 helyett. Három vörös külső ellenpélda, majd három zöld regresszióteszt és teljes Graph suite a bizonyíték.
2. **Magas, integrációban kezelve:** a `canonical_program_hash` v1 strukturális hash: skeleton + incidence. A szokásos node-attribútum `capacity: 2.0 → 3.0` változása nem változtatja meg. A hash nem teljes payloadazonosító és nem hitelesítés. A hash-alapú dedup javaslatot a diszkrimináló teszt megcáfolta; a végleges ingress **Core-validálás után pontos CBOR-byte-egyezést** használ. A Core hash/wire szerződés nem változott.
3. **Közepes, külön jóváhagyásra vár:** Berge `max_results=0` mellett egy eredményt bocsát ki; a max-depth miatti metszés `bound=None` marad. Két külön futó ellenpélda bizonyítja. A lent megnevezett token nélkül e védett fájlok érintetlenek.
4. **Közepes, javítva:** a CBOR model subscription korábban korlátlan FIFO-t épített. Most egyetlen pending snapshot, 64 MiB alapértelmezett, konfigurálható limit és megfigyelhető counters vannak. A generic sensor/source FIFO-k továbbra sem boundedek.
5. **Közepes, mérési hiba javítva:** WGPU warm-prepare submit nélkül gyűjtött staging allocationöket; három eredeti futásban több mint 5 GiB peak working set jelent meg. A javított `prepare_warm_cache_completed` minden iterációt befejez. A régi tiszta prepare és az új completed cella eltérő időhatár, közvetlen latenciadelta nem közölhető.
6. **Közepes, prototípusban javítva:** CUDA ismeretlen/hibás numerikus CLI argumentumot elfogadott; a kezdeti payload-guard mellett egy unclamped `reserve(rows*avg_deg)` túl nagy foglalást engedett volna. A reservation clamp, u32/payload/valós eszköz-launch limitek, benchmark előtti kötelező oracle és hibakódok javítva. A veszélyes nagy foglalást a javítás előtt nem futtattuk.
7. **Release-feltétel:** Chordex eredetileg Git nélküli könyvtár, Simulator több tucat módosított/untracked ingress fájllal. A reprodukció saját, SHA256-ellenőrzött és helyileg verziózott snapshotból történt; ez nem public clean-clone bizonyíték. Az új resolver a Chordex lockban meglévő, yanked `spin 0.10.0` bejegyzést igényelte; ez a dependency-karbantartás külön lelete.
8. **Meglévő kapuhibák:** Transformation és Simulator több fmt/Clippy eltérése; Chordex teljes workspace-ben `Quality::suffix(self)` nem-Copy fieldet mozgat shared reference mögül. Ezek nem a transport termékkódjának hibái, és nem lettek globális allow-val elrejtve. A Transformer négy release-teszthibájából kettő CRLF-érzékeny negatív kontroll, kettő debug-assertiont release-ben elváró teszt volt; célzott javításuk zöld.

## Forrásazonosság és ownership

Az eredeti állapot részletes, géppel olvasható leltára: [repositories.json](platform-audit/repositories.json), környezet: [environment.json](platform-audit/environment.json). A Graph `.git/REBASE_HEAD` marker elavult volt, aktív rebase könyvtár és folyamat nem volt; önmagában a marker nem merge/rebase-blokkoló.

| Projekt | Ellenőrzött eredeti állapot | Használt saját alap |
| --- | --- | --- |
| Core | `integrate/contract-cbor-webxr-clean`, tiszta, `0a34a976457fdc775abe180295ab0ac70598a89f` | ugyanaz, `audit/platform-2026-09-05` |
| Graph | helyi main `29087858`, 37 committal lemaradva; IP-s fetch után origin/main `b234c4871807dc255854cc4b50d1458547dca7d3` | `b234c487`, `audit/platform-2026-09-05` |
| Transformation main | tiszta `0d78d403983b9cd272e9fcac2c17e79467a611e7` | nem erre másoltuk vissza a WebXR-t |
| Transformation WebXR worktree | `feat/webxr-domain-projection`, tiszta `4298cfca0ed12a734f0792b33d434f47b165b4fe` | `4298cfca`, `audit/platform-2026-09-05` |
| Axon | tiszta main `bdbf837f87a309536105911aa812a59921adb577` | csak olvasás |
| Chordex | nincs `.git` | snapshot `50fff37`, saját worktree `integration/work/soma_chordex` |
| Simulator | `chore/contract-tooling@b7a21c7`, nincs upstream, 36 módosított + 29 untracked bejegyzés | snapshot `a4d03ed`, az eredetileg ignorált lock megőrzése `2eddbde`; `integration/work/hymeko_simulator` |
| Simulator Core dependency | a tényleges manifest `core-portable-id@93b1f114afa9a31643744d30b31cc45437cc3e04`-et választ | külön, változatlan saját worktree; nem cseréltük a benchmarkolt Core-ra |
| XR kiegészítő dependency | `86b32483` + helyi változások | forrássnapshot `33bb773`, módosítás nélkül |

A snapshot fájlonkénti SHA256 leltára: [integration-source-snapshot.json](platform-audit/integration-source-snapshot.json). Másoláskor forrás-előtte, forrás-utána és cél hash egyezett. A saját snapshotok és worktree-k az `integration/{snapshots,work}` fa alatt vannak; a sibling dependencyk saját checkoutokra mutató junctionökkel maradnak feloldhatók. A nagy root lockot nem cseréltük Chordex lockra: kizárólag a már rögzített spin rekorddal seedeltük, utána Cargo oldotta fel és ellenőrizte a Simulator lockot.

| Tulajdonos | Kódban ellenőrzött felelősség | Határ |
| --- | --- | --- |
| Core | `ir::Ir`, compile/lower, incidence, `writers::cbor_writer::{encode_compiled,encode,decode}` | egy IR és egy validáló CBOR façade |
| Graph | a Core borrowed incidence indexén traversal, Dijkstra/A*, Berge enumeration | nincs alapértelmezett clique/Levi másolat |
| Transformation/WebXR | dynamics, geometric/spatial projection, fiducial/camera/placement lekérdezések | domain projekció itt van; Core-ban nincs `src/webxr` vagy `data/webxr` visszamásolás |
| Chordex | `SimTransport`, `SimPublisher`, `ZenohTransport`, opaque model bytes | nem értelmezi vagy írja újra az IR-t |
| Simulator | validáló ingress komponálása, derived model és runtime telepítése | a bytes életciklusa és az idempotencia alkalmazási felelősség |
| Axon | Python/Gazebo/MJPEG/nginx deployment és hidak | a Zenoh charterbeli ígéret mögött nem találtunk a Chordexével egyenértékű model transport implementációt |

A Cargo-élek nem egyetlen soros láncot alkotnak: Graph és Transformation is közvetlen Core-consumer; Chordexnek nincs Core IR-dependencyje. Simulator Core-t, Transformationt/WebXR-t és Chordexet komponál. A [Mermaid adatút](../docs/plans/2026-09-05-platform-audit/plan.mmd) és a [lefordított terv](../docs/plans/2026-09-05-platform-audit/plan.pdf) ezt az ownershipet mutatja.

## Architekturális döntések

**Graph.** Az exact incidence arc/slot párokat zipeljük. Csak exit-independent cost és entry-independent policy esetén osztható meg az arc legjobb belépési g-je; javuló g A* alatt újraexpandálhat. Az új default `entry_independent(&self)` metódus konzervatív és megtartja a trait dyn-kompatibilitását. A workspace többlete 8 B/retained arc slot. Az általános once-per-arc/lineáris állítás A*-ra és entry-dependent policyre nem érvényes; ez helyesbíti a 2026-09-01 semantic-contract általános megfogalmazását. A heuristic `CONSISTENT` deklarációját a runtime továbbra is a callertől fogadja el; nem bizonyít tetszőleges heuristicot.

**Prepared incidence ágak.** `feat/core-incidence-operator-precompute@52c2649` történeti ~1M-incidence számai: Bᵀ 13.3 → 0.68 ms, preparation 15.3 ms, +8 B/incidence. A számolt megtérülés 1.21 használat, tehát legalább két hívás; ez nem újramért production E2E eredmény. A prepared forma elutasít NaN/Inf koefficienst, az unprepared propagálja: a domain eltér. Ismételt production consumert a vizsgált repókban nem azonosítottunk. `feat/core-canonical-incidence-backbone@64c79b2` ownership-refaktor, nem igazolt lényegi gyorsulás. Egyiket sem merge-eltük; a szűk átvételhez consumer-mérés és új, pontos Core-token kellene.

**D* és Berge.** D*/LPA* implementáció és konkrét dinamikus consumer nem került elő, így nincs kitalált benchmark. Berge egyszerű path/cycle outputot vektorba gyűjt; költsége a kibocsátott teljes lépésszámmal is nő, az arc-expansion budget pedig aritásonként eltérő munkát fed. A limiter ellenpéldái és a bounded diagnosztika nem jelentik a kombinatorikus robbanás megszüntetését.

**WGPU.** A profil alapján a readback életciklusa a szűk fejlesztési pont. A production újrafelhasználható workspace és queue-ordering már létezett; a változtatás a helyes használatot mérhetővé és regresszióteszteltté teszi. Nincs új incidence-kernel vagy erőltetett CUDA/Rust binding. A cold-library-cache cella pipeline create+drop, megtartott driver cache mellett; a completed CSR upload allocation+validation+upload+wait+drop, nem tiszta DMA-idő. A resident F cella host submit+completion, a phase7 timestamp K2 ettől külön kernel-GPU-intervallum. CPU adapter fallback nem jelenhet meg GPU-számként.

**CUDA.** A meglévő `tools/cuda_prototype` marad önálló f32 CSR×dense execution probe. Nincs CUDA-IR, canonikus serializer vagy új Rust dependency. A CPU-oracle regular/irregular és signed/duplicate/empty/nonfinite kontrollokat fed. A portolt RNG azonos seed/nnz kontrollt használ, de egy teljes, byte-szintű Rust/CUDA input-KAT export nincs implementálva; a prototype nem kanonikus-IR production backend. A cold setup/H2D/első hívás és a resident event/host/D2H/input-to-output időhatárok külön szerepelnek. A profiler alatti idő nem kerül normál benchmarkként a táblába.

**C/C++.** Most nem indokolt új HyMeKo C ABI. A jelenlegi hosted út Rust/WASM, a Chordex C headerje a saját chord-analysis API-ját szolgálja, nem egy HyMeKo IR-fogyasztót. Ha valódi hosted C consumer jelentkezik, az A irány (opaque Rust decode handle, explicit lifetime/allocator ownership, vékony RAII wrapper) az első jelölt; a C11 MCU view nem készül automatikusan mellé. C ABI smoke/ASan/UBSan itt N/A, mert ilyen új artifact nem készült.

**Transport: Chordex.** Az Axonnal szemben itt van működő trait/publisher, Zenoh backend, pontos topic és Simulator dependency. A [transport contract](../../integration/work/soma_chordex/docs/hymeko-zenoh-contract.md) rögzíti a topicot, 64 MiB defaultot, egy pending Putot, Delete/oversize kezelését, counters-t, arrival orderinget, explicit producer resyncet és az ACL/session határt. Nincs durable history, automatic replay, revision-based stale rejection vagy application install acknowledgment. A canonical hash nem payload-hitelesítés. A browser helper 64 MiB decode-capet és legfeljebb egy installed CBOR-cache-t tart; a tényleges böngésző render/solver-state megőrzés külön futtatási kapu marad.

## Tesztbizonyíték

Összesítés: [test-summary.json](platform-audit/test-summary.json). Az alábbi idő a tesztcélok által jelentett idők összege, nem buildet is tartalmazó parancsfalióra. A mérési folyamatok saját wall/RSS adatai külön `*.memory.json` fájlokban vannak.

| Suite | Baseline pass/fail/ignored | Utána pass/fail/ignored | Összegzett tesztidő utána |
| --- | ---: | ---: | ---: |
| Core workspace release | 525/0/7 | 527/0/7 | 18.91 s |
| Graph workspace release | 478/0/6 | 481/0/6 | 3.33 s |
| Transformation workspace release | 464/4/9 | 466/0/9 | 4.76 s |
| WGPU SpMM célzott | meglévő suite baseline része | 15/0/0 | 1.80 s |
| Chordex transport | 5/0/2 | 9/0/0, a két élő teszttel | 0.19 s |
| Simulator model ingress | 4/0/0 | 6/0/1 diagnosztikai probe | 0.11 s |
| Transformer debug-only panic kontroll | release-ben két hibás elvárás | debugban 2/0/0 | <0.01 s |
| JS ingress | külön baseline nem szükséges, JS nem változott | 14/0/0 | 94.76 ms |

Fő parancs Core/Graph/Transformation gyökérben: `cargo test --workspace --release --locked --offline --no-fail-fast -j 6`. Chordex: `cargo test -p chordex-transport --release --features zenoh --locked --offline -- --include-ignored`. Simulator: `cargo test --release --locked --offline --no-default-features --features zenoh-ingress --test model_ingress -- --nocapture`. WASM: `cargo check --lib --target wasm32-unknown-unknown --no-default-features --locked --offline -j 6` PASS. JS: a telepített Node **24.19.0** binárisával `node --test web/zenoh-ingress.test.mjs` PASS; a PATH első nvm shimje nem volt konfigurálva.

Core és Graph `cargo fmt --check`, valamint workspace/all-targets Clippy `-D warnings` PASS. Chordex transport Clippy PASS. Transformation workspace fmt és Clippy FAIL meglévő, a javított sorokon kívüli eltérésekkel; Simulator Clippy FAIL 14/16 meglévő library/test warning miatt. Ezeket a nyers logok név szerint tartalmazzák; nincs széles lint-kikapcsolás. A módosított Rust-fájlok külön rustfmt-ellenőrzése PASS.

Skip-osztályozás: commit gate a célzott correctness/fault/interop teszt. Nightly az optional hosszú stressz/deep-chain és Python/Torch integráció. Release gate a teljes releváns workspace, clean-clone/packaging, valós browser, fuzzing és saját gépen mért benchmarkmátrix. A golden-regeneráló tesztek nem automatikus gate-ek: szándékosan írnak oracle-t, ezért nem futnak `--ignored` tömeges kapcsolóval. Transformation `IRDelta::Batch` atomikussága és editor layout roundtrip ignorált tesztjei dokumentált, fennálló korlátok; a zöld suite nem jelenti megoldásukat. A benchmarkként ignorált Core/Graph/WebXR cellák külön, felügyelt folyamatokban futnak.

## Mérési protokoll és értelmezés

Host: Windows 11 Home 10.0.26200; Core Ultra 9 275HX, 24 logikai CPU; 68,097,818,624 B RAM; RTX 5080 Laptop, 16,303 MiB, Vulkan, NVIDIA 610.88, f32. Rust/Cargo 1.97.1, LLVM 22.1.6. CUDA 13.2.86, MSVC 19.51.36252, `nvcc -O3 -arch=sm_120`. A Rust build release/bench, a Rayon kontroll 24 thread. A feature-ket a buildlogok, az executable SHA256-ot a memóriafelügyelő rögzíti; normál méréssel build/profiler nem fut párhuzamosan.

Három külön folyamatból származó ismétlés; Criterion 3 s warmup, 20/30 sample, jellemzően 5/6 s ablak (a nagy construction cella 10 sample/8 s). A tényleges raw `sample.json` + `benchmark.json` + `estimates.json` az elsődleges adat. [criterion-summary.csv](platform-audit/criterion-summary.csv): átlag, medián, IQR, p95/p99, worst, variancia, CV és median CI. **A Criterion-quantilis iterációkötegenkénti átlagok quantilise, nem egyedi kérés tail latency.** A transport külön egyedi kérésmintákat ír, 5 warmup + 100 request/cella.

Instabilitási jelzés: 10% feletti run-medián range/medián vagy 10% feletti within-run CV figyelmeztető auditküszöb, nem hipotézisvizsgálat. Az eltérő gépen vagy eltérő életcikluson mért értékekből nincs százalékos regresszió. A régi report gyorsulási headline-ja nem aktuális mérési eredmény. RSS Windows OS high-water counterből, 100 ms-os pollal; a legutolsó ablak korlátja dokumentált, VAS nincs limitálva. A régi körök utólagos cap-assertiont, az új körök futás közbeni stopot használtak. Egy megfigyelt nulla nem elfogadott memóriaeredmény.

### Core történeti értékek ellenőrzése

| Cella | Feladatban idézett közelítő érték | Három új mintamedián | Következtetés |
| --- | ---: | ---: | --- |
| Hash 1.5k decl | 0.714 ms | 0.703 / 0.700 / 0.760 ms | nagyságrend reprodukált; sample-zaj |
| Hash 15k, arity 4 | 7.77 ms | 11.070 / 8.112 / 9.373 ms | instabil, pontos headline nem igazolt |
| Hash 15k, 160k incidence | 15.17 ms | 17.036 / 16.189 / 18.746 ms | instabil, pontos headline nem igazolt |
| Encode small | 29.7 µs | 54.245 / 33.250 / 28.588 µs | erősen instabil |
| Encode medium | 102.6 µs | 96.413 / 102.788 / 93.657 µs | közelítő tartomány, határeset sample-zaj |
| Encode large | 607 µs | 622.875 / 632.451 / 590.474 µs | közelítő tartomány reprodukált |
| Decode small | 5.83 µs | 6.791 / 6.940 / 6.761 µs | stabilan magasabb; az idézett érték nem reprodukált |
| Decode medium | 123 µs | 123.714 / 124.371 / 123.592 µs | stabilan közel |
| Decode large | 1.114 ms | 1.142 / 1.143 / 1.144 ms | stabilan közel |

A Core CBOR-folyamatok megfigyelt peake 14.6–17.8 MiB, hash-probé 63.6–63.7 MiB volt. A megtartott canonical IR allokációja és a process peak nem azonos mérőszám. A részletes minták és a további cellák alább, illetve a csatolt CSV/JSON fájlokban szerepelnek.

### Core lowering, incidence és memória

Construction (1k/10k/100k node, arity4): 101.899 µs / 2.125 ms / 30.110 ms, három futás mintamediánjának mediánja; 10k/arity32:3.418 ms. CSR SpMM 16k nnz/fd32: serial48.349 µs, Rayon24 26.809 µs, azonos outputtal. A corpus load+lower 8256 declarationre 40.52/40.73/39.90 ms. Ez utóbbi meglévő median-of-15 diagnosztika, egyedi p95 nélkül. Peak: corpus8.76–8.95 MiB, construction~67 MiB, CSR~95 MiB.

| 1M-incidence fixture | Index build, 3 medián | Bx / Bᵀy / BᵀBx, mediánok mediánja | Retained index / teljes clique |
| --- | ---: | ---: | ---: |
| 500k node, 200k arc, arity5 | 20.356/20.012/19.431 ms | 5.854/29.112/35.546 ms | 14,800,008 / 18,000,004 B |
| 50k node, 50k arc, arity20 | 15.986/15.383/15.716 ms | 4.785/17.302/22.270 ms | 12,400,008 / 76,200,004 B |

Process peak~262.9 MiB. A magas aritású teljes clique 19M leget épít, 55.228–57.129 ms alatt és 6.1× retained memóriával. Ez az unprepared operátor, nem a történeti prepared ág átvétele.

A regenerálható CBOR memóriafixture 51,422,046 B. Read-only baseline peak53.93–53.94 MiB; materialize144.88–144.90 MiB; teljes validált decode148.75–148.76 MiB. Mindkét IR-kép becsült retained heapje94,221,761 B. A ~3.9 MiB peak-különbség nem allocator-call-szám. A 250 ms retention-hold benne van a processzidőben, ezért az nem decode-latencia. [Fixture-méretek és SHA256](platform-audit/fixture-summary.json).

### Graph diagnosztika

Az azonos 200×200 grid d=80 esetén baseline Dijkstra1143.2 µs/A*L1 6.4 µs; utána három medián Dijkstra1168.3/1170.5/1190.9 µs, A*L1 7.1/6.9/7.2 µs. Azonos helyes költség, expanded-state11931 vs81. Egy baseline-run és kilencmintás diagnosztikai mediánok nem bizonyítanak szignifikáns regressziót. A window arity5 1k/10k/100k költsége továbbra is250/2500/25000, BFS/Dijkstra egyezéssel.

A végleges direct survey `--test-threads 1` mellett futott; a kezdeti két párhuzamos diagnosztika exploratívként megmarad. 200k-node/arity5 run1: direct13.818 ms, sparse-star1.046 ms, clique0.662 ms; build13.973/15.921/49.455 ms. 50k/arity20: direct9.035 ms, star0.654 ms, clique0.515 ms; build12.479/12.664/107.200 ms. **A direct reprezentáció itt nem a leggyorsabb traversal.** A Graph window-clique deduplikált szomszédságot tárol, így retained memóriája nem azonos a Core teljes clique-leg mérésével. Peak182.2–183.6 MiB; három végleges log: `graph-direct-serial-run*`.

| Diamond-lánc d | Teljes lehetséges output | Kibocsátott path / összes arc-lépés | Bound | Medián |
| --- | ---: | ---: | --- | ---: |
| 2 | 4 | 4/16 | None | 0.70 µs |
| 4 | 16 | 16/128 | None | 1.70 µs |
| 8 | 256 | 256/4096 | None | 43.75 µs |
| 12 | 4096 | 4096/98304 | Results | 806.60 µs |
| 16 | 65536 | 4096/131072 | Results | 962.85 µs |

Három processz, 3 warmup +20 egyedi minta/cella; minden eredményszám és minden path2d hossza ellenőrzött. d16 csonkolt, nem65536 út olcsó felsorolása. d12 pontos cap elérésekor is Results boundot ad az API. Kis cellák időfelbontás/zaj, több cella CV miatt instabil; p95/p99/variancia a CSV-ben. Rövid processz100 ms-os RSS-pollja nem megbízható tranziens maximum. A két külön zéró budget/depth ellenpélda továbbra is FAIL, jóváhagyásra vár.

### Transformation

A meglévő IR seam-probe öt skálája20/192/896/4096/20480 declaration,12/128/640/3072/16384 incidence. Writer (derived EditorIR→canonical compiler IR) és négy reader külön mért. A legnagyobb writer5.6725–6.1069 ms; HyMeKo emitter1.3315–1.4091 ms,339370 byte és stabil `564ea12064cae9f2` fingerprint. SysML2.6259–3.0954 ms, Rust1.6849–1.7415 ms, Lean2.3111–2.3572 ms. Minden kisebb skála/fingerprint a három emitter logban. Peak15.3–16.0 MiB. Ez roundtrip benchmark, nem a Simulator teljes útja.

WebXR compile/validate 100/1k/10k/100k entityn: 100k-nál compile 2.0855–2.1219 s, validation 27.94–29.03 ms, peak 767.6–767.7 MiB. A borrowed query sem globálisan allocation-free: validation registry és eredményvektor készül. A valódi TCP ingress külön ellenőrzi a fiducial-, kamera- és dynamics-adatokat; placement-pozíciót és orientációt nem állít. A stage-probe 1/128/2048 koordinátán mér. Browser render és solver restart nincs a headless update-időben.

### WGPU és CUDA

Az eredeti medium/fd32 backend-kontrollok három mediánja: CPU serial 4.090/4.364/3.807 ms (instabil); Rayon24 0.609/0.565/0.554 ms; WGPU oneshot 6.699/6.666/6.667 ms; static CSR 2.641/2.625/2.710 ms; teljesen resident F host submit+completion 154.462/156.599/156.139 µs. A különböző sorok eltérő feltöltési/életciklus-költséget tartalmaznak, nem egymás szinonimái.

A változatlan phase7 Part B önálló readback-bontása 14 méreten, 32–128 MiB között, fresh/reused és 8/16 MiB chunk variánsokkal futott: 3 warmup, 24 steady minta/variáns, három processz. Minden kiírt `bytes_ok` igaz, a 0-byte és odd-chunk kontroll is megmarad a nyers logban. Processzidő 41.497/39.237/37.541 s, peak 652.7–653.1 MiB. A 32 MiB-os fresh/reused allocation-medián 4225.45/0.10 µs, GPU-copy 1394.705/5612.880 µs, submit+wait 3227.15/5843.25 µs, map 7.80/8.50 µs, CPU memcpy 8255.15/8179.10 µs, total 15804.55/14023.40 µs. Az egyes mediánok nem összeadhatók; GPU-copy a host submit+wait spanen belüli idő. Part B csak staginget használ újra, a cél-Vec életciklusa eltér a resident benchmark teljes workspace-újrafelhasználásától. 64/128 MiB cellák a run/CV-küszöb szerint instabilak; minden szakasz nyers mintája és p95/p99/varianciája a diagnostic CSV-ben. Nincs univerzális readback-gyorsulási állítás.

| Végleges WGPU resident readback, azonos medium/fd32 | Három mintamedián |
| --- | ---: |
| Fresh readback | 2.904/2.881/2.809 ms |
| Reused, két várakozás | 0.824/0.816/0.823 ms |
| Reused, egy várakozás | 0.767/0.757/0.758 ms |

Azonos encode+submit lépés közös closure-be került; az érintett három cellát utána ismét3× megmértük, forgatott sorrendben. A két reused span stabil, a fresh CV miatt instabil. A reused egy-várakozásos út mediánja~0.758 ms; ez a meglévő readback API helyes életciklusának előnye, nem új kernelgyorsítás. Process peak631.7–632.9 MiB, a kiválasztatlan harness-setupokkal együtt. Cold library pipeline248.076 µs (driver cache megtartott); completed warm prepare69.495 µs, CSR upload132.090 µs, input upload125.543 µs. E setup-cellák előző végleges kódból származnak, a submit-closure konszolidáció nem érinti őket. A régi warm-prepare több mint5 GiB-os felhalmozását az új span megszünteti; eltérő időhatár miatt nincs prepare-latenciadelta.

| CUDA span | Regular200k/deg8/fd32 | Irregular200k/deg32/fd32 |
| --- | ---: | ---: |
| Cold allocation/setup,3 processz | 54.655–57.652 ms | 52.183–54.397 ms |
| Cold H2D completed | 3.698–3.894 ms | 8.251–8.738 ms |
| Cold első kernel, host | 238.3–259.2 µs | 809.9–816.2 µs |
| Resident kernel-event, mediánok mediánja | 113.856 µs | 594.544 µs |
| Instrumentált kernel-host | 120.450 µs | 605.750 µs |
| D2H reused host buffer | 1.998 ms | 1.966 ms |
| Steady X-upload+kernel+readback,CSR resident | 3.908 ms | 4.413 ms |
| Process peak | 210.97–211.02 MiB | 265.40–265.47 MiB |

Az50mintás kernel-event és irregular D2H cellák instabilak; steady E2E stabil az auditküszöbön. Mindhárom regular/irregular CPU-oracle PASS. [Cold adatok](platform-audit/cuda-cold-summary.csv). WGPU valódi timestamp kontroll:200k/deg8/fd8 47.810 µs,fd32 183.375 µs,fd128 1184.770 µs; uniformdeg32/fd32 566.370 µs. WGPU/CUDA fixture-byte azonosság nem bizonyított, így nincs backend-gyorsulási százalék.

Compute Sanitizer irregular4099×4093/fd17: **0 error,0 bytes leaked**, CPU-oracle PASS. Nsight LaunchStats/Occupancy: `ERR_NVGPUCTRPERM`,exit1; nincs számláló-jogosultság. Occupancy nem mért, device-jogosultság nem változott. [Memcheck](platform-audit/cuda-memcheck.log), [Nsight](platform-audit/cuda-nsight.log), [Kato/runbook](platform-audit/reproducibility.md). Instrumentált idők nem keverednek a normál táblákba.

### Transport összehasonlítás és E2E

| Szempont | Axon auditált model-lane | Chordex választott út |
| --- | --- | --- |
| Charter/implementáció | deploy,Gazebo,stream/bridge | Rust typed és byte transport |
| Zenoh/runtime/dependency | charterígéret; egyenértékű model implementation nem talált | meglévő session/backend/publisher; Simulator közvetlen consumer |
| Backpressure/latest | nem igazolt | egy pending Put/coalescing; generic FIFO változatlanul korlátlan |
| QoS/ack | nem igazolt | Zenoh delivery; nincs application-install ack |
| Reconnect/replay/history | nem igazolt | explicit producer full snapshot; nincs automatic replay/durable history |
| Ordering/duplicates | nem igazolt | callback arrival order; consumer exact-byte dedup; egy authorized producer/topic |
| Payload/malformed/schema | nincs új implementáció | 64 MiB limit másolás előtt; kizárólag Core validál |
| Metrics/access control | deployment feladat | mailbox counters; session/router ACL/TLS, a hash nem hitelesítés |
| C ABI/tesztelhetőség | jelen út nem indokolja | Rust fogyasztóhoz nem szükséges; valódi TCP és model assertions |

Topic:`hymeko/model/ir/cbor/v1`. A transport nem értelmezi az IR-t. A szemantikai teszt74181-byte spatial modellen4koordinátát,2fiducialt,ID17-et,URI-t és `camera0/logitech_optical`,Rgb8/sRGB adatot ellenőriz. Capacity2→3 ugyanazon strukturális hash mellett új modell, exact duplicate az implementált web-ágban nem installál újra. A web-ág WASM-fordítása bizonyított; valódi browser-viselkedése külön gate.

| Payload / transport | E2E p50,3run mediánja | Egyedi kérés p95 tartomány | Egyedi p99 tartomány | Mért teljes ciklus throughput |
| --- | ---: | ---: | ---: | ---: |
| 2268B / loopback | 53.40 µs | 57.21–65.62 µs | 58.12–89.82 µs | 16654–17093 request/s |
| 2268B / TCP | 94.65 µs | 114.99–155.95 µs | 140.34–221.22 µs | 8890–9849 request/s |
| 57214B / loopback | 1.0972 ms | 1.2346–1.3039 ms | 1.2812–1.5138 ms | 841–873 request/s |
| 57214B / TCP | 1.2343 ms | 1.3614–1.4340 ms | 1.4273–1.7940 ms | 764–780 request/s |
| 916106B / loopback | 17.5142 ms | 18.4796–19.2766 ms | 19.1361–19.5803 ms | 53.01–57.45 request/s |
| 916106B / TCP | 17.2319 ms | 19.4094–20.1625 ms | 19.9552–20.8629 ms | 55.03–56.41 request/s |

5warmup+100kérés/cella,3processz. Throughput a100kérés tényleges teljes ciklusidejéből, benne az assertion/log/drop overhead; soros, zárt ciklus, nem maximális hálózati terhelés. A külön encode/publish/receive/decode/projection/update átlag,medián,p95/p99,variancia a [diagnostic CSV-ben](platform-audit/diagnostic-summary.csv); throughput külön [CSV-ben](platform-audit/transport-throughput-summary.csv). Kis payload különösen zajos; a nagy loopback/TCP mediánok különbsége nem bizonyít TCP-előnyt. Peak és wall az adott futás `*.memory.json` fájljában. Reconnect külön correctness-teszt, explicit resync; nem durable replay-latencia.

Ábra: [campaign.png](platform-audit/campaign.png), [SVG](platform-audit/campaign.svg). A [transport-trace.gif](platform-audit/transport-trace.gif) valódi successive request-latenciákat mutat, nem szimulált mozgást vagy browser-felvételt. A standard library összesítő és a meglévő Matplotlib3.6.3 megismételhetően állítja elő. [Teljes kampányadat](platform-audit/campaign-summary.json), [memória](platform-audit/memory-summary.json).

## Public-release-readiness

| Terület | Állapot és konkrét fennmaradó feltétel |
| --- | --- |
| API | Graph javított, Berge budget hibák külön scope; a hash strukturális jelentése consumerben dokumentálandó; pre-existing Transformer Batch/layout korlátok |
| ABI | új HyMeKo C ABI nincs, stabilitása nem állítható; Chordex saját music-C API nem helyettesíti |
| Wire | Core façade + KAT/fault/corpus a tulajdonos; új séma nem készült; hash nem minden attribútumot véd és nem sender-auth |
| Docs | tervbundle és ownership/transport contract elkészült; korábbi általános once-per-arc/allocation-free/headline állítások helyesbítve |
| CI | Core talált workflow-ja nem futtat teljes teszt/fmt gate-et; belső SSH dependencyk hosted runner-hálózata nincs bizonyítva; CI/provider pin eltér a hook-pin-től; Graph/Transformation célzott keresésben nem találtunk teljes pipeline-t |
| Reproducibility | saját snapshotok, lockok, SHA-k és nyers minták rendelkezésre állnak; public clean-clone és minden belső dependency hozzáférése nincs igazolva |
| Security/fuzzing | meglévő malformed corpus lefutott; új hosszú coverage-guided fuzz kampány nincs; ACL/TLS és több producer ordering deployment-feltétel |
| Licensing metadata | MIT LICENSE fájlok; egyes crate-ek MIT OR Apache-2.0-t deklarálnak külön Apache fájl nélkül; Core manifest license mező hiányzik. Ez metadata-egyeztetési lelet, nem jogi következtetés |
| Packaging | belső Git/moving branch és helyi path/junction dependencyk; Chordex eredeti forrás nem verziózott; `spin 0.10.0` fresh-resolution probléma |
| Semver | nincs release vagy tag; API/wire compatibility gate és a védett bugfixek rendezése nélkül nincs új stabilitási ígéret |

Következő három legnagyobb értékű lépés: (1) jóváhagyás után a Berge budget-fix, célzott teljesítmény-/output kontrollal; (2) az elfogadott Chordex/Simulator/Core-portable-id/WebXR revíziók közös, publikálható lockolt dependency-éle és clean-clone CI-je; (3) valódi browser modell-update teszt és egy azonos inputot rögzítő Kato accelerator/transport kampány. Public release, tag, main merge vagy push nem történt.

## Igazolt Graph-blokkoló és jóváhagyási csomag

2026-09-05 03:04 CEST: a változatlan b234c487 Graph release libraryre linkelt `reports/platform-audit/graph_counterexamples.rs` mindhárom ellenpéldája hibával futott. Log: `reports/platform-audit/graph-counterexamples.log`. Directed rejected-entry: None a helyes Some(2) helyett; duplicate producer slot: None a Some(1) helyett; bizonyítottan admissible és consistent A*: 12 a Dijkstra/független oracle 11 eredménye helyett. A meglévő workspace suite ettől függetlenül sikeres.

A felhasználó által jóváhagyott token: `APPROVED-CORE-EDIT: graph-directed-slots-consistent-astar`. A javítás és három regresszióteszt elkészült; az első teljes utóteszt 481 pass / 0 fail / 6 ignored. A független review után az új tulajdonság default `entry_independent(&self)` metódus, így a trait dyn-kompatibilitása megmarad.

Pontos védett fájllista a Graph saját worktree-ben:

- `hymeko_graph/src/direct_ir/informed.rs`
- `hymeko_graph/tests/informed_search_vv.rs`

Technikai cél: minden tényleges `(arc, entry-slot)` szerepének megtartása; az entry/policy/heuristic által nem igazolt globális once-per-arc lezárás megszüntetése vagy bizonyítottan helyes best-entry relaxációja. Invariáns: kanonikus IR, wire, dependencia és publikus függvényszignatúra változatlan; nonnegative cost és admissible/consistent heuristic mellett optimális költség; irányított és duplikált incidenciák szerepe megőrzött; determinisztikus path reconstruction; meglévő output cap és statisztikai mezők szerződése dokumentált.

Tesztterv: a három most piros ellenpélda beemelése a meglévő V&V-be, pozitív és negatív directed esetek, zero-heuristic A*/Dijkstra/független oracle parity, meglévő informed/direct-IR és teljes Graph suite; fmt és clippy -D warnings; azonos workloadon korábbi slot-optimalizált futás vs javított futás. Teljesítmény-visszaesést mérni és indokolni kell, az optimális út helyessége nem váltható gyors hibás eredményre.

Rollback: kizárólag saját Graph feature-worktree két megnevezett fájljának task-diffje; main, upstream, felhasználói fájl, stash és más worktree érintetlen. A reproducer megmarad a hibás baseline bizonyítékaként. A tiltás forrása: Graph `CORE.YAML` 7–10. és 23/27. sor, teljes crate-védelem, valamint a feladat védett módosítási protokollja.

## Külön Graph Berge-jóváhagyási csomag

2026-09-05 03:35 CEST: változatlan Berge implementációra linkelt külső reproducer: 0 pass / 2 fail. `max_results=0` mellett 1 eredmény; kétlépéses út `max_depth=1` mellett üres eredmény, de `bound=None`. Bizonyíték: `platform-audit/graph-berge-counterexamples.log` és `graph_berge_counterexamples.rs`. Az első fordítás eltérő Cargo feature-identityjű Core rlib miatt hibás volt; az összetartozó librarypárral a reproducer lefordult, és a két termékhiba ténylegesen reprodukálódott.

Pontos további védett fájllista: Graph `hymeko_graph/src/direct_ir/berge_enum.rs`, `hymeko_graph/tests/direct_ir_optional.rs`.

Javasolt token: `APPROVED-CORE-EDIT: graph-berge-enumeration-budget-boundaries`.

Technikai terv: zéró eredménykeret ellenőrzése kibocsátás előtt a path és cycle ágban; a mélységi metszés jelzése a teljes keresés leállítása nélkül. A későbbi, még kereten belüli ágak eredményei megmaradnak. Eredmény-/expansion-limit továbbra is az egész futást állítja meg, determinisztikus elsőbbséggel. A depth jelzés önmagában nem bizonyít további kibocsátható eredményt; a pruned frontier ténye dokumentált. Nincs új IR, dependencia vagy meglévő publikus függvényszignatúra-változás.

Invariánsok és tesztek: nulla/egy/pontos eredménykeret path és cycle esetén; mélységkorlát és természetes befejezés; a metszett első ág nem rejti el a későbbi rövidebb megoldást; expansion limit elsőbbsége; meglévő diamond/triangle oracle; kis output-sensitive diamond-lánc skála, outputszám és összes lépésszám megőrzésével; célzott és teljes release suite, fmt, Clippy.

Rollback: csak a saját Graph worktree e két fájljának külön tematikus commitja; a már jóváhagyott informed-search javítás külön marad. Az engedélykérés oka a Graph `CORE.YAML` teljes crate-védelme; az előző token e fájlokra nem vonatkozik.

## Helyi commitok

A bizonyítékcsomag 1299 feldolgozott mintacsoportot és 86 memóriarekordot tartalmaz, a korábbi kontrollokkal együtt. A 3022 eredeti Criterion JSON-fájl SHA256-egyezéssel ellenőrzött [criterion-raw.zip](platform-audit/criterion-raw.zip) csomagban van; az összesítő szükség esetén visszabontja, a helyi kibontott példányok ignoráltak. [Artifact-manifest](platform-audit/artifact-manifest.json) sorolja a kért riport/terv/nyers bizonyíték fájlméreteit, sorait és SHA256 értékeit. Az executable/PDB és az 51 MB-os regenerálható target-fixture helyi buildtermék, nem Git-artifact.

Az első dokumentációs commit `cfeae63`. Végső Git-blob-ellenőrzéskor121 artifactnál CRLF/LF byte-különbséget találtunk; a root `.gitattributes` +5 soros, kizárólag e dátumozott tervre/riportcsomagra korlátozott kiegészítése megőrzi a leltározott bájtokat. A forráskód általános sorvégszabálya változatlan. A korrekció után a manifest minden tárolt artifact-hashét a Git index tartalmával is összevetjük; nem pusztán a munkafájlok létezését ellenőrizzük.

A kezdeti 45–90 perces baseline-becslést a teljes snapshot-integráció, a többszörös ellenpéldák és a kiterjesztett mérési mátrix meghaladta: a terv 02:57-kor készült, az implementációs commitok 05:22-kor, a végső readback/adatellenőrzés 05:30-kor zárult. A normál mérésekben megfigyelt legnagyobb RSS a javítás előtti warm-prepare ~5.4 GiB-os folyamata; 16 GiB-os túllépést nem észleltünk. A kis mesterséges cap-teszt szándékos leállítása nem benchmarkhiba.

A végső contract-gate: Core `402 clone(s), 7769 cloned line(s)` a `403/7800` baseline ellenében PASS. Graph exact b234 baseline `392/10048`, a végleges tesztharness `392/10021`; a provider nem módosult. A Graph hiányzó baseline-jára indított alapértelmezett duplication parancs automatikusan létrehozott egy fájlt; ezt a kizárólag saját worktree-ben keletkezett fájlt eltávolítottuk, a végső Graph-scan read-only `--measure`. Új küszöbbel semmi nincs elrejtve. Core/Graph SYMBOLS és a Simulator meglévő generátorának ellenőrzése PASS. A Core kezdeti indexeltérése részben worktree-névfüggő fejléc, részben korábban elavult tartalom volt; a provider továbbra is névfüggő marad.

A módosított Rust-fájlok scoped rustfmt-je PASS. A végső harness-konszolidáció után WGPU15/15 és Graph informed V&V17/17 célzott teszt újra PASS; a korábbi teljes workspace-számok külön bizonyítékok. Simulator Clippy16 diagnosztikája változatlan fájlra mutat, de emiatt az integration-test Clippy befejezése nem bizonyított. A memóriafelügyelő pozitív/negatív kontrolljai: gyermek Rayon-beállítás szülőmódosítás nélkül; exit7 és stderr a visszaadott eredményben/logban; 64 MiB-os allokáló child 32 MiB-os tesztlimitnél leállítva; regexes Criterion-filter. A wrapper `exit_code` mezőjét kell ellenőrizni, nem a .NET processzindítás által nem frissített shell `$LASTEXITCODE` értéket.

Új dependency kizárólag a Simulator natív, opcionális Zenoh-feature-jében: az egyébként már Chordex által használt Zenoh1.9 konfigurációs tesztfelülete. A lock214 új package-azonosságot vesz fel, a korábbi463 verzió megmarad. Core/Graph/WGPU/Transformation dependency-felület nem változik. A Chordex64 MiB limitje és counters API-ja új publikus felület; a Core IR/wire változatlan.

Az eredeti Core/Graph/Transformation/WebXR/Axon HEAD és status a végső ellenőrzésben egyezik a kiindulóval. Az eredeti Simulator közben külső változást kapott: a225 snapshotfájlból Cargo.toml, Cargo.lock és python/hymeko_sim_emit.py eltér, új overnight-robot fájlok is megjelentek. Ezeket az audit nem írta és nem állította vissza; az integráció a rögzített saját snapshoton maradt. [Végső megőrzési leltár](platform-audit/original-preservation-final.json). A hét tematikus implementációs commit helyi; release/tag/main merge/push nem történt. Rollback ezek fordított dependency-sorrendű revertje kizárólag a saját ágakon, a snapshot-alapok megőrzésével.

| Repo | Commit |
| --- | --- |
| core | `ca984692` fix(dev): supervise benchmark processes with logged working-set limits |
| core | `8940c5cb` fix(cuda): bound prototype inputs and separate verified timing spans |
| core | `063bb330` bench(wgpu): verify and compare bounded resident readback lifecycles |
| graph | `25e8c9f2` fix(graph): preserve directed slots and optimal consistent A-star paths |
| transformation | `1d7b813b` test: make variant fixtures and debug panic checks profile portable |
| chordex | `4bf70d78` feat(transport): bound canonical model snapshots with latest mailbox metrics |
| simulator | `2139aa49` feat(ingress): validate snapshot dedup and prove canonical CBOR over TCP |

Branch: `audit/platform-2026-09-05`. [Full SHA / file SHA256 / diff inventory](platform-audit/change-inventory.json).

| Repo | File | + | - |
| --- | --- | ---: | ---: |
| core | `SYMBOLS.md` | 2 | 4 |
| core | `hymeko_wgpu/benches/spmm_bench.rs` | 89 | 11 |
| core | `hymeko_wgpu/tests/spmm.rs` | 73 | 10 |
| core | `scripts/dev/measure_peak_memory.ps1` | 102 | 127 |
| core | `tools/cuda_prototype/spmm_prototype.cu` | 125 | 22 |
| graph | `SYMBOLS.md` | 539 | 0 |
| graph | `hymeko_graph/src/direct_ir/informed.rs` | 30 | 29 |
| graph | `hymeko_graph/tests/informed_search_vv.rs` | 150 | 16 |
| transformation | `hymeko_dynamics/tests/common/mod.rs` | 4 | 1 |
| transformation | `hymeko_formats/src/binary_graph.rs` | 2 | 0 |
| chordex | `crates/chordex-transport/src/lib.rs` | 4 | 0 |
| chordex | `crates/chordex-transport/src/zenoh_backend.rs` | 178 | 12 |
| chordex | `docs/hymeko-zenoh-contract.md` | 47 | 0 |
| simulator | `Cargo.lock` | 2490 | 43 |
| simulator | `Cargo.toml` | 5 | 0 |
| simulator | `SYMBOLS.md` | 79 | 4 |
| simulator | `src/model_ingress.rs` | 24 | 1 |
| simulator | `src/web.rs` | 25 | 5 |
| simulator | `tests/model_ingress.rs` | 375 | 23 |

## Discovery

Az alábbi ledgerblokkok a végrehajtás közben a beszélgetésben is megjelentek. A numerikus CSV/JSON/log és animációs kimenetek a kezdeti `reports/platform-audit/**` bizonyítéktervhez tartoznak.

```text
DISCOVERY LEDGER — Graph entry-independence szerződés
  find:    rg --files hymeko_graph/src/direct_ir -> meglévő informed.rs
  grep:    rg -n "TransitionPolicy|entry_independent" hymeko_graph/src hymeko_graph/tests -> 1 trait, 4 meglévő implementáció; entry_independent: 0
  symbols: SYMBOLS.md nem létezik; a forrás publikus traitje ellenőrizve
  read:    informed.rs TransitionPolicy, Undirected és relaxációs ciklus; független review
  verdict: EXTEND informed.rs: TransitionPolicy::entry_independent(); az új associated const helyett, mert az megszüntetné a trait dyn-kompatibilitását.
```

```text
DISCOVERY LEDGER — elkülönített transport-integráció
  find:    Test-Path D:/hnnwork/platform-audit/integration -> False
  grep:    rg -n "latest|coalesc|payload.*limit|stats|mailbox" crates/chordex-transport/src crates/chordex-transport/tests -> 0 hits
  symbols: Chordex/Simulator SYMBOLS.md nincs; meglévő Subscription, ZenohTransport, ModelContext és from_ir_cbor forrásból ellenőrizve
  read:    Chordex zenoh_backend.rs, transport.rs, hymeko-zenoh-contract.md; Simulator Cargo.toml, model_ingress.rs, tests/model_ingress.rs, web.rs, zenoh-ingress.mjs
  verdict: NEW integration/{snapshots,work}/ alatt helyi, verziózott pillanatképek és saját worktree-k; EXTEND meglévő Chordex subscription és Simulator ingress. Az eredeti piszkos Simulator és Git nélküli Chordex elutasítva szerkesztési célként, mert felhasználói munkát tartalmaznak.
```

Az előző ledger Simulator-indexre vonatkozó „nincs” állítása téves volt: a későbbi keresés elavult SYMBOLS.md-t talált. A bounded-ingress ledger már ezt rögzíti; az index végül a meglévő generátorral frissült.

```text
DISCOVERY LEDGER — Berge limit-ellenpéldák
  find:    rg --files hymeko_graph/tests -> meglévő direct_ir_optional.rs és direct_ir_common/mod.rs
  grep:    berge_enum.rs: max_results ellenőrzés push után; max_depth return bound nélkül
  symbols: meglévő EnumBudget, EnumOutcome és BudgetKind a forrásban
  read:    berge_enum.rs; direct_ir_optional.rs; meglévő külső Graph reproducer
  verdict: NEW reports/platform-audit/graph_berge_counterexamples.rs; a védett direct_ir_optional.rs szerkesztése elutasítva ebben a reprodukciós lépésben, mert az eddigi token nem vonatkozik rá.
```

```text
DISCOVERY LEDGER — bounded CBOR ingress
  find:    meglévő Chordex lib.rs, zenoh_backend.rs, tests/transport.rs; Simulator model_ingress.rs, web.rs, tests/model_ingress.rs
  grep:    latest/coalesc/mailbox implementáció: 0; Simulator from_ir_cbor: meglévő kanonikus decode és projekció
  symbols: Simulator SYMBOLS.md: ModelContext/from_ir_cbor/model_ingress 0 találat, elavult index; Chordex index nincs
  read:    az előző ledger fájljai; Core DecodeLimits; Zenoh 1.9 payload és matching API
  verdict: EXTEND a meglévő fájlok; új szimbólumok: DEFAULT_MODEL_MAX_PAYLOAD_BYTES, LatestSubscriptionStats, subscribe_hymeko_ir_cbor_with_limit, latest_stats, from_ir_cbor_if_changed. Új IR/crate helyett a meglévő ownership-határok maradnak.
```

```text
DISCOVERY LEDGER — egységes memóriafelügyelet
  find:    scripts/dev/measure_peak_memory.ps1 -> meglévő mérési segéd
  grep:    PeakWorkingSet64 és cap_16gb_ok -> jelenlegi mintavétel, csak utólagos assertion
  symbols: a meglévő PowerShell segéd nem Rust/Python publikus szimbólum
  read:    measure_peak_memory.ps1 teljes tartalma; három Core/WGPU futás memóriaeredményei
  verdict: EXTEND measure_peak_memory.ps1: közvetlen executable mód, nyers logok, futás közbeni RSS-stop és pontosított mintavételi korlát. Új mérési keretrendszer helyett a meglévő segéd bővül.
```

```text
DISCOVERY LEDGER — végső mérési összesítés és Berge-skála
  find:    reports/platform-audit/graph_counterexamples.rs és criterion-summary.csv meglévő; összesítő Python-script és Kato-manifest nincs
  grep:    direct_ir_optional.rs: meglévő diamond fixture, EnumBudget és enumerate_berge_paths; nyers CUDA/transport/Criterion minták elérhetők
  symbols: Graph publikus enumerációs API a forrásból ellenőrizve; új termék-API nem szükséges
  read:    graph_counterexamples.rs, direct_ir_optional.rs, plan.tex, a befejezett mérési logok
  verdict: EXTEND a külső Graph diagnosztikát bounded diamond-skálával; NEW reports/platform-audit/summarize.py és reproducibility.md a kért reprodukcióhoz. A védett Graph enumerátor változatlan.
```

```text
DISCOVERY LEDGER — szimbólumindexek és mérési ábra
  find:    Core/SYMBOLS.md és Simulator/SYMBOLS.md meglévő; Graph/SYMBOLS.md hiányzik
  grep:    contract-symbols --check: Core elavult, Graph hiányzó; meglévő render_policy_storage_figures.py policy-checkpointot igényel
  symbols: a provider forrásból épített indexe és a Simulator tooling/gen_symbols.py ellenőrizve
  read:    szimbólumgenerátor, duplication TokenScan, WGPU tesztharness, külső Graph reproducer, render_policy_storage_figures.py
  verdict: EXTEND a meglévő indexeket és summarize.py-t; NEW Graph/SYMBOLS.md a meglévő providerrel, reports/platform-audit/campaign.png és campaign.svg. A policy-renderelő elutasítva, mert RL-checkpointokra épül, nem e kampány mintáira.
```

```text
DISCOVERY LEDGER — izolált audit worktree-k
  find:    git worktree list; Test-Path D:\hnnwork\platform-audit -> 0 saját audit worktree
  grep:    rg -l -i "platform audit|forensic campaign" docs reports -g "*.md" -g "*.tex" -> 0 hits: none
  symbols: rg -n "PlatformAudit|platform.audit" SYMBOLS.md -> 0 hits: none
  read:    CORE.YAML; docs/agent-contract/modules/10-discovery.md; meglévő worktree-listák
  verdict: NEW D:\hnnwork\platform-audit\{core,graph,transformation}; rejected D:\hnnwork\core és a többi meglévő worktree because azok más munkákhoz tartoznak.
```

```text
DISCOVERY LEDGER — platformaudit terv és bizonyítékcsomag
  find:    rg --files docs/plans reports | rg "2026-09-05-platform-audit" -> 0 hits: none
  grep:    rg -l -i "platform audit|forensic campaign" docs reports -g "*.md" -g "*.tex" -> 0 hits: none
  symbols: rg -n "PlatformAudit|platform.audit" SYMBOLS.md -> 0 hits: none
  read:    docs/plans/2026-09-01-hymeko-platform-strategy/plan.tex; reports/2026-09-04-canonical-cbor-codec.md; reports/2026-09-04-webxr-transformation-relocation.md; scripts/dev/measure_peak_memory.ps1
  verdict: NEW docs/plans/2026-09-05-platform-audit/{plan.tex,plan.tikz,plan.mmd,plan.pdf}, reports/2026-09-05-platform-audit.md és reports/platform-audit/**; rejected a 2026-09-01 stratégiai terv because az korábbi roadmap, nem a jelenlegi commitok audit- és végrehajtási jegyzőkönyve.
```
