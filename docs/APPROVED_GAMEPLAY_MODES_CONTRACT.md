# Approved Gameplay Modes Contract

This document is the normative specification for Roady gameplay modes, the v2
protocol, the v2 canonical encoding, the strict JSON API, migration 0005,
lifecycle, security, privacy, tests, and deployment gates. Every statement
uses MUST, MUST NOT, or an exact definition sourced from code or from the
approved requirements. This contract freezes the approved requirements and
overrides all prior plan prose.

## 1. Product and mode matrix

### 1.1 Axes

Competition axis: `ranked` (submits to leaderboard, Worker-issued signed seed)
and `casual` (MUST NOT submit, local deterministic fallback seed). Conduct
axis: `cluck_hunt` (category `rotation.v1.cluck_hunt`) and `right_of_way`
(category `rotation.v1.right_of_way`).

### 1.2 Product matrix

| Mode | Competition | Conduct | Category key | Submits | Rotation |
|---|---|---|---|---|---|
| Ranked CluckHunt | ranked | cluck_hunt | rotation.v1.cluck_hunt | YES | forced |
| Ranked RightOfWay | ranked | right_of_way | rotation.v1.right_of_way | YES | forced |
| Casual CluckHunt | casual | cluck_hunt | none | NO | manual |
| Casual RightOfWay | casual | right_of_way | none | NO | manual |

### 1.3 Defaults and gating (authoritative, no internal contradiction)

- The default competition is `ranked`; the default conduct is `cluck_hunt`;
  the default mode is **Ranked CluckHunt**. A fresh process MUST initialize to
  Ranked CluckHunt when capabilities permit. This default is constant across
  all deployment stages; no stage changes the contractual default mode.
- A Ranked run MUST use a Worker-issued signed seed, MUST apply forced effect
  rotation, MUST NOT permit manual condition selection, and MUST NOT let the
  client choose the active rotation effect.
- A Casual run MUST permit manual classic-condition selection from the five v1
  conditions (IDs 0 through 4), MUST NOT submit to any leaderboard, MUST NOT
  call `/v2/scores`, `/v1/scores`, `/v2/session`, or `/v1/session`, MUST NOT
  create a v2 session, and MUST mark every local record unranked.
- The client MUST query `GET /v2/capabilities` before enabling Ranked. Ranked
  menu entry MUST be gated on `capabilities.ranked.enabled === true`. On
  capabilities fetch failure OR `enabled === false`, the client MUST disable
  Ranked for that session and present Casual modes only. This is a runtime
  capability fallback, not a change to the contractual default: when
  capabilities later permit, the default returns to Ranked CluckHunt.
- The deployment stage gate in section 16 governs only the **ranked
  enforcement transition** (`pending` to `live` after evidence replay). It
  MUST NOT change the default mode. Before enforcement parity, Ranked runs
  still occur and are stored as `pending`/shadow; they are never silently
  relabeled Casual.

### 1.4 Local namespaces

- Local best-score persistence MUST use separate namespaces. The v1 legacy key `car_game_best` MUST remain unchanged and MUST store only v1 classic records (`v1:global:k0:k1:k2:k3:k4`). Ranked records use `roady.v2.best.ranked.rotation.v1.cluck_hunt` and `roady.v2.best.ranked.rotation.v1.right_of_way`. Casual records use `roady.v2.best.casual.{conduct}.{condition_id}` for condition IDs 0 through 4. No namespace can read or write another namespace. Medals are computed within each namespace.

## 2. Compatibility and preservation

### 2.1 v1 byte and artifact preservation

- Every v1 byte, artifact, route, table, record, and condition ID 0 through 4
  MUST be preserved unchanged. `rules/roady-rules.v1.json`,
  `rules/roady-rules.v1.schema.json`, and `crates/roady-score-rules`
  (`RULES_VERSION=1`, `RULES_VERSION_ID="roady-rules.v1"`) MUST remain
  byte-identical. Condition IDs 0 through 4, their `storage_index`,
  `chicken_score_bonus`, `combo_bonus_multiplier`, and `reachable_events` MUST
  remain frozen.

### 2.2 v1 condition registry (frozen, sourced from `roady-rules.v1.json`)

| ID | name | idx | chicken_bonus | combo_mult | reachable_events (schedule order) |
|---:|---|---:|---:|---:|---|
| 0 | standard | 0 | 0 | 1 | traffic_surge, critter_burst |
| 1 | rush_hour | 1 | 0 | 1 | chicken_burst, combo_frenzy |
| 2 | chicken_frenzy | 2 | 1 | 1 | critter_burst, traffic_surge |
| 3 | stampede | 3 | 0 | 1 | combo_frenzy, chicken_burst |
| 4 | glass_cannon | 4 | 0 | 2 | critter_burst, traffic_surge |

### 2.3 v1 routes, tables, encoding (unchanged)

- Routes `/v1/session` POST, `/v1/scores` POST, `/v1/leaderboard` GET,
  `/v1/leaderboard.svg` GET, `/api/leaderboard.svg` GET, `/v1/me/rank` GET,
  `/v1/admin/scores/restore` POST, `/v1/admin/scores/:id/hide` POST,
  `DELETE /v1/admin/scores/:id`, `/healthz` GET MUST remain unchanged in JSON
  shapes, status codes, error codes, CORS, cache directives, rate limits, and
  canonical LF HMAC encodings. v1 canonical encodings `roady.v1.score` and
  `roady.v1.session` MUST remain ASCII-LF-joined with no trailing LF and
  base-10 integers; existing v1 golden fixtures remain authoritative.
- Tables `sessions`, `scores`, `moderation_log`, `admin_restorations` and
  migrations 0001 through 0004 MUST remain unchanged; v2 MUST add tables only
  via migration 0005. v1 and v2 leaderboards MUST remain separate.
  Deterministic ordering within each MUST remain
  `terminal_total DESC, submitted_at ASC, id ASC`.
- A Casual run MUST NOT produce a new v1 leaderboard submission. The v1 submission flow MUST remain available only as a compatibility API for already-deployed v1 clients with valid v1 sessions. The new v2 client MUST NOT initiate new v1 submissions.

## 3. Version tuple and category registry

- `protocol_version` MUST be 2; `rules_version` MUST be 2; `rules_id` MUST be
  `roady-rules.v2`; `policy_version` MUST be 1; `mode` MUST be `rotation`. A
  v2 request with any other protocol, rules, policy, mode, or category value
  MUST be hard-rejected with `422 unknown_version_tuple` before body parsing.
- `rotation.v1.cluck_hunt` and `rotation.v1.right_of_way` are the only v2
  categories. `score_categories(category_key, rules_version, display_name,
  active)` MUST contain exactly these two rows. Every category foreign key in
  `scores_v2`, `sessions_v2`, and `score_evidence` MUST reference
  `score_categories(category_key)`.

## 4. Gameplay rules (v2 rotation)

### 4.1 Rotation clock and phase cadence (frozen)

All boundaries use integer active-play milliseconds from Roady's active-play
clock (`Difficulty.elapsed` in seconds while `InputFrozen` is false; countdown,
pause, and input-frozen time MUST NOT advance it).

| Phase | Duration (ms) |
|---|---:|
| Initial grace | 8000 |
| Telegraph | 3000 |
| Active rotating effect | 18000 |
| Cooldown | 7000 |
| Full cadence | 28000 |

For window index `i` starting at 0: telegraph `[8000 + i*28000, 8000 + i*28000 + 3000)`;
active `[telegraph_end, telegraph_end + 18000)`; cooldown end `active_end + 7000`.
Concrete: i=0 telegraph `[8000,11000)` active `[11000,29000)` cooldown end 36000;
i=1 telegraph `[36000,39000)` active `[39000,57000)` cooldown end 64000;
i=2 telegraph `[64000,67000)` active `[67000,85000)` cooldown end 92000;
i=3 telegraph `[92000,95000)` active `[95000,113000)` cooldown end 120000.
The schedule commitment (section 5.3) MUST commit to all 16 windows.
Adding remaining time MUST expose subsequent occurrences and MUST NOT stretch,
shift, restart, or skip existing boundaries.

### 4.2 Rotation pool and anti-repeat

The serialized pool is `[RushHour(1), Stampede(3), GlassCannon(4)]`. Standard(0) is the neutral baseline and ChickenFrenzy(2) is a pickup. Each window consumes one draw from the `rotation` stream. If it repeats the prior effect, consume exactly one additional draw from the same stream. If that draw also repeats, select the prior effect's cyclic successor. No separate anti-repeat stream exists.

### 4.3 Exact composition and caps

| Axis | Exact transition |
|---|---|
| Traffic target | `min(min(1+floor(level/2),8) * rush_count * surge_count,8)`, where each active count multiplier is 2, otherwise 1. |
| Traffic speed | `min((5.0+level*0.7)*(0.85+speed_roll*0.30)*rush_speed*surge_speed,11.5)`, `speed_roll in [0,1]`, RushHour=1.35, TrafficSurge=1.25, otherwise 1. |
| Chicken target | `min(14 * chicken_burst * frenzy,40)`, where each active multiplier is 2, otherwise 1. |
| Critter target | `min(5 * stampede + critter_burst_extra,16)`, Stampede=2 otherwise1, CritterBurst extra=5 otherwise0. |
| Obstacle damage | Only closing impact speed strictly greater than 5.0 damages; `impact_speed*4.0*glass`, glass=2.0 otherwise1.0, one admitted impact per 500ms, health clamped `[0,100]`. |
| Critter damage | `25.0*glass`, one admitted hit per 400ms, health clamped `[0,100]`. |
| Chicken direct award | CluckHunt only: saturating `1 + ChickenBurst(1 or0) + Frenzy(1 or0)`. |
| Combo bonus | CluckHunt only: saturating `(combo_multiplier-1)*GlassCannon(2 or1)*ComboFrenzy(2 or1)`. Combo tiers are count0..4=>1,5..9=>2,10..14=>3,15..19=>4,20+=>5. |

