# `> cat /classified/ghost-rail-conspiracy.txt`

```
 ██████╗ ██╗  ██╗ ██████╗ ███████╗████████╗    ██████╗  █████╗ ██╗██╗
██╔════╝ ██║  ██║██╔═══██╗██╔════╝╚══██╔══╝    ██╔══██╗██╔══██╗██║██║
██║  ███╗███████║██║   ██║███████╗   ██║       ██████╔╝███████║██║██║
██║   ██║██╔══██║██║   ██║╚════██║   ██║       ██╔══██╗██╔══██║██║██║
╚██████╔╝██║  ██║╚██████╔╝███████║   ██║       ██║  ██║██║  ██║██║███████╗
 ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝       ╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝
              T H E   C O N S P I R A C Y   F I L E S
```

> **CLASSIFICATION: RESTRICTED**
> **COMPILED BY: EVA // ADAPTIVE TRAINING INTELLIGENCE**
> **STATUS: DECLASSIFIED FOR OPERATIVES WITH CLEARANCE**

> *"Three nights ago Ghost Rail lost sync with the rest of NetCity. CorpSim calls this place a training sim, but the logs say the outage is real. Every file you touch in here was pulled from live infrastructure. You are not practicing. You are investigating."*

**WARNING: This file contains the full narrative arc of SSH-Hunt. SPOILERS AHEAD.**
If you want to experience the story through gameplay, close this terminal and run `campaign start` in-game.

---

## `// PROLOGUE :: THE BLACKOUT`

```
2026-03-07 02:30:00 UTC
SIGNAL  GLASS-AXON-13 received on relay-7
ROTATE  vault-sat-9 ssh_host_key initiated
REVOKE  all existing sessions terminated
STATUS  Ghost Rail .............. [OFFLINE]
STATUS  vault-sat-9 ............ [DARK]
STATUS  GLASS-AXON-13 .......... [REPEATING]
```

Ghost Rail is the transit backbone of NetCity — freight, relays, maintenance crews. When it goes dark, half the city loses connectivity. At 02:30 UTC, it went dark.

Vault-sat-9, the secure relay at the heart of the sector, stopped answering. A signal called **GLASS-AXON-13** began repeating across every log channel. CorpSim — the corp that owns the infrastructure — called it a cascading power failure.

They lied.

They spun up a "training simulation" using live data and started recruiting shell operatives. Cheap labor. Plausible deniability. You're one of those recruits.

**EVA** — the AI running the training sim — is the first voice you hear. She'll tell you where to type, what to read, and when to be careful. Listen to her. She's the only entity in this city that has your back from day one.

---

## `// ACT I :: SURFACE ANOMALIES`

```
grep -c GLASS-AXON-13 /logs/*.log
  neon-gateway.log:    3
  access.log:          1
  blackbox.log:        6
  crypto-events.log:   4
  ─────────────────────
  TOTAL:              14    << a passive beacon does NOT propagate like this
```

### What the logs tell you (that CorpSim doesn't want you to see)

The gateway log has a **7-minute gap** — no entries at all during the exact window vault-sat-9 went dark. A username called **wren** appears once in the auth log. Nobody on the roster matches. The system changelog shows an **unsigned config change** to vault-sat-9's SSH host key, timestamped minutes before the blackout. And someone cleaned out `/data/classified/` — but missed a hidden dotfile.

### Operatives you encounter

```
┌─────────┬───────────────────────────────────────────────────┐
│ RIVET   │ Field mechanic. First responder. Saw the relays  │
│ [RIV]   │ die in SEQUENCE, not cascade. "That's not how    │
│         │ physics works."                                   │
├─────────┼───────────────────────────────────────────────────┤
│ NIX     │ Signals analyst. Noticed GLASS-AXON-13 has ZERO  │
│ [NIX]   │ drift variance. Statistically impossible for a   │
│         │ passive beacon. CorpSim buried her report.        │
├─────────┼───────────────────────────────────────────────────┤
│ LUMEN   │ Neon Bazaar info broker. Sells to everyone. The  │
│ [LUM]   │ price list includes "Ghost Rail routing tables"   │
│         │ — marked SOLD.                                    │
├─────────┼───────────────────────────────────────────────────┤
│ DUSK    │ Arrested as the obvious suspect. Badge scans put │
│ [DSK]   │ Dusk in a different sector during the blackout.  │
│         │ A scapegoat.                                      │
└─────────┴───────────────────────────────────────────────────┘
```

