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
