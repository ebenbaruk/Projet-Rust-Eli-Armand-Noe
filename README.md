# CrashLend

Projet réalisé dans le cadre du cours de Rust à l'ESILV (A4 - S7).

CrashLend est une version simplifiée d'un protocole de prêt DeFi comme Aave, écrite en Rust et exécutée en simulation. Des prêteurs déposent des USDC, des emprunteurs empruntent ces USDC en déposant de l'ETH en garantie, et les positions trop risquées peuvent être liquidées. Le projet rejoue ensuite de vrais krachs de l'ETH pour mesurer si le protocole y aurait survécu.

La spécification complète est dans [`docs/SPECIFICATION.md`](docs/SPECIFICATION.md).

## Membres

- Eli Benbaruk : protocole de prêt
- Armand : krachs et liquidations
- Noé : gardien du risque et stress test

## Lancer le projet

Prérequis : Rust stable (édition 2024).

```bash
cargo run          # démarre le serveur sur http://127.0.0.1:3000
cargo test         # tests unitaires et d'intégration
cargo bench        # benchmarks criterion
```

Exemple de session avec `curl` :

```bash
H='Content-Type: application/json'
curl -X POST localhost:3000/supply  -H "$H" -d '{"user":"alice","amount":1000000}'    # 10 000 USDC
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"bob","amount":1000000000}'    # 1 ETH
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"bob","amount":150000}'        # 1 500 USDC
curl -X POST localhost:3000/price   -H "$H" -d '{"price":150000}'                      # ETH à 1 500 USDC
curl localhost:3000/positions                                                          # bob est liquidable
```

Les montants sont des entiers : les USDC sont en centimes, l'ETH en gwei et le prix en centimes d'USDC pour 1 ETH.

## Organisation du code

| Module | Rôle |
|---|---|
| `types.rs` | types du domaine : montants, prix, pourcentages, position, paramètres |
| `error.rs` | `LendingError`, l'erreur unique de la bibliothèque |
| `protocol.rs` | état du pool et opérations de prêt |
| `interest.rs` | modèle de taux et intérêts |
| `persistence.rs` | sauvegarde et chargement de l'état en JSON |
| `guardian.rs` | gardien du risque : baisse sur 60 minutes et restrictions d'emprunt |
| `stress_test.rs` | krachs aléatoires rejoués sur plusieurs threads |
| `report.rs` | agrégation des résultats et rapport du stress test |
| `api/` | routeur axum, état partagé et routes |

## Protocole de prêt

### Routes

| Méthode | Route | Effet |
|---|---|---|
| POST | `/supply` | ajoute des USDC au pool |
| POST | `/deposit` | ajoute de l'ETH en garantie |
| POST | `/borrow` | emprunte des USDC |
| POST | `/repay` | rembourse la dette (un montant trop grand est ramené à la dette) |
| POST | `/withdraw` | retire de l'ETH |
| GET | `/pool` | liquidité, dette totale, utilisation, taux |
| GET | `/positions` | positions avec facteur de santé |
| GET / POST | `/price` | lit ou modifie le prix de l'ETH |
| POST | `/save`, `/load` | sauvegarde ou recharge l'état dans `state.json` |

En cas d'erreur, la réponse a un code HTTP adapté (400, 404, 422, 500) et le corps `{ "error": "<message>" }`.

### Choix de conception

- **Des newtypes pour chaque unité.** `EthAmount`, `UsdcAmount`, `Price` et `Bps` enveloppent un `u64`. Le compilateur refuse donc d'additionner des ETH et des USDC, ou de passer un prix à la place d'un montant. Grâce à serde, ces types restent de simples nombres dans le JSON.
- **Pas de nombres flottants.** Tous les calculs sont faits sur des entiers. Les produits intermédiaires passent par un `u128` pour éviter les dépassements de capacité.
- **Une seule erreur.** Toutes les fonctions qui peuvent échouer renvoient `Result<_, LendingError>`. L'API convertit chaque variante en code HTTP grâce à `IntoResponse`.
- **Une opération refusée ne modifie rien.** Chaque opération travaille sur une copie de la position et ne la réécrit qu'une fois tous les contrôles passés.
- **Les intérêts sans perte d'arrondi.** Une minute d'intérêt sur une petite dette vaut moins d'un centime. Ce reste est conservé dans la position et reporté à la minute suivante. Sur un an, on retrouve exactement le taux annuel.