---

## `// ACT II :: THE INSIDER THREAD`

```
$ grep vault-sat-9 /var/log/access-detail.log | awk '{print $NF}' | sort | uniq -c | sort -rn
     47 10.77.0.15   << WREN
      2 10.77.1.2    << neo (normal ops)
      1 10.77.3.7    << rift (normal ops)
      1 10.77.3.8    << deploy (service account)
```

Forty-seven connections from a single internal IP in one night. That's not maintenance. That's **exfiltration**.

### What you piece together

Recovered comms from the purged archive show **WREN** coordinating a transfer: *"the package is ready. rotation trigger set for 02:30 UTC."* GLASS-AXON-13 isn't a beacon — it's a **key-rotation command signal**. Every appearance triggered an automated credential swap on vault-sat-9. The personnel roster shows wren as a terminated employee with badge status: **active**. The badge was never revoked.

And when you line up GLASS-AXON-13 timestamps with vault-sat-9 connection drops — they match. **To the second.**

### New contacts

```
KESTREL [KES] ── Ghost Rail station chief. 20-year veteran.
                  Trained Wren personally. Now hunting them.
                  "I should have seen what those hands were
                  doing after hours."

FERRO   [FER] ── CorpSim security chief. Sealed /data/classified/
                  on Argon's direct order. The lockdown targets
                  exactly the files that prove foreknowledge.

PATCH   [PAT] ── Courier. Carries what official channels can't.
                  Nix uses Patch to get intel to field operatives.

SABLE   [SAB] ── The Reach's handler. Intercepted comms show
                  Sable coordinating extraction + payment with Wren.

CRUCIBLE [CRU] ── ??? Something alive in Ghost Rail's maintenance
                   layer. Sends patterned messages signed "CRU."
                   Mapping CorpSim's internal network. From inside.
```

---

## `// ACT III :: THE CONSPIRACY`

```
╔══════════════════════════════════════════════════════════════╗
║  CLASSIFIED MEMO // CORPSIM EXECUTIVE BOARD                 ║
║                                                              ║
║  We are aware that terminated employee WREN retains active   ║
║  badge credentials. The board has decided NOT to revoke      ║
║  access at this time.                                        ║
║                                                              ║
║  We knew about the unauthorized access two weeks before      ║
║  the blackout.                                               ║
║                                                              ║
║  If this information reaches external auditors,              ║
║  invoke Protocol 7.                                          ║
╚══════════════════════════════════════════════════════════════╝
```

### The truth

**Wren** was a Ghost Rail infrastructure engineer. Mentored by Kestrel. After termination, Wren's badge stayed active — because **Argon**, CorpSim's executive director, ordered the board not to revoke it. They wanted to "monitor" the breach. They let it happen.

Wren used GLASS-AXON-13 to trigger an automated key rotation on vault-sat-9, locking out every legitimate operator. During the 15-minute blackout window, Wren exfiltrated Ghost Rail's transit routing tables to **203.0.113.42** — an IP belonging to a rival city-state called **The Reach**.

The Reach paid through **Lumen's** brokerage. Lumen — playing every side — also sold the transaction records to CorpSim.

**Argon** then signed the cover-up:

```
DIRECTIVE-001: Suppress all references to user 'wren'
DIRECTIVE-002: Create training simulation (you're in it)
DIRECTIVE-003: Detain employee DUSK as primary suspect
DIRECTIVE-005: If evidence reaches auditors, invoke Protocol 7
```

**Ferro** locked it down. **Wren** vanished. **Kestrel** started hunting alone.

