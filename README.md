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
| `liquidation.rs` | règle de liquidation d'une position |
| `scenario.rs` | chargement des krachs historiques de `data/` |
| `replay.rs` | rejeu d'un krach, minute par minute |
| `bots.rs` | bots liquidateurs concurrents |
| `stress_test.rs` | krachs aléatoires rejoués sur plusieurs threads |
| `report.rs` | rapports du stress test et du rejeu |
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

## Krachs et liquidations

### Données

`data/` contient le prix de l'ETH minute par minute pendant trois krachs, au format de la section 5.2 de la spécification :

| Scénario | Période | Minutes | Vrai prix (départ → plus bas) | Baisse max |
|---|---|---|---|---|
| `black-thursday` | 12 mars 2020 | 1 440 | 195,02 → 101,37 USDC | −48 % |
| `luna` | 9 au 12 mai 2022 | 5 760 | 2 524,06 → 1 800,01 USDC | −29 % |
| `ftx` | 7 au 10 novembre 2022 | 5 760 | 1 569,26 → 1 076,24 USDC | −31 % |

Ces prix sont les cours de clôture des bougies d'une minute ETH/USDT de l'API publique de Binance. `scripts/fetch_scenarios.sh` les télécharge une seule fois avec `curl` et `jq`, et s'arrête si une minute manque. Le serveur ne dépend donc d'aucun service extérieur. `scenario.rs` n'accepte que les identifiants connus : un identifiant reçu par l'API ne peut pas viser un autre fichier du disque.

### Routes

| Méthode | Route | Effet |
|---|---|---|
| GET | `/scenarios` | scénarios disponibles, avec leur durée et leur vrai prix de départ |
| POST | `/replay/{id}` | lance le rejeu en arrière-plan et répond 202 tout de suite (409 si un rejeu est déjà en cours) |
| GET | `/liquidations` | liquidations du rejeu en cours ou du dernier : minute, bot, utilisateur, dette remboursée, collatéral saisi |
| GET | `/report` | rapport du dernier rejeu terminé, ou `null` |

### Démonstration

```bash
H='Content-Type: application/json'
curl -X POST localhost:3000/supply  -H "$H" -d '{"user":"alice","amount":10000000}'   # 100 000 USDC
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"bob","amount":1000000000}'   # 1 ETH
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"carol","amount":1000000000}'
curl -X POST localhost:3000/deposit -H "$H" -d '{"user":"dave","amount":1000000000}'
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"bob","amount":140000}'       # 1 400 USDC
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"carol","amount":120000}'     # 1 200 USDC
curl -X POST localhost:3000/borrow  -H "$H" -d '{"user":"dave","amount":100000}'      # 1 000 USDC
curl -X POST localhost:3000/replay/black-thursday                                     # environ 15 s
curl localhost:3000/liquidations                                                      # pendant ou après
curl localhost:3000/report                                                            # une fois terminé
```

Le rapport donne les prix rejoués, la baisse maximale, le nombre de liquidations, la dette remboursée, la dette irrécouvrable finale et le détail par bot. Avec trois emprunteurs à 1 400, 1 200 et 1 000 USDC, nos essais ont donné entre 8 et 11 liquidations. Leur nombre et leur répartition entre les bots changent d'un rejeu à l'autre, puisque les bots sont en concurrence. Seul le résultat est stable : aucune dette irrécouvrable.

### Règle de liquidation

`liquidation::liquidate` applique la section 4.5 à une position. C'est une fonction synchrone, utilisée à la fois par les bots et par le stress test.

- Elle refuse une position saine (`PositionHealthy`), un montant nul et un utilisateur inconnu.
- Elle rembourse au plus 50 % de la dette et donne au liquidateur `remboursé × 1,05 / prix` en ETH.
- **Quand le collatéral ne suffit plus, on suit Aave V3** (`_calculateAvailableCollateralToLiquidate` dans `LiquidationLogic.sol`) : le liquidateur prend tout le collatéral restant mais ne rembourse que sa valeur, prime déduite. Le reste de la dette reste visible comme dette irrécouvrable. S'il remboursait quand même 50 % de la dette, il paierait pour du collatéral inexistant, et la dette irrécouvrable disparaîtrait artificiellement.
- Une position sans collatéral n'est plus liquidable (`NothingToLiquidate`, l'équivalent de `CollateralCannotBeLiquidated` chez Aave), et sa dette ne produit plus d'intérêts.