Exactly one rotating effect, one scheduled event, and one Frenzy activation can be active. SpeedBoost and CoinMagnet can coexist. Protocol arithmetic uses checked integers and rejects overflow. Live CluckHunt bucket transitions retain the existing saturating operations. RightOfWay uses checked signed i64 transitions.

### 4.4 Scheduled events

E0 remains `[15000,23000)` and E1 `[40000,48000)`. Each consumes one draw from `scheduled_events`. Remove the flavor matching the rotating effect active at the event start (`RushHour=>TrafficSurge`, `Stampede=>CritterBurst`, `GlassCannon=>ComboFrenzy`) from `[TrafficSurge(0),ChickenBurst(1),ComboFrenzy(2),CritterBurst(3)]`, then select `eligible[draw%3]`. E0 and E1 can be equal. seed01 resolves E0=ComboFrenzy and E1=CritterBurst.

### 4.5 Population reconciliation

Spawn and retirement budgets advance only in active-play time: +12 entities/second below target and +18/second above target, retaining fractional remainder. RNG advances only on actual spawn attempts. Start keeps live entities and immutable traffic speed rolls. End retires effect extras before baseline surplus, ordered outside-camera, behind-car, farthest, then ascending entity bits.

### 4.6 Frozen CluckHunt score and time transitions

Chicken base=1; objective=10; coin=1; MegaCoin=5; critter score penalty=2; combo window=2500ms; coin clock=`min(clamp(current,0,90000)+1500,90000)`; Time pickup clock=`min(current+5000,99000)`; health pickup=`min(current+35,100)`. CluckHunt terminal total is checked `chickens+coins`; overflow rejects. Only non-protocol display uses a saturating terminal total.

## 5. Deterministic schedule and PRNG vectors

### 5.1 Master seed and streams

The master seed is exactly 32 bytes and serializes as 64 lowercase hexadecimal characters. SplitMix64 uses wrapping addition `0x9E3779B97F4A7C15`, multipliers `0xBF58476D1CE4E5B9` and `0x94D049BB133111EB`, then xor-shift31. Stream derivation runs FNV-1a-64 from `0xcbf29ce484222325` over master-seed bytes, the UTF-8 domain length as u32 little-endian, then domain bytes using prime `0x100000001b3`, and applies the SplitMix64 finalizer without increment.

Domains are `roady.rotation.v2.rotation`, `roady.rotation.v2.scheduled_events`, `roady.rotation.v2.frenzy.interval`, `roady.rotation.v2.frenzy.roll`, `roady.rotation.v2.frenzy.kind`, `roady.rotation.v2.frenzy.position`, and `roady.rotation.v2.frenzy.relocation`. Range mapping is `next_u64()%n`; rejection sampling is not used.

seed01 is bytes `01 02 ... 20`. Its `(FNV,state)` anchors are rotation `(5995f419c4dc0c2e,d57b9ede6427d32c)`, scheduled-events `(6d3eca77d6179b85,f28bbdc6ed34a573)`, interval `(5db24a622cfa7394,f0641024cf58a791)`, roll `(da8b1128a64d8b06,bd2f15367b6e6919)`, kind `(e6eba56363494e8b,cebdc4c26cdade04)`, position `(4b4ad0b4be929e2e,2f19e691358c35b2)`, relocation `(1678c319a195569d,a180f2b460780b20)`.

### 5.2 seed01 anchors

The first rotation draws are `89ca1998c369bfad,0aeaa2d3a4bd2ca7,f7eb05ac42dd4bab`; W0=Stampede, W1=GlassCannon, W2=Stampede. Scheduled-event draws are `208d629eaedd81ba,e8b774f1db6390e8`; E0=ComboFrenzy, E1=CritterBurst. Sixteen-window effects are `[Stampede,GlassCannon,Stampede,RushHour,GlassCannon,Stampede,RushHour,Stampede,RushHour,Stampede,RushHour,GlassCannon,RushHour,GlassCannon,RushHour,GlassCannon]`.

Seed commitment is `SHA-256(lp1("roady.v2.seed")||seed32)`. Schedule commitment is SHA-256 of `lp1("roady.v2.schedule")||u8(2)||u8(2)||u8(1)||lp1("rotation")||lp1(category)||seed32||u16BE(16)||records`; each record is `u8 effect||u64BE telegraphStart||u64BE activeStart||u64BE activeEnd||u64BE cooldownEnd`.

### 5.3 seed01 through seed20

seedNN is 32 consecutive bytes beginning with NN (`NN,NN+1,...,NN+31`).

| Seed | W0 | W1 | W2 | E0 | E1 | scheduleCommitCluck | scheduleCommitRow | seedCommit |
|---:|---|---|---|---|---|---|---|---|
| 01 | Stampede | GlassCannon | Stampede | ComboFrenzy | CritterBurst | `f287d212e7f0170ade4324886d33ea31f1ce45468e759e2bd4798bc9c6979ec1` | `bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e` | `1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b48` |
| 02 | Stampede | RushHour | Stampede | ChickenBurst | ChickenBurst | `334e4c0f526c8141668fe47efc4f6f33084b1200acd437c5371bb92a6ff255c7` | `4fa369f6bd2b5a2ddea60726a72a700559ae194cc8c5b19d61a15fe98a3defac` | `5e3366d30193ffc84bc561598cce68ff566448eff975c30f7950a14d9fc4093c` |
| 03 | RushHour | Stampede | RushHour | ChickenBurst | TrafficSurge | `949883a46b9af2b53f1baa4af4b43a3ba0fed43deb941fc046cb507576d781a7` | `3e611354756e820ee350fcdbac36412af08093c29cb6df261bef7daf3415d96a` | `9ee7af0b7c2ad1e12a27a376a4a176113385bbcb71ffa1a7c4c0cbe138c5babf` |
| 04 | RushHour | Stampede | GlassCannon | ComboFrenzy | ComboFrenzy | `e91a3d1d49dc5ec2bf4fa2149e3415c74f525455b838aa805f3d9ceecd97c2c3` | `511707e40e877d8f2a4377ff0c3c7b1dae2e93d8c62313927e62f8006a273eee` | `f6e037508be4596534bc0a6687f88616cdab314834379309341e6432b6263298` |
| 05 | GlassCannon | Stampede | GlassCannon | ChickenBurst | ChickenBurst | `65daafacc6cb058130efc7a7cb15ea6d2d6484199bd2e02a034626292d12af92` | `d5f192ef7a096d115a25c6fc39b00cf333dc637669b2953d17d99d874ae57b32` | `787bfeebf610c9e713c8e0b7138b4a0820cbc51748b89d7cd620f2eb8053360e` |
| 06 | GlassCannon | RushHour | GlassCannon | ChickenBurst | ComboFrenzy | `bf950fa753b6451fa566b73eb32f36e7a256c572eea96c6a1aa8a1a32d4b4200` | `4368c564e05348e06b53b8a044576fee3d7f4470b446fc45daa6dc4e3bf96a5c` | `a1a593496dce3d7c003a799e71defb6c60c3172d1a85781cf71187b715777fa4` |
| 07 | GlassCannon | RushHour | Stampede | CritterBurst | ComboFrenzy | `460c8485332aae4908ea528652c873585f9f99558d37919a18b218a34e189924` | `894f025408ad909581527fe27ddcbd57113e87e7ac9fbc74658840649d23f2f4` | `34a9bdfeaa3a8d478c3b97ee5840ec3f7993f35765f8feadefc4ee732b2dd43a` |
| 08 | RushHour | GlassCannon | Stampede | ComboFrenzy | CritterBurst | `833ec49d6c2aada73be6f872c02330833dea01aeb488f21c5c096ff532ed67fe` | `dd0eb4a80b78f32fb7b948eb99f95b207a01e2db2482466e63ef92faf9dc7d99` | `8129054312f2fae543ce070e550e4b173e4ef85c77e6686c0a82ff9fe3b0426b` |
| 09 | Stampede | GlassCannon | Stampede | ChickenBurst | CritterBurst | `61c0f30a16a7c4d03b856b3a396ad64b33f61e72fca35eeffa74066726a33749` | `deb5d0a5f2d62d7b22a31f2319eead87c0c013c96b94a3233c18c568ba68c37a` | `2f9a7a704f2aa8676693f846837c94c571336c02ac22bd1646b1109864273d8e` |
| 10 | GlassCannon | RushHour | Stampede | CritterBurst | CritterBurst | `45c7afc4e84f2733859099fc1803b91478e5b43a8a4ba7d827396b912b729e36` | `88f15f057071a469364c427fce1931954f55d5236c222c7c3098d2bc9589bcb5` | `83d8883ac422c10822fad3fb0075ffba947ec9a4f69c9307359ab95c6357f4ea` |
| 11 | GlassCannon | RushHour | Stampede | TrafficSurge | ComboFrenzy | `c2a7d64ab7802bfa5eb3fd9feee5b347c9b77f585523a019413a2f1bb0d0700f` | `9ef2cdd58be7135073053fea3aa101b8b058ddb996fe9bcda20ca79edd688e31` | `32ae12de16439abf296be1fdda0a4772bd799ee956bc97a5830364d966c40507` |
| 12 | Stampede | RushHour | GlassCannon | ChickenBurst | ChickenBurst | `fa8cae6c442c6ab16f1436b067628a660849f291548598ef306636ad25761225` | `1d1cf93b3eb3168c41949ce65dd2af1ff3856db6d9c92ca845a3b98f9f5cd7c4` | `542773e1dcdec7f27f7e4640faa0952e66cc7d16e2575b16bb71134af7f8652c` |
| 13 | GlassCannon | RushHour | Stampede | ChickenBurst | CritterBurst | `d418a5cac3e35063a237c22152b254787dfb17f4d74f2b717faa79aa24292e83` | `2838cd512820b3f74124b35b9a247fd71a739faa19ceae1f9a58d5b2779e30f6` | `687776b97fb817f5acf493072aa42ebb4cce1f8503e0c04692c9002c874eee2e` |
| 14 | GlassCannon | Stampede | GlassCannon | ChickenBurst | ComboFrenzy | `ab0f87d6bd1aa0aa40becbc939999318b096919c40c06ed499737a3179a0c437` | `ecaa99475910a42288628d6ff4b49eb6aedd22214fa73722428fe0e07d269e99` | `e74ebafaf7b5b8858c6d083501d3231d52ab64dbeffc08bde9bfe9b2a79cbba7` |
| 15 | GlassCannon | RushHour | GlassCannon | ChickenBurst | ChickenBurst | `9a7e90fd49090f41ad9bb1af30f947f41c7f994cd930dc83b6ccf36cb8b75ce1` | `bcbb71e41a8b97b9a178bb011c82f41b840754cd504a172cd2808e82d50d039a` | `2e32647c9cf1421344e2ed837d99291d26ea98b7d880b7dbea9844bc66b9c58c` |
| 16 | RushHour | Stampede | RushHour | CritterBurst | ChickenBurst | `48a75a340a46570e3bb6bfea3aa6e6739567da78beea186d537e461f20c6cfdb` | `13727477ab58fe5d873de4be11366edc480ba823595cad8de60256078a673b00` | `22782874120611f968dc4df9a302534b092e5d85f97189863d6abe44f57fb2db` |
| 17 | GlassCannon | RushHour | Stampede | CritterBurst | ComboFrenzy | `44abc664e087b818fab7c3c52b9a5b9ec314fe965cf443527db883c4f670e4e9` | `a5065400a24a96f354cdaeb067c6b17709a88e27d6d4a32d65f8fbac3c470e13` | `ba4a559bb957375c603af17cafb662ef475cd5efcd16cc61ebd10b4fb617d663` |
| 18 | RushHour | Stampede | GlassCannon | ComboFrenzy | TrafficSurge | `1ff8355a97c85e24d2e7c120134ba80dd8dc00c4911bce969cd049e962422d00` | `9e54baef1562faa6d4ff68aa3ab9abc086faada8d224b537b6fe60e332a17ddf` | `d713294ed3a7e5c2860d4c1880871222d4e7fc863866209c0a7e51ce0a7502fe` |
| 19 | GlassCannon | Stampede | RushHour | ChickenBurst | TrafficSurge | `1c661830b47f49760236f9ead230196d9dacb8c9b5af95e14ef3eef22ddfb16c` | `ac9548471d60f65d6ffc7521ca321a15c88f004a9d5579330522085149fec8c6` | `9ba2de0f69174ed1c39d11abc5f7743df9fb2949c27d838af9190b48a175908f` |
| 20 | RushHour | Stampede | RushHour | ChickenBurst | ChickenBurst | `3db7f479d56ddb5790c32765ca2c37adf0dccd1a6ba92fc3740ac722edc16228` | `abc51ca29d1dff0de62a6833101676833342353b169e0c15a986135d75cb38c8` | `12f7e80a5403318c8cacd471cec2e8347359b3733d316ec264a17716f44b6b56` |


