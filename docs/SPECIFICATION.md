# CrashLend - Spécification

| | |
|---|---|
| Projet | Rust Team Project, ESILV A4 S7 |
| Domaine | DeFi |
| Équipe | Eli Benbaruk, Armand, Noé |
| Version | 1.0 (4 novembre 2026) |
| Rendu | 23 novembre 2026 |

---

## 1. Présentation

### 1.1 Le contexte

Aave est l'un des principaux protocoles de prêt de la finance décentralisée. Il fonctionne comme une banque sans banquier :

- des **prêteurs** déposent des stablecoins (USDC) dans un pool commun et reçoivent des intérêts ;
- des **emprunteurs** déposent une garantie, appelée **collatéral** (par exemple de l'ETH), puis empruntent des USDC dans le pool ;
- le collatéral vaut toujours plus que la dette. Si le prix de l'ETH baisse et que la dette devient trop risquée, n'importe qui peut **liquider** la position : il rembourse une partie de la dette et récupère du collatéral avec une prime.

La liquidation protège les prêteurs : tant qu'elle intervient à temps, le pool ne perd pas d'argent. Si le prix chute trop vite pour que les liquidations suivent, le collatéral ne couvre plus la dette. Le protocole subit alors une perte, appelée **dette irrécouvrable** (*bad debt*). C'est ce qui est arrivé à plusieurs protocoles lors du krach du 12 mars 2020 (*Black Thursday*).

### 1.2 Ce que nous construisons

CrashLend est une version simplifiée d'Aave, écrite en Rust et exécutée en simulation (sans blockchain). Il est accessible via une API HTTP.

Il ajoute trois éléments qui n'existent pas dans un protocole de prêt classique :

1. **Le rejeu de krachs historiques.** On rejoue minute par minute le vrai prix de l'ETH lors de trois krachs (Black Thursday 2020, effondrement de LUNA en 2022, faillite de FTX en 2022). Des bots liquidateurs, qui s'exécutent en parallèle, surveillent les positions et se concurrencent pour les liquider.
2. **Un gardien du risque.** Il observe la vitesse de baisse du prix et durcit automatiquement les conditions d'emprunt quand le marché chute.
3. **Un stress test.** On simule 10 000 krachs aléatoires, avec et sans gardien, pour mesurer la probabilité que le protocole termine sans dette irrécouvrable.

### 1.3 La question à laquelle le projet répond

> Notre protocole de prêt aurait-il survécu aux pires krachs de l'histoire crypto, et un mécanisme de protection automatique améliore-t-il ses chances ?

La démonstration finale répond à cette question avec des chiffres :
- le nombre de liquidations pendant un krach réel ;
- la dette irrécouvrable restante ;
- le taux de survie mesuré sur 10 000 scénarios.

---

## 2. Périmètre

### 2.1 Inclus

- Un pool unique avec deux actifs : ETH (collatéral) et USDC (actif emprunté).
- Les opérations de prêt : fournir des USDC, déposer et retirer de l'ETH, emprunter et rembourser des USDC.
- Le calcul du facteur de santé, un taux d'intérêt variable et l'accumulation des intérêts.
- La liquidation, la dette irrécouvrable, le rejeu de krachs, les bots liquidateurs, le gardien du risque et le stress test.
- Une API HTTP en JSON, et la sauvegarde et le chargement de l'état dans un fichier JSON.

### 2.2 Exclus

- Pas de blockchain réelle ni de smart contract. Tout est simulé en mémoire.
- Pas d'autres actifs, pas de tokens de dépôt (aTokens), pas de gouvernance, pas de flash loans.
- Les prêteurs ne perçoivent pas d'intérêts : les intérêts augmentent la dette des emprunteurs, mais leur redistribution n'est pas modélisée.
- Pas d'authentification sur l'API.
- Pas d'interface graphique : la démonstration passe par l'API et les journaux du serveur.

---

## 3. Glossaire

| Terme | Définition |
|---|---|
| Collatéral | ETH déposé par un emprunteur en garantie de sa dette. |
| Dette | Montant d'USDC emprunté, intérêts inclus. |
| LTV (*Loan To Value*) | Part maximale de la valeur du collatéral qu'on peut emprunter. |
| Seuil de liquidation | Part de la valeur du collatéral au-delà de laquelle la dette devient liquidable. |
| Facteur de santé | Indicateur de risque d'une position. En dessous de 1, la position est liquidable. |
| Liquidation | Remboursement partiel de la dette par un tiers, qui reçoit du collatéral avec une prime. |
| Prime de liquidation | Bonus en collatéral versé au liquidateur. |
| Utilisation | Part des USDC du pool actuellement empruntée. |
| Dette irrécouvrable | Part de la dette qui n'est plus couverte par le collatéral. |
| Scénario | Série de prix de l'ETH, minute par minute, sur une période donnée. |
| Pas de temps | Une minute simulée. |

---

## 4. Règles métier

### 4.1 Unités

L'argent n'est jamais représenté par des nombres flottants, qui introduisent des erreurs d'arrondi. Toutes les valeurs sont des entiers.

| Grandeur | Type Rust | Unité |
|---|---|---|
| Quantité d'ETH | `EthAmount(u64)` | gwei (10⁻⁹ ETH) |
| Montant en USDC | `UsdcAmount(u64)` | centime (10⁻² USDC) |
| Prix de l'ETH | `Price(u64)` | centimes d'USDC pour 1 ETH |
| Pourcentage | `Bps(u64)` | point de base (1 % = 100 bps) |

Les multiplications intermédiaires sont faites en `u128` pour éviter tout dépassement de capacité. Le résultat final est ensuite reconverti en `u64`.

### 4.2 Paramètres du protocole

| Paramètre | Valeur |
|---|---|
| LTV maximal | 75 % |
| Seuil de liquidation | 80 % |
| Part maximale de la dette liquidable en une fois | 50 % |
| Prime de liquidation | 5 % |
| Taux de base | 2 % par an |
| Utilisation optimale | 80 % |
| Pente sous l'utilisation optimale | 4 % |
| Pente au-dessus de l'utilisation optimale | 60 % |

Les paramètres sont regroupés dans une structure `Params` pour que le stress test puisse les faire varier.

### 4.3 Formules

```
valeur du collatéral = quantité d'ETH × prix

emprunt maximal      = valeur du collatéral × LTV

facteur de santé     = valeur du collatéral × seuil de liquidation / dette
                       (infini si la dette est nulle)

utilisation          = USDC empruntés / USDC fournis

taux annuel          = si utilisation ≤ 80 % :
                           2 % + (utilisation / 80 %) × 4 %
                       sinon :
                           6 % + ((utilisation − 80 %) / 20 %) × 60 %
```

Le taux suit le même modèle « à deux pentes » qu'Aave :
- tant que le pool a des liquidités, emprunter reste bon marché ;
- au-delà de 80 % d'utilisation, le taux monte fortement pour inciter au remboursement.

Les intérêts s'appliquent à chaque pas de temps simulé :

```
dette += dette × taux annuel / 525 600      (525 600 = minutes dans une année)
```

### 4.4 Opérations

| Opération | Effet | Refusée si |
|---|---|---|
| Fournir des USDC | ajoute des liquidités au pool | montant nul |
| Déposer de l'ETH | augmente le collatéral de l'utilisateur | montant nul |
| Emprunter des USDC | augmente la dette et diminue la liquidité | montant nul, dette résultante > emprunt maximal, liquidité insuffisante, emprunts suspendus par le gardien |
| Rembourser | diminue la dette | montant nul, aucune dette (un montant supérieur à la dette est ramené à la dette) |
| Retirer de l'ETH | diminue le collatéral | montant nul, collatéral insuffisant, dette > emprunt maximal après le retrait |

### 4.5 Liquidation

Une position est liquidable si son facteur de santé est strictement inférieur à 1.

Le liquidateur choisit un montant de dette à rembourser, plafonné à 50 % de la dette de la position. En échange, il reçoit :

```
collatéral saisi = montant remboursé × (1 + prime) / prix
```

Si le collatéral restant est inférieur à ce montant, le liquidateur reçoit tout le collatéral restant et ne rembourse que sa valeur, prime déduite (`valeur du collatéral / (1 + prime)`). Le reste de la dette devient de la dette irrécouvrable. Une position sans collatéral ne peut plus être liquidée (`NothingToLiquidate`).

Une liquidation sur une position saine est refusée avec l'erreur `PositionHealthy`.

### 4.6 Dette irrécouvrable

À tout instant, pour chaque position :

```
dette irrécouvrable = max(0, dette − valeur du collatéral)
```

La dette irrécouvrable du protocole est la somme sur toutes les positions. **Le protocole survit à un scénario si sa dette irrécouvrable est nulle à la fin du scénario.**

### 4.7 Gardien du risque

À chaque nouveau prix, le gardien calcule la baisse du prix sur les 60 dernières minutes :

```
baisse = (prix le plus haut sur 60 min − prix actuel) / prix le plus haut sur 60 min
```

Il en déduit un mode :

| Mode | Condition | Effet sur les nouveaux emprunts |
|---|---|---|
| `Normal` | baisse < 5 % | LTV maximal de 75 % |
| `Alert` | 5 % ≤ baisse < 15 % | LTV maximal abaissé à 50 % |
| `Emergency` | baisse ≥ 15 % | emprunts suspendus |

Le gardien ne touche jamais aux positions existantes. Il agit seulement sur les nouveaux emprunts. Il peut être désactivé, ce qui permet la comparaison faite par le stress test.

---

## 5. Scénarios de krach

### 5.1 Scénarios historiques

| Identifiant | Période | Événement |
|---|---|---|
| `black-thursday` | 12 mars 2020 | baisse de l'ETH d'environ 43 % en une journée |
| `luna` | 9 au 12 mai 2022 | effondrement de l'écosystème Terra |
| `ftx` | 7 au 10 novembre 2022 | faillite de l'exchange FTX |

Les prix sont téléchargés une seule fois depuis l'API publique et gratuite de Binance (bougies d'une minute, paire ETH/USDT), puis enregistrés dans `data/`. Le projet ne dépend d'aucun service extérieur à l'exécution.