Ce que l'on ne reprend pas d'Aave : le *close factor* à 100 % sous un facteur de santé de 0,95, la règle anti-poussière et le compteur `deficit`. Les deux mesures de la perte ne sont pas les mêmes. Aave n'inscrit un `deficit` que lorsqu'une liquidation laisse une position sans collatéral. Notre dette irrécouvrable (`Protocol::total_bad_debt`, section 4.6) compte aussi les positions sous l'eau qui ont encore de l'ETH : c'est ce que perdrait le pool si toutes les positions étaient soldées au prix courant. Elle est donc au moins égale au `deficit` d'Aave, et peut être positive quand celui-ci est nul. C'est le cas d'un rejeu sans liquidateur : aucune liquidation n'a lieu, les emprunteurs gardent tout leur ETH, et pourtant leur dette dépasse la valeur de ce collatéral.

### Rejeu et bots

À chaque minute, le rejeu met le prix à jour, ajoute une minute d'intérêts et informe le gardien, puis attend 10 ms. Il envoie la minute courante aux bots par un canal `tokio::sync::watch`. À la fin, il leur envoie `Finished`, attend leur arrêt et enregistre le rapport.

Trois bots tournent en parallèle du rejeu :

| Bot | Temps de réaction | Soit |
|---|---|---|
| `fast` | 20 ms | 2 minutes simulées |
| `average` | 50 ms | 5 minutes simulées |
| `slow` | 150 ms | 15 minutes simulées |

Chaque bot repère les positions liquidables, attend son temps de réaction, puis tente de les liquider en demandant toute la dette (la règle la ramène à 50 %). Pendant son attente, un bot plus rapide peut avoir pris la même position, ou le prix peut être remonté : la tentative est alors comptée comme ratée (`missed`). C'est le fonctionnement d'une vraie blockchain, où la transaction la plus rapide l'emporte.

**Mise à l'échelle des prix.** Les scénarios contiennent les vrais prix (195 USDC au départ du Black Thursday), alors que le pool démarre à 2 000 USDC. Rejouer ces prix tels quels ferait chuter l'ETH de 90 % dès la première minute. Le rejeu multiplie donc chaque prix par `prix courant / premier prix du scénario` : le krach garde exactement sa forme (−48 % au plus bas pour le Black Thursday), quel que soit le prix de départ. Les liquidations et le gardien ne dépendent que de ces variations, pas du niveau du prix.

### Async ou threads

- **Le rejeu et les bots sont des tâches tokio.** Ils passent presque tout leur temps à attendre (`tokio::time::sleep`). Une tâche en attente ne coûte presque rien, alors qu'un thread système garderait sa pile et sa place dans l'ordonnanceur pour ne rien faire, soit 4 threads bloqués par rejeu.
- **La route `/replay` répond tout de suite.** Elle lance le rejeu avec `tokio::spawn` et ne l'attend pas : le rapport est rangé dans l'état partagé à la fin.

### État partagé

Le rejeu, les bots et les routes partagent le même `AppState`. Son état `replay` (rejeu en cours, historique, dernier rapport) a son propre `Mutex`. Les verrous sont toujours pris dans le même ordre : protocole, gardien, puis rejeu. Deux tâches ne peuvent donc jamais s'attendre l'une l'autre. `replay::start` tient le protocole et le rejeu en même temps : deux requêtes simultanées ne peuvent pas lancer deux rejeux.

Aucun verrou n'est gardé pendant une attente. Le compilateur le garantit d'ailleurs : un `std::sync::MutexGuard` n'est pas `Send`, donc une tâche lancée avec `tokio::spawn` qui le garderait pendant un `.await` ne compilerait pas.