### 5.4 seed01 Frenzy vector

Opportunities occur at 11446, 20456, 28802, 40458, 48549, and 58060ms. Roll residues mod10000 are 7564, 5850, 4625, 6756, 1830, and 7341. The first five fail; 58060 succeeds by the `t>=55000` pity rule. The v2 rules crate MUST generate and byte-lock this vector and every table row for Rust, TypeScript, WASM, and workerd.

## 6. CluckHunt scoring under rotation-v2 rules (exact)

- Chicken direct award per hit MUST be `CHICKEN_BASE_AWARD(1) +
  condition_bonus + event_bonus`; condition bonus is 0 under rotation (no
  ChickenFrenzy condition in the pool); ChickenBurst event bonus +1; Frenzy
  pickup bonus +1. All saturating add. Combo bonus is separate (below).
- Combo bonus MUST be `(combo_multiplier - 1) * glass_cannon_mult *
  combo_frenzy_mult`, saturating, where `glass_cannon_mult` is 2 while
  GlassCannon is the active rotation effect else 1, and `combo_frenzy_mult`
  is 2 while ComboFrenzy is the active scheduled event else 1. The base +1
  point is never multiplied. Per-hit reachable max combo-scaled award is 9
  (base 1 + (5-1)*2 at GlassCannon, or base 1 + (5-1)*2 at ComboFrenzy on a
  non-GlassCannon rotation).
- Coin award MUST be 1 per coin (`COIN_SCORE_AWARD`); MegaCoin MUST add 5 coin
  points and one `CoinCollected` event (`MEGA_COIN_POINTS`). Coin time
  transition MUST be `coin_time_after_collect(current) =
  min(current.clamp(0,90) + 1.5, 90.0)`. Time pickup transition MUST be
  `time_after_pickup(current) = min(current + 5.0, 99.0)`.
- Critter penalty MUST be `chickens.saturating_sub(2)` and health
  `25.0 * damage_mult` per 0.4s cooldown. Objective reward MUST be
  `chickens.saturating_add(10)` on the completion edge, once per round.
- Objective targets (CluckHunt, conduct-appropriate): hit 10 chickens,
  collect 6 coins, reach combo 3, cycling by `ObjectiveRoundIndex % 3`. One
  round-wide objective with one +10 award, independent of the active rotation
  effect. The completion edge MUST fire exactly once; the reward MUST apply
  before the Terminal ledger event.
- Ranked CluckHunt wave bonus: a Ranked CluckHunt run MUST add `+2` to the
  terminal chicken bucket per completed wave (one full 28000 ms cadence with
  at least one active effect). The wave bonus MUST NOT be multiplied by any
  premium or combo. Casual CluckHunt MUST NOT add a wave bonus.

## 7. Chicken Frenzy pickup

### 7.1 Frenzy rebalance (frozen)

Chicken target 28; population multiplier exactly 2x; direct bonus +1; combo
bonus multiplier none (MUST NOT multiply combo); telegraph 2000 ms; active
duration 15000 active-play ms; with Chicken Burst cap 40 chickens.

### 7.2 Spawn schedule, lifetime, collection (corrected, frozen)

- Eligible after 8000 active-play ms; at most one orb per round.
- The first seeded pickup opportunity MUST occur at
  `8000 + (frenzy_interval_draw % 4001)` active-play ms (NOT exactly 8000).
  Each subsequent opportunity time MUST equal the previous opportunity time
  plus `8000 + (next_frenzy_interval_draw % 4001)` ms. `frenzy_interval_draw`
  is the next `u64` from the frenzy interval stream.
- Each eligible check uses the Frenzy roll stream and succeeds exactly when
  `frenzy_roll_draw % 10000 < 400` (4 percent). If no orb has spawned by 55000
  ms, the next eligible check MUST force spawn (pity). Spawning consumes the
  one-per-round allowance whether collected or expired.
- Orb lifetime MUST be 12000 active-play ms. Collection MUST use XZ distance
  `< 1.2`. At the exact ms `spawn_time + 12000`, expiry MUST precede
  collection: a coincident expiry and collection at `spawn_ms + 12000` MUST
  resolve as expiry and MUST NOT collect (same-ms precedence: activation/combo
  expirations before segment ends before segment starts before activation
  spawns before activation collections before activation activations before
  gameplay transitions before terminal; here the orb expiry at `spawn_ms +
  12000` is an activation expiration and is processed before a collection at
  the same ms). Collection MUST start a 2000 ms telegraph, then 15000 ms active
  Frenzy. Pause MUST freeze eligibility, lifetime, telegraph, and active
  duration.

### 7.3 Relocation

At orb age 6000ms relocation is evaluated once. `approached` means the car was within 20.0 XZ units at any prior active tick. `invalid` means no finite road segment within 4.0. `unreachable` means not approached and current distance exceeds 45.0. If invalid or unreachable, consume exactly 16 draws from `frenzy.relocation`, two for each of eight candidates. Candidate coordinates in the car right/forward basis are `lateral=(drawA%2001-1000)*22/1000` and `ahead=13.75+(drawB%1001)*11.25/1000`. Select the first candidate within 4.0 of a finite road and outside spawn exclusions; otherwise keep the original.

### 7.5 Stream independence

Interval, Frenzy roll, ordinary pickup kind, spawn position, and relocation
streams MUST be independent deterministic PRNG domains. Each MUST advance only
for its named decision; an unrelated RNG consumer MUST NOT perturb any Frenzy
stream.