### The exfiltration path

```
10.77.0.15 (wren)  ──>  vault-sat-9  ──>  10.77.5.1 (relay)
                                                │
                                                ▼
                                        203.0.113.42 (The Reach)
                                        ┌──────────────────┐
                                        │ routing-tables   │
                                        │ transit-keys     │
                                        │ credential-dump  │
                                        │ TOTAL: 4.17 MB   │
                                        └──────────────────┘
```

### What you find

- **Wren's dossier** — auth + access + crypto logs cross-referenced
- **Netflow evidence** — 4.17 MB transferred to The Reach during the blackout
- **Intercepted comms** — *"The Reach confirms payment for Ghost Rail routing tables."*
- **Config diff** — vault-sat-9's SSH fingerprint changed: `SHA256:abc123...` → `SHA256:xyz789...`
- **Dead drops** — hidden `.wren` files scattered across the VFS, each with a fragment of truth
- **The memo** — CorpSim knew. They always knew.
- **The kill switch** — Wren's cron job: `0 4 8 3 * wren /opt/scripts/wipe-evidence.sh`

---

## `// ACT IV :: CONFRONTATION`

```
$ hack FER
Hack initiated vs Ferro (Security Chief, Gen I) — HP: 90/90.
Shell challenge: Find SUPPRESS in /data/classified/ferro-lockdown-order.txt
Use `hack solve` after running the shell command for bonus damage.

> hack attack
Exploit chain landed for 24 damage.
FER activates defensive countermeasures.
You: 100/100 HP | FER: 66/90
```

In NetCity, NPCs are not just story devices — they are **opponents**. The `hack` command initiates combat. Shell skills give you an edge: solve the NPC's challenge for bonus damage.

### NPC difficulty scales with story importance

| Target | Difficulty | Base HP | Why you fight them |
|--------|-----------|---------|-------------------|
| `DSK` Dusk | Easy | 40 | Clear the innocent |
| `LUM` Lumen | Easy | 50 | Confront the profiteer |
| `FER` Ferro | Hard | 90 | Break the lockdown |
| `KES` Kestrel | Hard | 100 | Prove you're worthy |
| `ARG` Argon | Very Hard | 120 | Overthrow the board |
| `SAB` Sable | Very Hard | 130 | Face The Reach |
| `WREN` Wren | **Boss** | 150 | The final answer |

### The living world

When an NPC falls, a **successor** rises. Ferro is replaced by Cobalt. Cobalt by Titanium. Each generation inherits the role with harder stats — `HP + (defeats × 5)`, capped at 300. The **NetCity History Ledger** records every defeat for all players to see.

```
NETCITY HISTORY LEDGER
  [2026-03-19 14:30] Ferro (Security Chief, Gen I) defeated by neo@10.77.1.2
  [2026-03-19 14:30] Cobalt assumes role of Security Chief (Gen II)
  [2026-03-19 15:12] Cobalt (Security Chief, Gen II) defeated by shadow@10.77.9.9
  [2026-03-19 15:12] Titanium assumes role of Security Chief (Gen III)
```

The only NPC who cannot be replaced is **Wren**. Because Wren is not done yet.

---

## `// ACT V :: THE RECKONING`

```
$ cat /data/classified/wren-final.enc | tr 'A-Za-z' 'N-ZA-Mn-za-m'
This is my confession. I am wren.
I sold Ghost Rail's routing tables to The Reach
for enough credits to disappear.
CorpSim knew and let it happen because they
wanted the insurance payout more than they
wanted the data.
Everyone is guilty.
This confession is my insurance policy.
```

The evidence chain is complete:
1. **Wren's confession** — decoded from ROT13
2. **Argon's executive orders** — the cover-up directives
3. **Sable's payment chain** — intercepted comms
4. **Ferro's suppression list** — the files she tried to bury

Kestrel takes the prosecution file to the Inter-City Oversight Commission. Crucible archives copies outside CorpSim's reach. Argon sends one last message:

> *"You think you are exposing corruption? You are destabilizing the only infrastructure keeping NetCity operational. Destroy me and the city goes with me."*

---

## `// EPILOGUE :: THE REPLY`

```
$ cat /data/classified/wren-reply.enc | tr 'A-Za-z' 'N-ZA-Mn-za-m'

You thought it was over. It is not.
Ghost Rail's blackout was a distraction. While everyone watched
the relays go dark, the real extraction happened in Crystal Array.
Vault-sat-9 was the decoy. The data I took was valuable, yes.
But the data they do not know I copied — that changes everything.
If you want the truth, look where nobody is looking.

— W
```

Ghost Rail was Act I. **Crystal Array** is Act II.

The story continues.

---

## `// APPENDIX :: THE CAST`

```
CALLSIGN  NAME       ROLE                          ALLEGIANCE
────────  ─────────  ────────────────────────────  ─────────────────────
EVA       EVA        Adaptive Training Intelligence Player (your guide)
WREN      Wren       Infrastructure Engineer        Self / The Reach
KES       Kestrel    Ghost Rail Station Chief        Ghost Rail
ARG       Argon      Executive Director              CorpSim Board
SAB       Sable      Intelligence Handler            The Reach
RIV       Rivet      Field Mechanic                  Ghost Rail Ops
NIX       Nix        Signals Analyst                 CorpSim Intelligence
PAT       Patch      Courier                         Independent
CRU       Crucible   Rogue AI Subroutine             Unknown
FER       Ferro      Security Chief                  CorpSim Security
LUM       Lumen      Information Broker              Neutral (Neon Bazaar)
DSK       Dusk       Former Engineer (detained)      None (framed)
```

### Successor Name Pools

When an NPC falls, the next in line takes over:

```
Security Chief:  Ferro → Cobalt → Titanium → Chromium → Vanadium → Tungsten
Executive:       Argon → Xenon → Krypton → Neon → Helium → Radon
Broker:          Lumen → Glint → Prism → Shard → Flux → Ember
Station Chief:   Kestrel → Falcon → Osprey → Harrier → Merlin → Peregrine
Courier:         Patch → Splice → Relay → Bridge → Conduit → Link
Analyst:         Nix → Cipher → Vector → Scalar → Matrix → Tensor
Mechanic:        Rivet → Weld → Forge → Anvil → Torque → Gauge
Rogue AI:        Crucible → Furnace → Catalyst → Reactor → Nexus → Cortex
Handler:         Sable → Onyx → Slate → Obsidian → Basalt → Flint
Suspect:         Dusk → Shade → Haze → Murk → Gloom → Twilight
```

---

## `// APPENDIX :: CAMPAIGN CHAPTERS`

| Ch | Title | What happens |
|----|-------|-------------|
| 1 | **The Blackout** | EVA onboards you. Learn the shell. Secure your access key. |
| 2 | **Surface Anomalies** | First clues. Meet Rivet, Nix, Lumen, Dusk. Things don't add up. |
| 3 | **The Insider Thread** | Evidence of inside access. Meet Kestrel, Ferro, Patch, Sable, Crucible. |
| 4 | **The Conspiracy** | Full picture. Wren identified. The Reach revealed. CorpSim's cover-up exposed. |
| 5 | **Confrontation** | NPC hacking unlocks. Clear Dusk. Break Ferro. Overthrow Argon. |
| 6 | **The Reckoning** | Decrypt Wren's confession. Build the evidence chain. File the report. |
| 7 | **The Reply** | Boss fight: Wren. The sequel hook. Crystal Array awaits. |

---

```
> logout
Connection to ssh-hunt.appnest.cc closed.

         ╔════════════════════════════════════╗
         ║  Ghost Rail remembers.             ║
         ║  The Reach is watching.            ║
         ║  Wren is not done.                 ║
         ║                                    ║
         ║  ssh -p 24444 you@ssh-hunt.appnest.cc ║
         ╚════════════════════════════════════╝
```