### 5.2 Format

```json
{
  "id": "black-thursday",
  "name": "Black Thursday",
  "start": "2020-03-12T00:00:00Z",
  "prices": [19412, 19387, 19401]
}
```

`prices` contient un prix par minute, exprimé en centimes d'USDC.

### 5.3 Rejeu

- Le rejeu avance d'un pas de temps toutes les 10 ms par défaut, ce qui fait défiler une journée en environ 15 secondes.
- À chaque pas, le rejeu met à jour le prix, accumule les intérêts et informe le gardien.
- Pendant ce temps, trois bots liquidateurs tournent en parallèle. Chacun vérifie régulièrement les positions et liquide celles dont le facteur de santé est inférieur à 1. Les bots ont des délais de réaction différents. Quand deux bots visent la même position, le premier l'emporte et le second reçoit `PositionHealthy` ou une dette déjà réduite.
- À la fin du rejeu, un rapport est produit.

---

## 6. Stress test

### 6.1 Méthode

1. On part d'un portefeuille initial de positions, identique pour toutes les simulations.
2. On génère N scénarios aléatoires (10 000 par défaut). Chacun est défini par une baisse totale (entre 10 et 70 %) et une durée (entre 30 minutes et 24 heures), avec une légère variation aléatoire minute par minute.
3. Pour chaque scénario, on rejoue le krach sur une copie du protocole, avec une liquidation simplifiée et synchrone à chaque pas. On note si le protocole survit.
4. On répète l'expérience avec le gardien activé puis désactivé.