## 8. RightOfWay (exact, frozen)

### 8.1 Packages

- The player MUST carry at most 3 packages. A delivery MUST consume all carried packages sequentially and MUST award each in delivery order.
- Package base award MUST be 5. `delivery_chain` is the checked u32 count of packages delivered since the last animal hit, across any number of drop-offs. Before each package award, `chain_bonus=delivery_chain`; after the award, increment the chain by one. Thus the first three packages since a hit award bases 5, 6, and 7; later packages continue 8, 9, and so on until an animal hit resets the chain to zero.
- Each delivered package MUST add 3000 ms to the remaining clock as
  `remaining_ms = min(remaining_ms + 3000, 90000)` (a cap on the remaining
  clock, NOT a cap on cumulative bonus). The package time bonus therefore
  saturates the remaining clock at 90000 ms.

### 8.2 Coins and courtesy (exact bands and rearm)

- Coin base award MUST be 1; each coin MUST add `remaining_ms =
  min(remaining_ms + 1500, 90000)` (cap on the remaining clock, not cumulative
  bonus). MegaCoin applies the CluckHunt MegaCoin rule only in CluckHunt; in
  RightOfWay coin awards use base 1 unless a conduct rule states otherwise
  (none does, so coins are +1).
- Courtesy base award MUST be 2, applied only when car speed `>= 4`, strictly
  outside the chicken-hit threshold and no farther than one car width beyond
  it. The concrete courtesy band is `1.0 < XZ_distance <= 2.12` units from the
  chicken center, where 1.0 is the chicken hit radius (`HIT_RADIUS`) and the
  outer bound is `1.0 + 1.12 = 2.12` (one car half-width `1.12` from
  `car.rs::car_footprint_half_extents`). Courtesy MUST NOT be awarded at
  distance `<= 1.0` (that is a hit, penalized) nor at distance `> 2.12`.
- Courtesy MUST rearm per chicken only after the car leaves that chicken's
  courtesy band (XZ distance `> 2.12`). A global 500 ms cooldown MUST gate
  consecutive courtesy awards across all chickens. A courtesy award counts for the objective only when its credited score after premium/guilt multiplication is greater than zero.

### 8.3 Animal hits (exact signed penalty and decay)

- A chicken or critter hit MUST apply a signed `-10` score delta to the
  RightOfWay accumulator. The premium MUST decay to
  `floor(previous_premium_bps * 9000 / 10000)`. The delivery chain bonus MUST
  reset to 0. A 5000 ms guilt window MUST be refreshed (set to 5000 ms
  remaining).

### 8.4 Positive awards, mission, and wave (exact premium arithmetic)

- A positive award MUST compute `value = floor(base * premium_bps / 10000)`.
  If guilt is active, credited value MUST be `floor(value * 5000 / 10000)`;
  otherwise credited value MUST be `value`. The remaining clock MUST NOT be
  affected by positive awards (only package/coin time bonuses affect the
  clock).
- Mission and wave awards MUST be multiplied by the premium using this
  formula. A Ranked RightOfWay run MUST add `+2` per completed wave (one full
  28000 ms cadence with at least one active effect), multiplied by the
  premium. A Casual RightOfWay run MUST NOT add a wave bonus.

### 8.5 Worked arithmetic example (verified)

Premium 10000 bps, no guilt, no prior hits: package 1 (base 5, chain 0)
awards 5 and +3000 ms; package 2 (base 5, chain 1) awards 6 and +3000 ms;
package 3 (base 5, chain 2) awards 7 and +3000 ms; total package score 18,
time bonus 9000 ms (remaining clock saturates at 90000 only if it was already
near the cap; otherwise it increases by 9000). Coin (base 1) awards 1 and
+1500 ms. Courtesy (speed >= 4, base 2) awards 2. Ranked wave +2 awards 2.
After one animal hit, premium becomes `floor(10000*9000/10000) = 9000`, chain
resets to 0, guilt is active (5000 ms); a package with base 5 during guilt
awards `floor(floor(5*9000/10000)*5000/10000) = floor(4*5000/10000) = 2`.

### 8.6 Accumulation and terminal (exact checked arithmetic)

- All RightOfWay score deltas MUST accumulate in a signed `i64` with checked
  overflow rejection; an overflow MUST be hard-rejected. At terminal, the
  accumulated value MUST be clamped to zero (`max(accumulated, 0)`), then
  converted to `u32` with a checked conversion that MUST reject any value
  exceeding `u32::MAX`. The terminal total MUST equal that result.
- The RightOfWay terminal aggregate is NOT `chickens + coins`. The backend
  (section 12.5) MUST carry RightOfWay-specific aggregates and MUST NOT apply
  the `terminal_total == chickens + coins` invariant to RightOfWay.

### 8.7 RightOfWay objectives (conduct-appropriate)

- RightOfWay MUST use conduct-appropriate neutral fixed targets, NOT "hit 10
  chickens" (hitting chickens is penalized in RightOfWay, so a chicken-hit
  objective would contradict the conduct). The RightOfWay objective cycle MUST be: deliver 3 packages, earn 3 courtesy awards, collect 6 coins, cycling by `ObjectiveRoundIndex % 3`. One round-wide objective with one +10 award (multiplied by the premium per section 8.4). The completion edge MUST fire exactly once; the reward MUST apply before the Terminal ledger event.

### 8.8 Casual condition composition and prohibitions

Casual RightOfWay defaults to Standard whenever the player has not explicitly selected a condition, regardless of Ranked capability or availability. Manual IDs compose as follows: Standard is neutral; RushHour applies the exact traffic count/speed multipliers in section 4.3 for the full round; ChickenFrenzy applies a full-round chicken target of 35 and regular-coin target of 3 per road block but gives no chicken-hit points (hits remain -10); Stampede doubles the baseline critter target and preserves chase behavior; GlassCannon doubles obstacle/critter damage and does not multiply RightOfWay score awards. Casual retains the selected classic condition's two scheduled v1 events, including their population/traffic effects, but ComboFrenzy never multiplies RightOfWay score awards and ChickenBurst only changes chicken population. The separate seeded Frenzy orb and ranked wave +2 are disabled in Casual. RightOfWay MUST NOT revoke a license or define micro-objectives.

## 9. Objectives, UI, and local persistence

### 9.1 Conduct-appropriate objectives (frozen)

| Conduct | Slot 0 | Slot 1 | Slot 2 |
|---|---|---|---|
| cluck_hunt | Hit 10 chickens | Collect 6 coins | Reach combo 3 |
| right_of_way | Deliver 3 packages | Earn 3 courtesy awards | Collect 6 coins |

Both conducts cycle by `ObjectiveRoundIndex % 3`. One round-wide objective
with one +10 award. The completion edge MUST fire exactly once per round. The
reward MUST apply before the Terminal ledger event. RightOfWay objective
targets count fully delivered packages and credited courtesy awards, never chicken hits.

### 9.2 HUD and accessibility

- The implementation MUST extend the existing event/status panel into one
  unified rules-status panel with at most two compact rows: rotating effect
  and scheduled event. Chicken Frenzy MUST use the existing power-up/status
  region. Independently positioned panels with non-audited coordinates MUST
  NOT be added.
- Telegraphs MUST show name, ASCII signature, countdown, and a static
  segmented bar; color MUST be supplementary. Reduced motion MUST remove
  pulsing and flashing and MUST retain text, brackets, countdown, and bar
  state. The unified panel MUST pass 844x390, 960x480, and 1440x900 overlap
  audits before visual implementation. Signatures (frozen): RushHour
  `>> TRAFFIC`; Stampede `** CRITTERS`; GlassCannon `!! GLASS`; ChickenFrenzy
  `<> FRENZY`.

### 9.3 Local persistence and medals

- Persistence MUST record only terminal totals from completed rounds.
  In-progress peaks MUST NOT become records. A record MUST be written only if
  the terminal total strictly improves the global or category record. The v1
  key `car_game_best` and schema `v1:global:k0:k1:k2:k3:k4` MUST remain
  unchanged. v2 MUST use the namespaces in section 1.4. Medals MUST be computed from terminal bests only. Casual namespaces retain the selected condition's v1 thresholds. Ranked CluckHunt thresholds are 50/150/300 and Ranked RightOfWay thresholds are 30/90/180 for Bronze/Silver/Gold; these values belong to `roady-rules.v2` and never alter v1.

Medal thresholds (v1, frozen, per condition, sourced from `persist.rs::medal_for`):

| Condition | Bronze | Silver | Gold |
|---|---:|---:|---:|
| Standard | 20 | 40 | 70 |
| RushHour | 15 | 30 | 55 |
| ChickenFrenzy | 35 | 65 | 100 |
| Stampede | 15 | 25 | 45 |
| GlassCannon | 25 | 50 | 80 |

## 10. Pause, restart, and terminal ordering

- Schedule, active effects, seeded pickup state, budgets, and ledger sequence
  MUST survive pause/resume unchanged. Resume MUST NOT emit duplicate
  segment-start or activation events and MUST NOT re-arm the schedule or
  re-seed pickups. A fresh restart MUST clear all rotation state and MUST
  obtain a new one-time session and seed. The Terminal ledger event MUST be
  appended after final objective processing and reward and before the GameOver
  snapshot. Terminal MUST always be the last ledger event.

## 11. Canonical big-endian binary protocol (v2)

### 11.1 Integer widths and length prefixes

