# Pro Circuit career (ProCircuit.cpp, ProCircuitProcs.cpp)

These notes come from the Ghidra decomp of `Mcm2.exe`. Addresses are given in brackets; constants marked "read" were read from the executable through REA. The port's code is `crates/mm2-viewer/src/procircuit.rs` (the rules) and `src/screens/pro.rs` (the screens).

## Data files

- **`UI/UIRES.RES` → `PCTables.pb`, section `[ProCircuit]`** (read by 0x4d7bd0; the code's defaults follow in brackets):
  - Money: `StartingCapital` (5000), `PurseMultiplier` (17.5).
  - Entry fees: `EnduroFee` (300), `BajaFee` (500), `NationalsFee` (1500), `SupercrossFee` (2500).
  - Caps: `RepairCapPct` (0.1), `MedicalCapPct` (0.05), `StuntCapPct` (0.075).
  - Prize shares: `1_PlacePct` … `11_PlacePct` (0 when absent). If they sum to 1 or less, the remainder is added to first place.
  - The fees sit at +0x1229 + 4·event, using the race event numbers: 1 Baja, 2 Nationals, 3 Supercross, 5 Enduro.
- **`PCSched.pb`**: one section per series (`[Enduro]`, `[Baja]`, `[Nationals]`, `[Supercross]`) with keys:
  - `NumberOfTracks`, `Laps` (3 when absent), `BonusTrack` (0 means none)
  - `Track_N`: the track file's stem under `Teraform\<event>`
- **`UI\PCNames.txt`**: the computer riders' names, one per line. Each rider gets a different random name.
- **`bikes.pb` / `riders.pb`**: `Availability` (1–3) is the career level at which a manufacturer or outfit becomes available.

## The circuit (saved whole, 0x1225 bytes, 0x4d8850 load / 0x4d88a0 save)

| offset | field |
|---|---|
| 0x00 | name[64] |
| 0x40 | series (event number): starts at 5 Enduro, then 1 Baja, 2 Nationals, 3 Supercross |
| 0x44 | race number in the series, from 1 |
| 0x48 | level 1–3 (which bikes, outfits and rivals are available) |
| 0x4c | class: 1 = 125, 2 = 250, 3 = Open |
| 0x50 | difficulty 1–3 (Easy, Normal, Hard), the race's AI level |
| 0x54 | announcement text[1024] |
| 0x454 | total entry fees paid |
| 0x460 | riders (opponents + the player) |
| 0x464 | flags: 1 = bailout offered, 2 = advertising offered, 4 = out (failed), 8 = finished |
| 0x465 | riders[n], 0x140 bytes each; the player is rider 0 |

**Each rider:**

| offset | field |
|---|---|
| +0x00 | last race: points |
| +0x04 | last race: winnings |
| +0x08 | last race: repairs |
| +0x0c | last race: medical |
| +0x10 | last race: rank |
| +0x14 | last race: stunt money |
| +0x18 | season total: points |
| +0x1c | season total: winnings |
| +0x20 | season total: repairs |
| +0x24 | season total: medical |
| +0x28 | overall rank |
| +0x2c | season total: stunt money |
| +0x30 | cash (the player's starts at `StartingCapital`) |
| +0x34 | debts |
| +0x38 | bike name[64] |
| +0x78 | bike paint[64] |
| +0xb8 | outfit[64] |
| +0xf8 | name[32] (the player's is the profile name) |
| +0x118 | bike short name[32] |
| +0x138 | engine cc |
| +0x13c | four-stroke flag |

## Rivals' rides (0x4d8dc0, run when a circuit starts and at each new level)

Each computer rider gets a random unused outfit, taken from the highest availability that is ≤ level. They also get a random unused bike of the highest availability ≤ level, whose engine class (125/250/400/500/600 → 0–4) fits the circuit's class:
- **125:** class 0.
- **250:** class 1, or class 2 about 20% of the time. A draw is kept when `rand/32768 ≤ 0.8` for class 1 and `> 0.8` for class 2 (read).
- **Open:** class 3 or 4.

## A race (Start in PCC, 0x4da6c0)

The race uses these settings:
- Mode: tournament for Start, practice for Practice.
- Event: the series.
- Laps: the schedule's.
- Opponents: riders − 1 for Start, none for Practice.
- AI level: the difficulty.
- Track: `Track_<race>`.

Baja and Enduro are course races.

**Points by place** (0x4600f0): 20, 17, 15, 12, 10, 8, 6, 5, 3, 1 for places 1–10. Nothing beyond 10th or for a DNF.

**Each racer's result** (0x45d670, from the bike):
- place (+0x784)
- the dollar units for repairs (+0x7ac), medical (+0x7b0) and stunts (+0x7b4)

Repair and medical units grow only in 0x48ea00, once per crash (on its first step): each gets `rand01 × 0.667`, and for a computer rider first `rand01 × 0.15` as well. `mm2_physics::career::crash_units` has this. They are zeroed in 0x490a10.

## Settling a race (0x4dd440, when the race's standings page opens)

The purse is `purse = int(fee × PurseMultiplier)`. `share[i] = PlacePct[i] / Σ PlacePct[0..riders]`. Total fees += fee.

For each racer:

| amount | formula |
|---|---|
| medical | `min(purse × MedicalCapPct, medical_units × 250)` (read) |
| repairs | `min(purse × RepairCapPct, repair_units × 500)` (read) |
| stunt money | `min(purse × StuntCapPct, stunt_units × 400)` (read) |
| winnings | `int(floor(purse × share[place − 1]))` |

The racer's cash then changes by `stunt − medical − repairs − fee + winnings`, and the race amounts are added to the season totals.

The purse left over (`purse − Σ winnings`) goes to the race winner. The race winner is ranked by this race's points, and the remainder is added to their winnings, total winnings and cash.

Two rankings follow, both sorted by points with the highest first: the race rank (+0x10) and the overall rank (+0x28).

## After the race (PCLRace, 0x4dce60)

Opening the results page settles the race (0x4dd440), fills the lists from the race tallies (sorted by race rank), then advances the circuit (0x4d88f0, below) and saves it. So by the time Back is clicked, the race number is already the next race's. Back then does this:

1. **Bonus offer.** If the next race is the series' bonus track (which only the leader reaches, see Advancing) and the player ranks 1st overall, the bonus offer comes up (PCDeal 0x4de4d0): title 5207/5208, text 5209/5210, "Bonus Cash:" $3000, which is added to cash as the page opens. Accept races the bonus track. That race is not settled: when it ends, the circuit advances and carries on as Continue does, with no results page (0x49aaa0, message 0x88e), so the $3000 is all it pays. Decline advances past it and carries on the same way (0x4d92f0).
2. **Out of money.** Otherwise, if the player's cash is below the series' fee, the following happen in order:
   1. A **Bailout Offer** (0x4dd7c0, 5193/5194), once. It offers enough to cover the fee: `fee − cash`. The amounts: Your Balance (green, or red when below 0), Next Race Fee (red), Cash Offered (green). Accepting adds the offer to cash and to debts, saves, and goes to PCC.
   2. An **Advertising Offer** (0x4de060, 5201/5202), once. The signing bonus is `fee − cash + debts`. The amounts: balance, fee, Debts (5203, red), Signing Bonus (5199) and Mascot Bonus Per Race (5200), shown as $1000 (0x4de30f) though nothing pays it. Accepting adds `bonus − debts` to cash (so cash comes to the fee). The deal also puts the sponsor's mascot on the podium: with flag 2 set, the race end loads `Winnerd.mcf` instead of `Winner.mcf` (0x45eea9).
   3. Declining either offer, or running out after both, puts the player **out**: flag 4, then PCTrans (0x4ddb40: text 5220, picture FailScreen, button 5198 "Results...") → PCFinish.
3. **Otherwise** it carries on as Continue does (0x4d92f0): a finished circuit goes to PCTrans (0x4de9b0: title 5213, text 5214 plus 5215 when the player is 1st, picture WinScreen, button 5198) → PCFinish; a series' first race goes to PCTrans (0x4de7d0: title 5223, text 5221 Enduro / 5204 Baja / 5205 Nationals / 5206 Supercross, picture WinScreen / BajaScreen / NatScreen / SuperScreen, button 2326 "Continue") → PCC; any other race goes straight to PCC.

## Advancing (0x4d88f0)

The race number goes up by 1. A series ends once the race number passes `NumberOfTracks`, or when it reaches `BonusTrack` while the player is not 1st overall (only the leader races the bonus track).

| series ends | next |
|---|---|
| Enduro (5) | Baja (1), level 2 |
| Baja | Nationals (2), level 3 |
| Nationals | Supercross (3) |
| Supercross | finished |

Each new level re-picks the rivals' rides (0x4d8dc0) and adds string 5212 ("New Bikes and Rider Uniforms…") to the announcement. When the circuit finishes, flag 8 is set and the winnings are banked. Finishing in the top three sets the award-bike flag (global +0x1ee8 |= 1, string 5215).

**Announcement text** is the track's description:

| series | string id |
|---|---|
| Enduro | 5279 + race |
| Baja | 5289 + race |
| Nationals | 5349 + race |
| Supercross | 5399 + race |

**A new circuit's announcement** is strings 5308–5311 (welcome and goals).

## Screens

- **PCStart:**
  - Current circuits are listed in LstName/Rank/Class/Diff/Points/Cash. Rank is shown as "%d / %d", the class as "125", "250" or string 5327 "Open", and the difficulty as 5323–5325.
  - Finished circuits are listed in the LstDone* columns, with status 5186 Failed or 5187 Complete.
  - Buttons: New, Continue, Delete, with the confirm 5180–5182.
- **PCNew:** the name, the class radios (default 250), the difficulty radios (default Normal) and 3–10 opponents (default 7).
- **PCC:** tabs Next Race (5190), Bike & Rider (5191) and Standings (5192); the pages go at (16, 41). Buttons: Start, Practice, Options (the options screen), Controls (the options screen's controls page), Help (MCM2HELP), Back (save, then PCStart).
- **PCCNext:**
  - Fee and purse.
  - Track, type (5101/5103/5104/5106) and "race / count".
  - Cash, fees, repairs and medical.
  - The announcement.
  - The track picture.
- **PCCStand:** rank, name, points, bike, cash, winnings and expenses.
- **PCLRace:** the race's rank, name, points, bike, winnings, repairs and medical.
- **PCFinish:** the season totals.
- **PCDeal:** the title, the description, and up to five money lines in Item1–5.
- **PCDone:** shown when the player fails.