### État partagé

Le `Protocol` est détenu par un `Arc<Mutex<Protocol>>` placé dans `AppState`. axum le transmet à chaque route via `State`, et le rejeu et les bots en reçoivent une copie du pointeur. Chaque accès prend le verrou pour une opération courte et le relâche aussitôt. `/save` copie l'état et relâche le verrou avant d'écrire sur le disque. `/load` lit le fichier avant de prendre le verrou.

Un `std::sync::Mutex` suffit : le verrou n'est jamais conservé pendant un `.await`, donc il ne bloque pas le runtime tokio.

### Benchmark

`cargo bench` mesure deux boucles sur un pool de 10 000 positions :

| Benchmark | Mesure | Résultat indicatif |
|---|---|---|
| `health_factor_10000_positions` | vérifier quelles positions sont liquidables, comme un bot à chaque pas | environ 80 µs |
| `accrue_interest_10000_positions` | ajouter une minute d'intérêt à chaque position | environ 85 µs |

Ces deux boucles tournent à chaque minute simulée : elles déterminent la vitesse d'un rejeu. Pour l'améliorer, on pourrait stocker les positions dans un `Vec` plutôt qu'une `BTreeMap`, ce qui rend le parcours plus favorable au cache.

## Gardien du risque et stress test

### Gardien du risque

À chaque nouveau prix, le gardien calcule la baisse entre le prix le plus haut des 60 dernières minutes et le prix actuel. Il en déduit un mode, qui s'applique uniquement aux nouveaux emprunts :

| Mode | Baisse sur 60 min | Nouveaux emprunts |
|---|---|---|
| `Normal` | < 5 % | LTV de 75 % |
| `Alert` | 5 % à 15 % | LTV abaissé à 50 % |
| `Emergency` | ≥ 15 % | suspendus (`BorrowingPaused`) |

Les positions existantes ne sont jamais modifiées. Le gardien agit seulement par deux champs de `Protocol` : `borrowing_paused` et `max_ltv_override`. `Protocol` ne dépend donc pas du gardien.

### Stress test

Le stress test rejoue N krachs aléatoires sur le même portefeuille, une fois sans gardien et une fois avec.

- **Krach :** une chute en ligne droite de 10 à 70 % sur une durée de 30 minutes à 24 heures, avec un bruit de ±0,5 % par minute.
- **Portefeuille :** 80 emprunteurs entre 30 et 70 % de LTV, et 20 « acheteurs de baisse ». Ces derniers empruntent le maximum autorisé une fois que le prix a baissé de 2,5 %, 5 %, … ou 50 %.
- **Liquidation simplifiée :** chaque minute, les liquidateurs remboursent 50 % de la dette d'au plus 2 positions et saisissent le collatéral avec une prime de 5 %. Sans cette limite de capacité, la liquidation suit toujours un krach progressif et aucune dette irrécouvrable n'apparaît. C'est précisément ce qui a manqué lors du Black Thursday.
- **Survie :** le protocole survit si la dette irrécouvrable est nulle à la fin du krach.

Résultat avec 10 000 scénarios et la graine 42 :

| | Sans gardien | Avec gardien |
|---|---|---|
| Taux de survie | 91,2 % | 92,3 % |
| Dette irrécouvrable moyenne | 1 536,59 USDC | 939,41 USDC |

Le gardien réduit la dette irrécouvrable de 39 %, mais il améliore peu le taux de survie. Dans les krachs les plus violents, la perte vient des positions ouvertes avant le krach, que le gardien ne touche pas.

### Routes