- u8: enum ordinals, flags, version bytes, kind, platform, source, effect.
- u16 big-endian: schedule record count only.
- u32 big-endian: event/segment/activation counts, seq, nonnegative score aggregates, objective target, reward, counts, and blob lengths.
- u64 big-endian: all ms timestamps.
- i64 big-endian: RightOfWay signed score deltas (section 12.5).
- 32 raw bytes: hashes, no length prefix.
- lp1 (u8 length then UTF-8): sessionId, challenge, mode, categoryKey, build;
  1 to 255 bytes.
- lp4 (u32 BE length then bytes): ledger and evidence blobs; 0 to 524288 bytes.

### 11.2 Domain strings (frozen)

Each MUST be emitted as `lp1(domain)` at the head of its block: session header
`roady.v2.session` (16 bytes), score HMAC `roady.v2.score` (14), event chain `roady.v2.event` (14), final root `roady.v2.root` (13), schedule `roady.v2.schedule` (17), seed commitment `roady.v2.seed` (13), worker proof `roady.v2.proof` (14), evidence `roady.v2.evidence` (17).

### 11.3 Enum ordinals

Conduct: CluckHunt=0, RightOfWay=1. Event kinds: ChickenHit=1, CoinCollected=2, TimePickup=3, ObjectiveCompleted=4, CritterPenalty=5, SegmentChanged=6, Terminal=7, PackagePickup=8, PackageDelivery=9, CourtesyAward=10, AnimalHit=11, WaveAward=12, CoinAward=13, FrenzyChanged=14. Reason: TimeUp=1, Wrecked=2. Platform: Web=1, Native=2. Effect: Standard=0, RushHour=1, ChickenFrenzy=2, Stampede=3, GlassCannon=4. Objective: HitChickens=1, CollectCoins=2, ReachCombo=3, DeliverPackages=4, CourtesyAwards=5.

### 11.4 Limits and primitives

Maximum events 4096; canonical ledger 262144 bytes; event record 192 bytes; schedule segments 16; activations 32; score body 16384; evidence body 524288; build64 UTF-8 bytes; name3..5 ASCII alphanumeric; remaining0..99000ms. Integers are big-endian: u8 enums/flags, u16 schedule record count, u32 event count/sequence and nonnegative counts, i32 animal-hit delta, u64 milliseconds, i64 signed accumulator. `lp1` is u8 length plus bytes; `lp4` is u32 length plus bytes; hashes are raw32.

### 11.5 Session and schedule bytes

The unstarted header is `lp1("roady.v2.session")||u8(2)||u8(2)||u8(1)||lp1("rotation")||lp1(category)||lp1(sessionId)||lp1(challenge)||seedCommit32||scheduleHash32||u64 issuedAt||u64 startByExpiry||u8(0)||u64(0)`. The started header has the same fields but encodes `u64(0)||u8(1)||u64 startedAt` after issuedAt. The unstarted proof is `HMAC-SHA-256(proofKey,lp1("roady.v2.proof")||unstartedHeader)` and the started proof uses the same domain plus startedHeader; both are canonical unpadded base64url. Schedule bytes are the exact construction in section5.2 with seed32 and 16 records.

### 11.6 Event record and payload layouts

An event record is exactly `lp1("roady.v2.event")||u32 seq||u64 activeMs||u8 kind||payload`. It does not contain a previous hash. `eventHash=SHA-256(previousHash32||eventRecord)`, where the first previous hash is `h0=SHA-256(canonicalStartedSessionHeader)`; store `eventRecord||eventHash32` in the evidence ledger. Thus no domain or previous hash is duplicated.

Payloads:

- ChickenHit: `u32 base,u32 eventBonus,u32 frenzyBonus,u8 comboBefore,u8 comboAfter,u32 bucketBefore,u32 bucketAfter`.
- CoinCollected: `u8 mega,u32 base,u8 comboBefore,u8 comboAfter,u32 bucketBefore,u32 bucketAfter,u64 remainingBefore,u64 remainingAfter`.
- TimePickup: `u64 remainingBefore,u64 remainingAfter`.
- ObjectiveCompleted Cluck: `u8 objective,u32 target,u32 baseReward,u32 bucketBefore,u32 bucketAfter`.
- CritterPenalty: `u32 penalty,u32 bucketBefore,u32 bucketAfter,u64 cooldownAfter`.
- SegmentChanged: `u8 segmentKind,u8 effectOrEvent,u8 active,u64 start,u64 end`.
- FrenzyChanged: `u8 phase,u64 start,u64 end` (phase Spawned=1,Telegraph=2,Active=3,Expired=4).
- PackagePickup: `u8 carriedBefore,u8 carriedAfter`.
- PackageDelivery: exactly one record per package delivered, in package order: `u8 deliveredOrdinalWithinDropoff` (0,1,2), `u32 chainIndex,u32 base,u32 premium,u8 guilt,u32 credited,i64 accumulatorBefore,i64 accumulatorAfter,u64 remainingBefore,u64 remainingAfter`. A three-package drop-off emits three consecutive records; each record increments delivery_chain after its transition.
- CourtesyAward: `u32 chickenStableId,u32 premium,u8 guilt,u32 credited,i64 before,i64 after,u32 cooldownAfter`.
- AnimalHit: `u8 animalKind,i32 delta,u32 premiumBefore,u32 premiumAfter,u64 guiltAfter,i64 before,i64 after`.
- WaveAward: `u32 base,u32 premium,u8 guilt,u32 credited,i64 before,i64 after`.
- CoinAward: `u32 base,u32 premium,u8 guilt,u32 credited,i64 before,i64 after,u64 remainingBefore,u64 remainingAfter`.
- ObjectiveCompleted RightOfWay: `u8 objective,u32 target,u32 base,u32 premium,u8 guilt,u32 credited,i64 before,i64 after`.
- Terminal Cluck: `u8 conduct0,u8 reason,u32 total,u32 chickens,u32 coins,u8 objective,u8 maxCombo,u64 duration,u64 remaining,lp1 build,u8 platform`.
- Terminal RightOfWay: `u8 conduct1,u8 reason,u32 total,i64 accumulator,u32 premium,u32 packages,u32 courtesy,u32 hits,u32 maxDeliveryChain,u8 objective,u64 duration,u64 remaining,lp1 build,u8 platform`.

### 11.7 Ledger envelope, chain, root, and score HMAC

Evidence bytes are `lp1("roady.v2.evidence")||lp1(sessionId)||u32 eventCount||lp4(concatenated stored event records)`. `evidenceHash=SHA-256(evidenceBytes)` and JSON `evidenceHash` is its 64-character lowercase hex encoding. `h0=SHA-256(startedSessionHeader)` and each eventHash follows section11.6. The last record is Terminal and its eventHash is hN. `conductAggregates` is the corresponding Terminal payload without kind. `finalRoot=SHA-256(lp1("roady.v2.root")||h0||hN||conductAggregates)`; eventCount occurs only in the evidence envelope, not again in aggregates/root. Score-HMAC input is `lp1("roady.v2.score")||u8(2)||u8(2)||u8(1)||lp1("rotation")||lp1(category)||lp1(sessionId)||finalRoot32||scheduleHash32||seedCommitment32||conductAggregates`; use HMAC-SHA-256 and canonical unpadded base64url.

RightOfWay score-HMAC golden uses category `rotation.v1.right_of_way`, session `S01`, finalRoot byte11 repeated32, schedule hash `bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e`, seed commitment `1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b48`, reason TimeUp, total17, accumulator17, premium9000, packages3, courtesy2, hits1, maxDeliveryChain3, objective1, duration60000, remaining5000, build dev, platform web. The exact input length is derived from the final layout (including the 15-byte `lp1("roady.v2.score")` field); the v2 rules fixture MUST publish its hex and HMAC using key `roady-v2-test-client-key` and block ranked enablement until Rust/TS/WASM/workerd agree. Existing v1 canonical goldens remain byte-frozen in their current fixtures.

### 11.8 Same-millisecond ordering and evidence authority

Order is expirations, segment ends, segment starts, activation spawns, collections, activations, gameplay events by kind then stable entity ID, Terminal. At Frenzy spawn+12000 expiry precedes collection. Evidence is bounded client consistency evidence, never authority; fabricated consistent ledgers remain possible.

## 12. Strict JSON API (v2)

### 12.1 General rules

- Every `/v2` route MUST reject unknown JSON fields, malformed or oversized
  bodies, and MUST apply CORS from configured allowed origins. Errors MUST be
  `{"error":{"code":CODE,"message":MSG,"requestId":ID}}` with a `requestId`
  in every response. Preflight `OPTIONS` MUST return 204 with CORS headers.
  The fail-closed config guard MUST reject any request when secrets, caps, or
  origins are missing or placeholder, returning `503 config_error`.

### 12.2 GET /v2/capabilities

- MUST return 200
  `{"ranked":{"enabled":BOOL,"categories":[KEY,KEY]},"rulesVersion":2,"protocolVersion":2,"policyVersion":1}`.
  MUST set
  `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`.
  MUST NOT require authentication or a body.

### 12.3 POST /v2/session

- Body MUST be exactly
  `{"mode":"rotation","categoryKey":CATEGORY,"turnstileToken":TOKEN}`.
  Unknown fields MUST be rejected with `422 invalid_body`. `categoryKey` MUST
  be a valid category key.
- Turnstile MUST require action `roady_score_session` and a hostname matching
  an allowed origin. The always-pass test secret MUST be rejected outside dev
  builds.