### 6.2 Résultat

```json
{
  "scenarios": 10000,
  "survival_rate_without_guardian": 0.71,
  "survival_rate_with_guardian": 0.94,
  "average_bad_debt_without_guardian": 152300,
  "average_bad_debt_with_guardian": 18400,
  "threads": 8,
  "duration_ms": 2140
}
```

*(les valeurs ci-dessus sont des exemples)*

---

## 7. Architecture

### 7.1 Modules

```
src/
  main.rs           démarrage du serveur
  lib.rs            déclaration des modules
  types.rs          EthAmount, UsdcAmount, Price, Bps, UserId, Position, Params
  error.rs          enum LendingError
  protocol.rs       état du protocole et opérations de prêt
  interest.rs       modèle de taux et accumulation des intérêts
  persistence.rs    sauvegarde et chargement de l'état en JSON
  scenario.rs       chargement des scénarios historiques
  replay.rs         rejeu d'un scénario
  liquidation.rs    règle de liquidation
  bots.rs           bots liquidateurs
  guardian.rs       gardien du risque
  stress_test.rs    génération et exécution des scénarios aléatoires
  report.rs         rapport de fin de rejeu et de stress test
  api/
    mod.rs          routeur et état partagé
    lending.rs      routes de prêt
    crash.rs        routes de rejeu et de liquidation
    risk.rs         routes du gardien et du stress test
data/               scénarios historiques (JSON)
tests/              tests d'intégration
benches/            benchmarks criterion
```

### 7.2 État partagé

L'état du protocole (`Protocol`) est détenu par un `Arc<Mutex<Protocol>>`. Ce pointeur est partagé :
- avec axum, via `State` ;
- avec la tâche de rejeu ;
- avec les bots liquidateurs.

