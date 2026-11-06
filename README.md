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