- Response MUST be 200
  `{"sessionId":ID,"challenge":CHALLENGE,"mode":"rotation","categoryKey":CATEGORY,"seedHex":HEX64,"seedCommitment":HEX64,"scheduleHash":HEX64,"issuedAt":MS,"startByExpiry":MS,"proof":B64URL}`. `seedHex` is the Worker-generated 32-byte seed returned over TLS so the client can generate the bound schedule; the Worker also stores its encrypted form and commitment.
  `startByExpiry` MUST equal `issuedAt + 300000`. MUST set
  `Cache-Control: no-store`. `RATE_LIMIT_SESSION` MUST be required and MUST
  fail closed with `429 rate_limited`.

### 12.4 POST /v2/session/:id/start

- URL `:id` and body `sessionId` MUST be byte-identical; mismatch returns 409 `session_mismatch`. Request body MUST be exactly `{"sessionId":ID,"proof":UNSTARTED_PROOF}` with unknown fields rejected. Missing session returns 404 `invalid_session`; invalid proof returns 401 `invalid_proof`. MUST atomically claim an unstarted, unexpired session only if `used=0` and `startByExpiry>now`. Already started returns 409 `replay`; unstarted but expired returns 409 `expired_session`. Success returns 200
  `{"started":true,"startedAt":MS,"proof":STARTED_PROOF}` where `STARTED_PROOF` is the Worker HMAC over the canonical started header (`startedFlag=1`, `startedAt=MS`, `startByExpiry=0`). The unstarted proof is invalid for score submission. MUST NOT impose a completion TTL after start. MUST set `Cache-Control: no-store`.

### 12.5 POST /v2/scores (conduct-aware)

- The request body MUST be selected by `categoryKey` conduct. Unknown fields
  MUST be rejected with `422 invalid_body`.
- CluckHunt body MUST be exactly
  `{"sessionId":ID,"proof":B64URL,"name":NAME,"categoryKey":"rotation.v1.cluck_hunt","terminalTotal":INT,"chickens":INT,"coins":INT,"objectiveCompleted":BOOL,"maxCombo":INT,"roundDurationMs":INT,"timeLeftMs":INT,"gameOverReason":REASON,"build":STR,"platform":STR,"finalRoot":HEX64,"scheduleHash":HEX64,"eventCount":INT,"signatureKeyId":"v2.client.1","protocolVersion":2,"rulesVersion":2,"policyVersion":1}`.
  `terminalTotal` MUST equal `chickens + coins` and fit `u32`.
- RightOfWay body MUST be exactly
  `{"sessionId":ID,"proof":B64URL,"name":NAME,"categoryKey":"rotation.v1.right_of_way","terminalTotal":INT,"signedAccumulator":"CANONICAL_I64_DECIMAL","premiumBps":INT,"packagesDelivered":INT,"courtesyCount":INT,"animalHits":INT,"maxDeliveryChain":INT,"objectiveCompleted":BOOL,"roundDurationMs":INT,"timeLeftMs":INT,"gameOverReason":REASON,"build":STR,"platform":STR,"finalRoot":HEX64,"scheduleHash":HEX64,"eventCount":INT,"signatureKeyId":"v2.client.1","protocolVersion":2,"rulesVersion":2,"policyVersion":1}`. The signed accumulator is a string matching `^-?(0|[1-9][0-9]{0,18})$`, parsed as signed i64; `-0` and unsafe JSON numeric i64 values are rejected.
  `terminalTotal` MUST equal `max(0, signedAccumulator)` checked-converted to
  u32; it MUST NOT be required to equal `chickens + coins` (RightOfWay carries
  no `chickens`/`coins` fields). `signedAccumulator` MUST fit `i64` and
  `terminalTotal` MUST fit `u32`.
- Common field rules: `timeLeftMs` MUST be `<= 99000`. `roundDurationMs` MUST be a non-negative safe integer. CluckHunt `maxCombo` MUST be 1 to 5. RightOfWay `maxDeliveryChain` MUST fit u32. `name` MUST match `^[A-Z0-9]{3,5}$`. `gameOverReason` MUST be `time_up` or `wrecked`; `platform` MUST be `web` or `native`.
- The request MUST include `X-Roady-Client-Signature` as unpadded base64url.
  The Worker MUST verify the client HMAC over v2 canonical score bytes (with
  the conduct-specific aggregate fields in their frozen order), the session
  started proof, one-time use, and category binding. A started session has no completion expiry and score submission MUST NOT apply `startByExpiry` or any wall-clock completion TTL. After all validation, score submission MUST execute `UPDATE sessions_v2 SET used=1 WHERE session_id=:id AND started=1 AND used=0`; exactly one changed row is required before insertion. Zero changed rows returns `409 replay`. If the subsequent score insert fails, return `500 insert_failed`; the claimed session remains consumed.
- A ranked v2 score MUST be inserted as `pending`, never directly `live`.
  Response MUST be 201
  `{"inserted":true,"rank":null,"globalRank":null,"categoryKey":CATEGORY,"total":INT,"submittedAt":MS,"status":"pending","evidenceCapability":B64URL,"evidenceExpiresAt":MS}`.
  `evidenceCapability` MUST bind to score id, session id, `finalRoot`, and a
  24-hour expiry; D1 MUST store only its hash. MUST set
  `Cache-Control: no-store`. `RATE_LIMIT_SUBMIT` MUST be required.

### 12.6 POST /v2/evidence

- Body MUST be exactly `{"evidenceCapability":B64URL,"finalRoot":HEX64,"ledgerBytes":B64URL,"evidenceHash":HEX64}`, at most 524288 bytes. The ledger is at most 262144 canonical bytes and 4096 events. Unknown capability returns 404 `invalid_capability`; expired capability returns 409 `expired_capability`. An identical live retry returns 200 `{"accepted":true,"idempotent":true,"status":"live","rank":INT}`. An identical quarantined retry returns 200 `{"accepted":false,"idempotent":true,"status":"quarantined","rank":null}`. A different root/hash/decoded-ledger-byte reuse returns 409 `evidence_conflict`.
- Successful evidence replay MUST transition the score from `pending` to `live`. Evidence/root disagreement MUST transition to `quarantined` with an exact consistency reason and MUST NOT rewrite history. Missing evidence after 24 hours MUST transition to `unranked_missing_evidence`. A first accepted replay returns 201 `{"accepted":true,"idempotent":false,"status":"live","rank":INT}`; an identical retry returns 200 with `idempotent:true`. A mismatch returns 409 `evidence_conflict` after transitioning to `quarantined`. `unranked_missing_evidence` is applied only by the 24-hour expiry job. MUST set `Cache-Control: no-store`.

### 12.7 GET /v2/leaderboard

- MUST accept query `categoryKey` (required), `limit` (1 to 100, default 25),
  `offset` (>= 0). MUST return 200
  `{"categoryKey":KEY,"entries":[{"rank":INT,"name":STR,"score":INT,"submittedAt":MS}],"generatedAt":MS}`
  ordered by `terminal_total DESC, submitted_at ASC, id ASC` within the category. The query MUST filter `status='live'`; pending, quarantined, unranked_missing_evidence, hidden, and deleted rows never appear. MUST set
  `Cache-Control: public, max-age=30, s-maxage=60, stale-while-revalidate=120`
  on the origin-agnostic cached body. `RATE_LIMIT_READ` MUST apply and MUST
  fail closed for a configured binding error.

### 12.8 GET /v2/me/rank

- MUST accept query `sessionId`. For every `status='live'` score, including `submission_source='admin_restore'`, return 200 `{"sessionId":ID,"status":"live","rank":INT,"categoryKey":KEY,"entry":{name,total,submittedAt,submissionSource},"nearby":[{rank,name,score,submittedAt}]}`. For pending, quarantined, unranked_missing_evidence, hidden, or deleted rows return 200 with `rank:null` and `nearby:[]`. An unused session returns `403 invalid_session`. MUST set `Cache-Control:private,no-store`; `RATE_LIMIT_RANK` applies.

### 12.9 POST /v2/admin/scores/restore

- MUST require `Authorization: Bearer LB_ADMIN_TOKEN` with constant-time comparison. Cluck `known` is exactly `{name,categoryKey,terminalTotal,chickens,coins,objectiveCompleted,gameOverReason}` and `synthetic` is exactly `{maxCombo,roundDurationMs,timeLeftMs,build,platform,submittedAt}`. RightOfWay `known` is exactly `{name,categoryKey,terminalTotal,signedAccumulator,premiumBps,packagesDelivered,courtesyCount,animalHits,maxDeliveryChain,objectiveCompleted,gameOverReason}` and `synthetic` is exactly `{roundDurationMs,timeLeftMs,build,platform,submittedAt}`. The outer body is exactly `{restorationKey,evidenceHash,known,synthetic,reason}` and is at most 4096 bytes.
- For restoration, `payloadHash=SHA-256(UTF-8(JSON.stringify(requestObject)))`, where the server reconstructs the object in exact outer key order `restorationKey,evidenceHash,known,synthetic,reason` and nested objects use the field order specified above; no whitespace is emitted. Set `sessionId="admin_restore:"+restorationKey`, challenge/proof `admin_restore`, seed_enc to exactly 32 zero bytes, seedCommitment/scheduleHash to 64 zero hex, issued_at/started_at/submitted_at to `synthetic.submittedAt`, start_by_expiry NULL, started=used=1, Turnstile=0, finalRoot=evidenceHash, eventCount=1, ipHash `admin_restore`, status live, submissionSource admin_restore, and no evidence capability. Insert the synthetic session, score, `admin_restorations_v2`, and `moderation_log_v2`; every generated field is synthetic. Identical restorationKey/evidenceHash/payloadHash returns 200 `{"restored":true,"idempotent":true,"scoreId":INT}`; differing reuse returns 409 `restoration_conflict`. First success returns 201 `{"restored":true,"idempotent":false,"scoreId":INT}`. MUST set `Cache-Control:no-store`.