Chaque accès prend le verrou, effectue une opération courte et le relâche. Le verrou n'est jamais conservé pendant une attente.

### 7.3 Tâches asynchrones et threads

| Traitement | Exécution | Justification |
|---|---|---|
| Serveur HTTP | tâches tokio | attente réseau : l'async gère beaucoup de connexions avec peu de ressources |
| Rejeu d'un scénario | tâche tokio | la tâche passe l'essentiel de son temps à attendre le pas suivant (`tokio::time::sleep`) |
| Bots liquidateurs | tâches tokio | même chose : ils attendent, vérifient brièvement, puis attendent de nouveau |
| Stress test | threads système (`std::thread::scope`) | calcul pur, sans attente : on répartit les scénarios sur les cœurs du processeur. En tâche async, ce calcul bloquerait le serveur |

### 7.4 Dépendances

| Crate | Usage |
|---|---|
| `tokio` | runtime asynchrone, tâches, minuteries |
| `axum` | API HTTP |
| `serde`, `serde_json` | sérialisation des types, scénarios, sauvegarde, API |
| `rand` | génération des scénarios aléatoires du stress test |
| `criterion` | benchmarks (dépendance de développement) |

Toute nouvelle dépendance doit être justifiée dans la PR qui l'ajoute.

---

## 8. API HTTP

Toutes les requêtes et réponses sont en JSON. En cas d'erreur, l'API répond avec un code HTTP adapté et le corps `{ "error": "<message>" }`.

| Méthode | Route | Corps de la requête | Réponse |
|---|---|---|---|
| POST | `/supply` | `{ "user": "alice", "amount": 100000 }` | état du pool |
| POST | `/deposit` | `{ "user": "bob", "amount": 2000000000 }` | position de l'utilisateur |
| POST | `/borrow` | `{ "user": "bob", "amount": 150000 }` | position de l'utilisateur |
| POST | `/repay` | `{ "user": "bob", "amount": 50000 }` | position de l'utilisateur |
| POST | `/withdraw` | `{ "user": "bob", "amount": 500000000 }` | position de l'utilisateur |
| GET | `/positions` | | liste des positions avec facteur de santé |
| GET | `/pool` | | liquidité, utilisation, taux courant |
| POST | `/save` | | sauvegarde l'état dans `state.json` |
| POST | `/load` | | recharge l'état depuis `state.json` |
| GET | `/price` | | prix courant |
| GET | `/scenarios` | | liste des scénarios disponibles |
| POST | `/replay/{id}` | | lance le rejeu en arrière-plan |
| GET | `/liquidations` | | historique des liquidations |
| GET | `/risk` | | mode courant du gardien et baisse observée |
| POST | `/risk/toggle` | | active ou désactive le gardien |
| POST | `/stress-test` | `{ "scenarios": 10000 }` | résultat du stress test |
| GET | `/report` | | rapport du dernier rejeu |

---

## 9. Erreurs

Toutes les fonctions de la bibliothèque qui peuvent échouer renvoient `Result<T, LendingError>`. Le code de la bibliothèque n'utilise aucun `unwrap()`.

| Variante | Cas |
|---|---|
| `ZeroAmount` | montant nul |
| `UnknownUser` | utilisateur inconnu |
| `InsufficientCollateral` | emprunt ou retrait qui dépasserait l'emprunt maximal |
| `InsufficientLiquidity` | le pool ne contient pas assez d'USDC |
| `NothingToRepay` | remboursement sans dette |
| `PositionHealthy` | liquidation d'une position saine |
| `NothingToLiquidate` | liquidation d'une position sans collatéral |
| `BorrowingPaused` | emprunt refusé par le gardien |
| `ScenarioNotFound` | identifiant de scénario inconnu |
| `ReplayAlreadyRunning` | un rejeu est déjà en cours |
| `TooManyScenarios` | stress test demandé avec plus de 100 000 scénarios |
| `Storage(String)` | erreur de lecture ou d'écriture de fichier JSON |

---

## 10. Qualité

### 10.1 Tests

- **Tests unitaires** dans chaque module, sur les formules et sur chaque cas d'erreur.
- **Tests d'intégration** dans `tests/`, un par membre :
  - `tests/lending.rs` : cycle complet de dépôt, emprunt, intérêts, remboursement et retrait ;
  - `tests/crash_replay.rs` : rejeu d'un scénario court avec liquidations ;
  - `tests/stress.rs` : stress test sur un petit nombre de scénarios dont le résultat est connu.