| Méthode | Route | Effet |
|---|---|---|
| GET | `/risk` | mode, baisse observée (en bps) et état du gardien |
| POST | `/risk/toggle` | active ou désactive le gardien |
| POST | `/stress-test` | `{ "scenarios": 10000 }`, renvoie le rapport du stress test |

`POST /price` compte comme une minute pour le gardien. On peut donc le tester à la main :

```bash
curl -X POST localhost:3000/price -H "$H" -d '{"price":200000}'
curl -X POST localhost:3000/price -H "$H" -d '{"price":160000}'   # -20 % : Emergency
curl localhost:3000/risk
curl -X POST localhost:3000/stress-test -H "$H" -d '{"scenarios":10000}'
```

### Async ou threads

- **Le stress test tourne sur des threads système** (`std::thread::scope`). C'est du calcul pur, sans attente : chaque thread reçoit une tranche des scénarios et l'exécute sur son propre cœur. `scope` permet aux threads d'emprunter le portefeuille sans `Arc`. Chaque scénario travaille sur un clone de `Protocol`, donc les threads n'ont aucun verrou à prendre.
- **La route lance ce calcul dans `tokio::task::spawn_blocking`.** Exécuté directement dans un handler async, il occuperait un worker tokio pendant plusieurs secondes et ralentirait les autres requêtes.
- **Chaque scénario a sa propre graine** (`graine + index`). Le résultat est donc identique quel que soit le nombre de threads, ce que vérifie `tests/stress.rs`.

Le gardien de l'API a son propre `Mutex`, à côté de celui du protocole. Quand on a besoin des deux, on verrouille toujours le protocole en premier, ce qui exclut tout interblocage.

### Benchmark et flamegraph

`cargo bench --bench stress_test` mesure 10 000 scénarios sur 1 thread puis sur tous les cœurs. Une mesure dure plusieurs secondes : criterion fait 10 échantillons à plat (`SamplingMode::Flat`), et le bench prend environ 9 minutes.

| Benchmark | Temps | Débit |
|---|---|---|
| `stress_test/threads/1` | 15,5 s | 645 scénarios/s |
| `stress_test/threads/8` | 3,31 s | 3 020 scénarios/s |

Sur 8 cœurs, le gain est de ×4,7 et non ×8. Deux raisons probables : le processeur (Apple Silicon) mélange cœurs de performance et cœurs d'efficacité, et les tranches n'ont pas la même durée, alors que le thread le plus lent fixe la durée totale.

#### Flamegraph

[`docs/flamegraph-stress-test.svg`](docs/flamegraph-stress-test.svg) profile 3 000 scénarios sur 1 thread. On le régénère avec :

```bash
CARGO_PROFILE_RELEASE_DEBUG=true cargo run --release --example flamegraph
```

L'exemple utilise `pprof`, qui échantillonne le programme de l'intérieur. Contrairement à `cargo flamegraph` (dtrace ou perf), il ne demande pas de droits root.

| Fonction | Part du temps | Cause |
|---|---|---|
| `Protocol::accrue_interest` | ~36 % | une division et un modulo `u128` par position et par minute |
| `liquidate` → `Position::health_factor` | ~35 % | facteur de santé de chaque position à chaque minute, dont 21 % dans la division `u128` (`__udivti3`) |
| `buy_the_dip` → `Protocol::position` | ~25 % | recherche dans la `BTreeMap` par comparaison de `String`, pour chaque acheteur et à chaque minute |
| `Guardian::observe` | < 1 % | négligeable |

Pistes d'amélioration :

- **`is_liquidatable` sans division.** Comparer `valeur du collatéral × seuil < dette × 10 000` donne le même résultat avec une seule multiplication.
- **Ne plus chercher les acheteurs qui ont déjà emprunté**, en gardant un drapeau par acheteur. Sinon, stocker les positions dans un `Vec` indexé plutôt que dans une `BTreeMap<UserId, _>`.
- **Le gardien n'est pas à optimiser.** Le maximum sur 60 prix ne coûte presque rien, donc une file monotone n'apporterait rien de mesurable.