### 12.10 Moderation routes

- `POST /v2/admin/scores/:id/hide` MUST set status `hidden` and return 200
  `{"ok":true,"id":ID,"status":"hidden"}`. `DELETE /v2/admin/scores/:id` MUST
  set status `deleted` and return 200 `{"ok":true,"id":ID,"status":"deleted"}`.
  Both MUST require admin authorization (returning `401 unauthorized` on mismatch) and MUST write a `moderation_log_v2` row.

### 12.11 Status and error code summary

| Condition | Status | Code |
|---|---:|---|
| unknown version tuple/category | 422 | unknown_version_tuple |
| malformed/oversized body | 422 | invalid_body |
| total mismatch (CluckHunt) | 422 | total_mismatch |
| RightOfWay aggregate mismatch | 422 | total_mismatch |
| invalid signed i64 decimal | 422 | invalid_signed_accumulator |
| evidence/root/replay mismatch | 409 | evidence_conflict |
| u32/i64 overflow | 422 | score_overflow |
| invalid name | 422 | invalid_name |
| Turnstile failure | 422 | turnstile_failed |
| missing client signature | 401 | missing_signature |
| signature mismatch | 401 | invalid_signature |
| unknown session | 404 | invalid_session |
| invalid session proof | 401 | invalid_proof |
| URL/body session mismatch | 409 | session_mismatch |
| unknown evidence capability | 404 | invalid_capability |
| expired evidence capability | 409 | expired_capability |
| restoration key conflict | 409 | restoration_conflict |
| replay | 409 | replay |
| expired session | 409 | expired_session |
| condition/category mismatch | 409 | condition_mismatch |
| rate limited | 429 | rate_limited |
| config error | 503 | config_error |
| not found | 404 | not_found |
| internal error | 500 | internal_error |

### 12.12 CORS, cache, idempotency

- CORS headers MUST be reapplied per request to origin-agnostic cached bodies.
- `no-store` MUST be set on session, start, score, evidence, rank, and admin
  mutation responses. Read routes MUST cache without per-origin CORS headers
  and MUST reapply CORS on serve.
- Score submission is intentionally non-idempotent: the one-time session claim makes every retry return `409 replay`. Evidence submission is idempotent only when capability, root, evidence hash, and decoded `ledgerBytes` are byte-identical. Admin restoration identical retries return 200 with the original score ID; conflicts return 409. First successful restoration returns 201 `{"restored":true,"idempotent":false,"scoreId":INT}`.

## 13. Migration 0005

### 13.1 Additive SQL

Migration 0005 MUST be additive to migrations 0001 through 0004 and MUST NOT
alter any existing table.

```sql
CREATE TABLE score_categories (
  category_key TEXT PRIMARY KEY, rules_version INTEGER NOT NULL,
  display_name TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1))
);
INSERT INTO score_categories (category_key, rules_version, display_name, active) VALUES
  ('rotation.v1.cluck_hunt', 2, 'Cluck Hunt', 1),
  ('rotation.v1.right_of_way', 2, 'Right of Way', 1);

CREATE TABLE sessions_v2 (
  session_id TEXT PRIMARY KEY, category_key TEXT NOT NULL,
  protocol_version INTEGER NOT NULL, rules_version INTEGER NOT NULL,
  policy_version INTEGER NOT NULL, mode TEXT NOT NULL,
  challenge TEXT NOT NULL, proof TEXT NOT NULL, seed_enc BLOB NOT NULL,
  seed_commitment TEXT NOT NULL CHECK(length(seed_commitment) = 64),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash) = 64),
  issued_at INTEGER NOT NULL, start_by_expiry INTEGER,
  started_at INTEGER,
  started INTEGER NOT NULL DEFAULT 0 CHECK(started IN (0, 1)),
  used INTEGER NOT NULL DEFAULT 0 CHECK(used IN (0, 1)),
  turnstile_verified INTEGER NOT NULL DEFAULT 0 CHECK(turnstile_verified IN (0, 1)),
  ip_hash TEXT NOT NULL,
  CHECK((started = 0 AND started_at IS NULL AND start_by_expiry IS NOT NULL)
     OR (started = 1 AND started_at IS NOT NULL AND start_by_expiry IS NULL)),
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key)
);

CREATE TABLE scores_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
  category_key TEXT NOT NULL,
  terminal_total INTEGER NOT NULL CHECK(terminal_total >= 0),
  -- CluckHunt aggregate buckets (NULL for right_of_way)
  chickens INTEGER CHECK(chickens IS NULL OR chickens >= 0),
  coins INTEGER CHECK(coins IS NULL OR coins >= 0),
  -- RightOfWay conduct-specific aggregates (NULL for cluck_hunt)
  signed_accumulator INTEGER,
  premium_bps INTEGER CHECK(premium_bps IS NULL OR (premium_bps >= 0 AND premium_bps <= 10000)),
  packages_delivered INTEGER CHECK(packages_delivered IS NULL OR packages_delivered >= 0),
  courtesy_count INTEGER CHECK(courtesy_count IS NULL OR courtesy_count >= 0),
  animal_hits INTEGER CHECK(animal_hits IS NULL OR animal_hits >= 0),
  objective_completed INTEGER NOT NULL CHECK(objective_completed IN (0, 1)),
  max_combo INTEGER CHECK(max_combo IS NULL OR max_combo BETWEEN 1 AND 5),
  max_delivery_chain INTEGER CHECK(max_delivery_chain IS NULL OR max_delivery_chain >= 0),
  round_duration_ms INTEGER NOT NULL CHECK(round_duration_ms >= 0),
  time_left_ms INTEGER NOT NULL CHECK(time_left_ms >= 0),
  game_over_reason TEXT NOT NULL CHECK(game_over_reason IN ('time_up', 'wrecked')),
  build TEXT NOT NULL,
  platform TEXT NOT NULL CHECK(platform IN ('web', 'native')),
  session_id TEXT NOT NULL UNIQUE, submitted_at INTEGER NOT NULL, ip_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending','live','quarantined','unranked_missing_evidence','hidden','deleted')),
  moderation_note TEXT,
  submission_source TEXT NOT NULL DEFAULT 'verified'
    CHECK(submission_source IN ('verified', 'admin_restore')),
  restoration_key TEXT,
  final_root TEXT NOT NULL CHECK(length(final_root) = 64),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash) = 64),
  event_count INTEGER NOT NULL CHECK(event_count BETWEEN 1 AND 4096),
  evidence_capability_hash TEXT UNIQUE CHECK(evidence_capability_hash IS NULL OR length(evidence_capability_hash)=64),
  evidence_expires_at INTEGER,
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key),
  FOREIGN KEY(session_id) REFERENCES sessions_v2(session_id),
  CHECK(
    (category_key = 'rotation.v1.cluck_hunt' AND chickens IS NOT NULL AND coins IS NOT NULL
       AND signed_accumulator IS NULL AND max_combo IS NOT NULL
       AND max_delivery_chain IS NULL AND terminal_total = chickens + coins)
    OR
    (category_key = 'rotation.v1.right_of_way' AND signed_accumulator IS NOT NULL
       AND premium_bps IS NOT NULL AND packages_delivered IS NOT NULL
       AND courtesy_count IS NOT NULL AND animal_hits IS NOT NULL
       AND max_delivery_chain IS NOT NULL AND max_combo IS NULL
       AND chickens IS NULL AND coins IS NULL
       AND terminal_total = MAX(0, signed_accumulator))
  )
);

CREATE TABLE score_evidence (
  id INTEGER PRIMARY KEY AUTOINCREMENT, score_id INTEGER NOT NULL UNIQUE,
  session_id TEXT NOT NULL,
  final_root TEXT NOT NULL CHECK(length(final_root) = 64),
  evidence_hash TEXT NOT NULL CHECK(length(evidence_hash) = 64),
  ledger_bytes BLOB NOT NULL CHECK(length(ledger_bytes) BETWEEN 1 AND 262144),
  replay_result TEXT NOT NULL CHECK(replay_result IN ('match','mismatch')),
  quarantine_reason TEXT,
  uploaded_at INTEGER NOT NULL,
  FOREIGN KEY(score_id) REFERENCES scores_v2(id),
  FOREIGN KEY(session_id) REFERENCES sessions_v2(session_id)
);

CREATE TABLE admin_restorations_v2 (
  restoration_key TEXT PRIMARY KEY,
  evidence_hash TEXT NOT NULL UNIQUE CHECK(length(evidence_hash)=64),
  payload_hash TEXT NOT NULL CHECK(length(payload_hash)=64),
  category_key TEXT NOT NULL,
  known_json TEXT NOT NULL CHECK(json_valid(known_json)),
  synthetic_json TEXT NOT NULL CHECK(json_valid(synthetic_json)),
  reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 256),
  score_id INTEGER NOT NULL UNIQUE,
  restored_at INTEGER NOT NULL,
  admin TEXT NOT NULL,
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key),
  FOREIGN KEY(score_id) REFERENCES scores_v2(id)
);

CREATE TABLE moderation_log_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  action TEXT NOT NULL CHECK(action IN ('hide','delete','restore')),
  target_score_id INTEGER NOT NULL,
  admin TEXT NOT NULL,
  at INTEGER NOT NULL,
  note TEXT,
  FOREIGN KEY(target_score_id) REFERENCES scores_v2(id)
);

CREATE UNIQUE INDEX idx_scores_v2_restoration_key
  ON scores_v2(restoration_key) WHERE restoration_key IS NOT NULL;
CREATE INDEX idx_sessions_v2_expires_at ON sessions_v2(start_by_expiry);
CREATE INDEX idx_scores_v2_category
  ON scores_v2(category_key, status, terminal_total DESC, submitted_at ASC, id ASC);
CREATE INDEX idx_scores_v2_submitted_at ON scores_v2(submitted_at);
CREATE INDEX idx_scores_v2_pending_evidence ON scores_v2(status,evidence_expires_at) WHERE status='pending';
CREATE INDEX idx_score_evidence_score_root ON score_evidence(score_id,final_root);
```