### 10.2 Benchmarks

| Benchmark | Mesure |
|---|---|
| `health_factor` | calcul du facteur de santé sur 10 000 positions |
| `replay` | rejeu complet de `black-thursday` sans délai entre les pas |
| `stress_test` | 10 000 scénarios avec 1 thread puis N threads |

Un flamegraph est réalisé sur le stress test.

### 10.3 Outillage

`cargo fmt`, `cargo clippy -- -D warnings` et `cargo test` doivent passer avant chaque demande de fusion.

---

## 11. Répartition

Chaque membre est responsable d'une partie métier complète : le code, les tests, le benchmark, les routes HTTP associées et la section correspondante du README.

### 11.1 Eli - Protocole de prêt

- Types du domaine et erreurs (`types.rs`, `error.rs`).
- Opérations de prêt et facteur de santé (`protocol.rs`).
- Modèle de taux et intérêts (`interest.rs`).
- Sauvegarde et chargement JSON (`persistence.rs`).
- Squelette du serveur et état partagé (`main.rs`, `api/mod.rs`), routes de prêt (`api/lending.rs`).
- Test d'intégration `tests/lending.rs`, benchmark `health_factor`.

### 11.2 Armand - Krachs et liquidations

- Récupération et préparation des données historiques (`data/`).
- Chargement des scénarios (`scenario.rs`).
- Rejeu asynchrone (`replay.rs`).
- Règle de liquidation (`liquidation.rs`) et bots liquidateurs (`bots.rs`).
- Routes de rejeu (`api/crash.rs`).
- Test d'intégration `tests/crash_replay.rs`, benchmark `replay`.

### 11.3 Noé - Gardien du risque et stress test

- Gardien du risque (`guardian.rs`).
- Stress test multithreadé (`stress_test.rs`).
- Rapports (`report.rs`).
- Routes du gardien et du stress test (`api/risk.rs`).
- Test d'intégration `tests/stress.rs`, benchmark `stress_test` et flamegraph.

### 11.4 Travail commun

- Le premier créneau du jour 1 sert à fixer ensemble les types et les signatures des fonctions publiques, pour que chacun puisse avancer en parallèle.
- Chaque membre relit au moins une demande de fusion de chacun des deux autres.
- Chaque membre doit pouvoir expliquer l'ensemble du projet lors de la revue de code individuelle, pas seulement sa partie. Une répétition commune est prévue avant la revue.

---

## 12. Planning

| | 13:30-15:00 | 15:15-16:45 | 17:00-18:30 | 18:45-20:15 |
|---|---|---|---|---|
| Jour 1 | Types et signatures (ensemble), squelette du projet (Eli) | Eli : opérations de prêt. Armand : données et scénarios. Noé : gardien | Point d'étape à 17h, puis fin du cœur avec `cargo test` vert | Eli : intérêts et routes. Armand : rejeu et liquidation. Noé : début du stress test |
| Jour 2 | Armand : bots. Noé : stress test multithreadé. Eli : sauvegarde. Premiers benchmarks | Tests d'intégration, routes restantes | Point performance à 17h, puis clippy et README | Présentation |
| En ligne | Envoi du dépôt avant le 23 novembre, puis revue de code | | | |

**Objectif de fin de jour 1** : via l'API, on peut fournir des liquidités, déposer, emprunter, faire baisser le prix et liquider une position.

**Démonstration finale** :
1. Mise en place de plusieurs emprunteurs.
2. Rejeu du Black Thursday avec les bots, puis le rapport de liquidations et de dette irrécouvrable.
3. Stress test avec et sans gardien.

---

## 13. Conventions

### 13.1 Code

- Le code doit rester simple et lisible : des structures, des énumérations, du `match` et des boucles. Pas de généricité inutile, pas de macros personnalisées, pas de `unsafe`.
- Chaque fichier commence par un commentaire `//!` qui décrit son rôle. Chaque élément public est documenté par un commentaire `///`.
- Les commentaires expliquent le pourquoi, pas le quoi.
- Le code, les noms et les commentaires sont en anglais. La documentation du projet est en français.

### 13.2 Git

- Aucun commit direct sur `main`. Chaque tâche a sa branche : `feat/...`, `fix/...`, `test/...`, `docs/...`.
- Une demande de fusion correspond à un seul changement. Elle a une description courte (objet, motivation, méthode de test) et elle est relue par un autre membre avant la fusion.
- Les commits sont petits et leurs messages, en français, suivent la forme `type: description` (par exemple `feat: calcul du facteur de santé`).