### Benchmark

`cargo bench --bench replay` rejoue le Black Thursday complet (1 440 minutes) sans délai entre les pas, sur un pool de 1 000 emprunteurs. Il mesure le travail fait à chaque minute : changer le prix, ajouter les intérêts de chaque position et informer le gardien. Les bots sont laissés de côté : ils attendent un temps fixe, donc avec eux le bench mesurerait l'horloge et non le code.

| Benchmark | Temps | Par minute simulée |
|---|---|---|
| `replay_black_thursday_1000_positions` | 7,7 ms | environ 5,4 µs |

**Ce que le bench a révélé.** La première mesure donnait 1,69 s, soit 1,2 ms par minute alors que le calcul en prend quelques microsecondes. La minuterie de tokio a une résolution de 1 ms : même `sleep(Duration::ZERO)` attend le tic suivant. Sans délai, le rejeu cède donc simplement la main aux autres tâches (`tokio::task::yield_now`), et le rejeu est 219 fois plus rapide. Le test d'intégration en profite aussi.

Pendant la démo, avec 10 ms par minute, le calcul représente moins de 0,1 % du temps : le rejeu passe son temps à attendre, ce qui confirme le choix d'une tâche tokio. Pour accélérer le calcul lui-même, les pistes sont celles du flamegraph du stress test : `accrue_interest` recalcule la dette totale à chaque minute, et les positions sont rangées dans une `BTreeMap` plutôt qu'un `Vec`.

## Gardien du risque et stress test

### Gardien du risque

À chaque nouveau prix, le gardien calcule la baisse entre le prix le plus haut des 60 dernières minutes et le prix actuel. Il en déduit un mode, qui s'applique uniquement aux nouveaux emprunts :

| Mode | Baisse sur 60 min | Nouveaux emprunts |
|---|---|---|
| `Normal` | < 5 % | LTV de 75 % |
| `Alert` | 5 % à 15 % | LTV abaissé à 50 % |
| `Emergency` | ≥ 15 % | suspendus (`BorrowingPaused`) |

Les positions existantes ne sont jamais modifiées. Le gardien agit seulement par deux champs de `Protocol` : `borrowing_paused` et `max_ltv_override`. `Protocol` ne dépend donc pas du gardien.

Ces deux champs sont sauvegardés avec le reste de `Protocol`. Après un `/load`, le gardien observe donc le prix rechargé, comme après `POST /price`. Sans cela, le fichier ramènerait les restrictions du moment de la sauvegarde, même avec un gardien désactivé depuis, et le gardien ne verrait pas l'écart entre le prix rechargé et ceux de la dernière heure.

### Stress test

Le stress test rejoue N krachs aléatoires sur le même portefeuille, une fois sans gardien et une fois avec.

- **Krach :** une chute en ligne droite de 10 à 70 % sur une durée de 30 minutes à 24 heures, avec un bruit de ±0,5 % par minute.
- **Portefeuille :** 80 emprunteurs entre 30 et 70 % de LTV, et 20 « acheteurs de baisse ». Ces derniers empruntent le maximum autorisé une fois que le prix a baissé de 2,5 %, 5 %, … ou 50 %.
- **Liquidation synchrone :** chaque minute, les liquidateurs liquident au plus 2 positions avec la règle de la section 4.5 (`liquidation.rs`), qui rembourse jusqu'à 50 % de la dette contre du collatéral avec une prime de 5 %. Une position qui n'a plus rien à saisir ne prend pas de place. Sans cette limite de capacité, la liquidation suit toujours un krach progressif et aucune dette irrécouvrable n'apparaît. C'est précisément ce qui a manqué lors du Black Thursday.
- **Survie :** le protocole survit si la dette irrécouvrable est nulle à la fin du krach.

Résultat avec 10 000 scénarios et la graine 42 :

| | Sans gardien | Avec gardien |
|---|---|---|
| Taux de survie | 91,2 % | 92,3 % |
| Dette irrécouvrable moyenne | 1 536,59 USDC | 939,41 USDC |