### 13.2 Seed storage and D1 semantics

- `sessions_v2.seed_enc` MUST store encrypted seed material the Worker MUST
  decrypt for schedule generation. `sessions_v2.seed_commitment` MUST store
  the SHA-256 seed commitment. The Worker MUST store encrypted usable seed material and the commitment, and MUST return `seedHex` only in the TLS-protected session response so the client can execute the deterministic schedule.
- Migration 0005 MUST NOT claim D1 transaction semantics that D1 lacks. The
  Worker MUST use `D1Database.batch` for multi-statement atomicity where D1
  supports it and MUST NOT assume cross-statement serializable isolation
  beyond what `batch` provides. The atomic start claim is the single `UPDATE sessions_v2 SET started=1,started_at=:now,start_by_expiry=NULL WHERE session_id=:id AND started=0 AND used=0 AND start_by_expiry>:now`; exactly one changed row is required. The score-use claim is the single `UPDATE sessions_v2 SET used=1 WHERE session_id=:id AND started=1 AND used=0`; exactly one changed row is required.

## 14. Lifecycle, security, and privacy

### 14.1 Session lifecycle

- An unstarted session MUST be started within five minutes of issuance;
  `startByExpiry` MUST equal `issuedAt + 300000`. After the atomic start claim,
  a started session MUST have no wall-clock completion deadline. A restart
  MUST always obtain a new session and seed. A pause resume MUST preserve all
  run state and MUST NOT create a new session. An offline or native run MUST
  remain unranked unless it has a valid pre-play receipt. Cleanup can remove
  explicitly abandoned unstarted sessions but MUST NEVER reject a completed
  run solely because play outlived a TTL.

### 14.2 Score and evidence lifecycle

- A ranked v2 score MUST be inserted as `pending` and MUST transition to
  `live` only after successful evidence replay. Evidence/root disagreement
  MUST transition to `quarantined`. Missing evidence after 24 hours MUST
  transition to `unranked_missing_evidence`. Hidden or deleted scores MUST be
  excluded from rank queries. Ordering MUST remain
  `terminal_total DESC, submitted_at ASC, id ASC` within each category.

### 14.3 Turnstile, proof, and HMAC

- Turnstile MUST be required for session issuance with action
  `roady_score_session` and a hostname matching a configured allowed origin.
  The always-pass test secret MUST be rejected outside dev builds.
- The Worker MUST issue a signed session proof using HMAC-SHA-256 over
  canonical session bytes. The client MUST submit a nuisance HMAC signature
  over canonical score bytes; this MUST NOT be treated as proof of honest
  gameplay.

### 14.4 Key rotation

- The Worker MUST accept old and new client HMAC key IDs during rotation. The
  client MUST switch to the new key ID after Worker acceptance. The old key
  MUST be retired only after the supported old-client window closes. Rotation
  MUST be observed through non-sensitive telemetry.

### 14.5 Rate and replay defenses

- Fixed per-hashed-IP limits per 60 seconds are: capabilities and leaderboard reads 30 (`RATE_LIMIT_READ`), session issuance 3 (`RATE_LIMIT_SESSION`), session start/score/evidence 5 (`RATE_LIMIT_SUBMIT`), rank 60 (`RATE_LIMIT_RANK`). Write bindings MUST exist and fail closed; public reads permit an absent binding only in unsupported test runtimes and fail closed on a configured binding error. Atomic claims prevent concurrent replay.

### 14.6 Privacy and retention

- The Worker MUST NEVER store a raw client IP. IP attribution MUST be
  `base64url(SHA-256(clientIP + pepper))`. Scheduled cleanup MUST delete
  expired sessions and MUST hide live scores older than 90 days outside the
  top 1000 per category. Retention MUST preserve per-player ranks for active
  top-1000 scores.

## 15. Tests

### 15.1 Required test vectors

- Exact 8000/3000/18000/7000 phase boundaries.
- Signed seed and schedule parity across native Rust, game WASM, Worker WASM,
  and workerd.
- Anti-repeat selector vectors for seeds 01 through 20.
- 30, 60, and 120 FPS population budget equivalence.
- Pause/resume MUST emit no duplicate transitions.
- Restart MUST use a new session and clear prior state.
- Same-ms ordering at every segment boundary, including Frenzy
  `spawn_ms + 12000` expiry-before-collection.
- Objective reward MUST precede Terminal.
- Frenzy 4 percent check, 55000 ms pity, one opportunity, expiry, relocation
  (8 candidates, fixed draw count), and stacking cap.
- No same-flavor rotation/event overlap from the exclusion algorithm in section 4.4.
- Classic v1 responses MUST remain byte-identical with v2 data present.
- Category and board isolation and deterministic ties.
- Canonical Rust, TypeScript, and WASM hashes and replay outputs MUST match.
- RightOfWay replay arithmetic: package chain bonuses, premium decay
  `floor(prev*9000/10000)`, guilt `floor(value*5000/10000)`, signed i64
  accumulation, terminal clamp-and-check.
- 844x390, 960x480, and 1440x900 unified-panel layout audits.
- Browser errors, shader errors, request failures, and reduced-motion
  behavior.
- Shadow phase MUST show zero unexplained aggregate/root mismatch before
  ranked enforcement.

### 15.2 Rotation and golden assertions

- The test suite MUST assert seed01 produces
  `W0=Stampede, W1=GlassCannon, W2=Stampede` and
  `E0=ComboFrenzy, E1=CritterBurst`.
- The test suite MUST assert seed01 CluckHunt schedule commitment `f287d212e7f0170ade4324886d33ea31f1ce45468e759e2bd4798bc9c6979ec1`, RightOfWay commitment `bb785fb44d72ad7ea1b957df9bcc95dffdd814a475e736a0e74beceee2d3049e`, and seed commitment `1f79a204b991758a8798f650465fc89634f967a3976312a2eaaff5912bbd8b48`.
- The test suite MUST assert the stream anchors and first draws in sections 5.1-5.2, seed01 Frenzy opportunity vector in 5.4, v1 goldens in existing v1 fixtures, and the RightOfWay score-HMAC fixture in 11.7. Cross-language fixtures MUST prove each maximum-size event fits the 192-byte cap.

## 16. Deployment gates

### 16.1 Migration stages

1. Freeze classic v1: byte-lock score and session fixtures; keep classic UI,
   storage, and boards unchanged.
2. Shared rules v2: deterministic schedule and PRNG domains; integer-ms
   transitions; exact effect composition and caps; canonical event enums,
   encoding, and hash fixtures.
3. Local shadow ledger: record classic rounds without network enforcement;
   validate pause, restart, terminal, objective, pickup, and boundary order.
4. v2 infrastructure: separate routes, tables, category registry; pre-play
   Worker sessions and signed seeds; exact compatibility allowlist; no ranked
   v2 writes.
5. Opt-in rotation without Frenzy: schedule, reconciliation, unified HUD,
   separate unranked board; compare score distribution, crashes, abandonment,
   and lifecycle parity.
6. Frenzy activation pickup: round-scoped deterministic pickup schedule;
   exact 2x/+1/15-second rules; verify max-one opportunity and pity
   boundaries.
7. Evidence upload and replay: moderation-gated and sampled evidence first;
   cross-language replay and root comparison; fix all false mismatches without
   weakening assertions.
8. Ranked rotation board: require clean terminal and successful replay;
   pending to live only after evidence validation; classic remains separate.
9. Production review: after two stable production weeks, review fairness, distributions, abandonment, layout, moderation load, and legitimate rejection rate without changing the Ranked CluckHunt default.

### 16.2 Enforcement and compatibility gates

- Ranked enforcement (the `pending` to `live` transition after evidence
  replay) MUST remain disabled until native Rust, game WASM, Worker
  WASM/workerd, and browser lifecycle fixtures demonstrate parity.
- Shadow validation MUST show zero unexplained aggregate/root mismatch before
  ranked enforcement.
- The default mode MUST remain Ranked CluckHunt in every stage (section 1.3).
  The capabilities gate (section 1.3) disables Ranked for a session when
  `capabilities.ranked.enabled === false`; that runtime fallback is not a
  contractual default change.
- v1 routes and tables MUST remain byte-identical and readable throughout all
  stages. v2 data MUST NOT alter v1 leaderboard output. The classic
  compatibility period MUST preserve every v1 byte, artifact, ID 0 through 4,
  route, table, and record.