Le gardien réduit la dette irrécouvrable de 39 %, mais il améliore peu le taux de survie. Dans les krachs les plus violents, la perte vient des positions ouvertes avant le krach, que le gardien ne touche pas.

Aucune des 2 888 901 liquidations des 20 000 rejeux ne vise une position déjà insolvable, et aucune n'épuise son collatéral. La dette irrécouvrable vient des positions que les liquidateurs n'atteignent pas à temps : elles deviennent insolvables en attendant leur tour. Le plafond au collatéral restant de la section 4.5 ne change donc pas ces chiffres.

### Routes

| Méthode | Route | Effet |
|---|---|---|
| GET | `/risk` | mode, baisse observée (en bps) et état du gardien |
| POST | `/risk/toggle` | active ou désactive le gardien |
| POST | `/stress-test` | `{ "scenarios": 10000 }`, renvoie le rapport du stress test (100 000 scénarios au maximum) |

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

[`docs/flamegraph-stress-test.svg`](docs/flamegraph-stress-test.svg) profile le benchmark `stress_test/threads/1`. Il est produit avec les outils standard : `sample`, l'échantillonneur intégré à macOS, et [inferno](https://github.com/jonhoo/inferno), les scripts de flamegraph de Brendan Gregg portés en Rust.

Sur macOS, `cargo flamegraph` passe par `xctrace`, qui demande Xcode complet. `sample`, lui, n'a besoin ni de Xcode ni des droits root.

```bash
cargo install inferno rustfilt
BIN=$(CARGO_PROFILE_BENCH_DEBUG=true cargo bench --bench stress_test --no-run 2>&1 \
  | grep -o 'target/release/deps/stress_test-[0-9a-f]*')
$BIN --bench --profile-time 40 stress_test/threads/1 > /dev/null & sleep 5
sample $! 30 1 -file /tmp/stress.sample
rustfilt < /tmp/stress.sample | inferno-collapse-sample \
  | sed -E 's/stress_test-[0-9a-f]+`//g' \
  | inferno-flamegraph --title "CrashLend : stress test sur 1 thread (cargo bench)" \
  > docs/flamegraph-stress-test.svg
```

Sur Linux, `cargo flamegraph` passe par `perf` : `cargo flamegraph --bench stress_test -- --bench --profile-time 30 stress_test/threads/1`.

`sample` ne voit pas les fonctions inlinées : `liquidate`, `health_factor` et `buy_the_dip` apparaissent sous `simulate`.

| Fonction | Part du temps | Cause |
|---|---|---|
| `Protocol::accrue_interest` | 43,5 % | une division `u128` par position et par minute. Dans ces 43,5 %, `current_rate` pèse 13,5 % : `total_debt` reparcourt toutes les positions à chaque minute pour calculer l'utilisation. |
| `simulate` (code inliné) | 30,4 % | surtout le facteur de santé de chaque position, à chaque minute (`liquidate`) |
| `memcmp` | 20,3 % | comparaisons de `String` dans la `BTreeMap`, quand `buy_the_dip` cherche la position de chaque acheteur à chaque minute |
| `Guardian::observe` | 2,9 % | maximum sur les 60 derniers prix |

Pistes d'amélioration :

- **Ne plus chercher les acheteurs qui ont déjà emprunté**, en gardant un drapeau par acheteur. On peut aussi stocker les positions dans un `Vec` indexé plutôt que dans une `BTreeMap<UserId, _>`. Cela supprime les 20 % de `memcmp`.
- **Tenir la dette totale à jour dans `Protocol`** à chaque emprunt, remboursement et intérêt, au lieu de la recalculer. Cela supprime le second parcours des positions à chaque minute.
- **`is_liquidatable` sans division.** Comparer `valeur du collatéral × seuil < dette × 10 000` donne le même résultat avec une seule multiplication.
- **Le gardien est une priorité faible.** Une file monotone rendrait le maximum en O(1), mais elle ne peut pas gagner plus que ses 3 %.
