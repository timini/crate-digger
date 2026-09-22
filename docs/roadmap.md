# Crate Digger — Epics and Roadmap

This document breaks the [product specification](product-spec.md) into epics and ranks them. Each epic is tracked as a GitHub issue labelled `epic`. The kanban board is [issue #23](https://github.com/timini/crate-digger/issues/23), where the epics are attached as sub-issues in priority order.

## Board conventions

| Label | Meaning |
| --- | --- |
| `status: now` | In progress. Keep this column to 2–3 epics. |
| `status: next` | Ready to start once a Now slot frees up. |
| `status: later` | Prioritised backlog. |
| Closed issue | Done. |
| `P0`–`P3` | Priority tier (below). |
| `milestone N: …` | Delivery milestone from the [implementation plan](implementation-plan.md). |

To move a card, swap its `status:` label. Update the rank table below whenever the priority order changes.

## Priority tiers

- **P0: Offline walking skeleton.** A DJ can import, play, rate and playlist music with no network. Every other epic builds on this, and it proves the core review UX with mocked discovery.
- **P1: Core promise.** The discovery loop keeps a fresh, ready-to-review queue that adapts to ratings. This is the product's differentiator.
- **P2: Needed for the v1 release.** YouTube references, export, the shared catalogue and packaging.
- **P3: v1 scope, last in line.** Private backup and the pilot evaluation.

## Ranked epics

| Rank | Epic | Priority | Status | Milestone | Depends on |
| ---: | --- | :---: | --- | :---: | --- |
| 1 | [#1 Foundation: Rust core, Tauri shell, SQLite & domain model](https://github.com/timini/crate-digger/issues/1) | P0 | Now | 1 | — |
| 2 | [#2 Durable job system & background workers](https://github.com/timini/crate-digger/issues/2) | P0 | Now | 1 | #1 |
| 3 | [#3 Library import & management](https://github.com/timini/crate-digger/issues/3) | P0 | Next | 1 | #1 |
| 4 | [#4 Audio playback](https://github.com/timini/crate-digger/issues/4) | P0 | Next | 1 | #1 |
| 5 | [#5 Discovery review queue & ratings](https://github.com/timini/crate-digger/issues/5) | P0 | Next | 1 | #1, #4 |
| 6 | [#6 Playlists](https://github.com/timini/crate-digger/issues/6) | P0 | Next | 1 | #1, #3 |
| 7 | [#7 Staging & archive management](https://github.com/timini/crate-digger/issues/7) | P1 | Next | 1 | #1, #2 |
| 8 | [#8 Canonical track identity & version matching](https://github.com/timini/crate-digger/issues/8) | P1 | Next | 2 | #1 |
| 9 | [#9 Audio analysis worker & model selection](https://github.com/timini/crate-digger/issues/9) | P1 | Later | 2 | #2, #8 |
| 10 | [#10 LLM agent adapters (API & local) with safe tool use](https://github.com/timini/crate-digger/issues/10) | P1 | Later | 3 | #1, #2 |
| 11 | [#11 Tier 1 cultural discovery sources](https://github.com/timini/crate-digger/issues/11) | P1 | Later | 3 | #8, #10 |
| 12 | [#12 Soulseek acquisition via slskd](https://github.com/timini/crate-digger/issues/12) | P1 | Later | 3 | #2, #7, #8 |
| 13 | [#13 Tier 2 personalised ranking](https://github.com/timini/crate-digger/issues/13) | P1 | Later | 3 | #5, #9, #11 |
| 14 | [#14 Ready-queue replenishment & resource budgets](https://github.com/timini/crate-digger/issues/14) | P1 | Later | 3 | #2, #9, #11, #12 |
| 15 | [#15 Onboarding, settings & credentials](https://github.com/timini/crate-digger/issues/15) | P1 | Later | 1→3 | #1 |
| 16 | [#16 YouTube reference links](https://github.com/timini/crate-digger/issues/16) | P2 | Later | 3 | #8, #10 |
| 17 | [#20 Playlist export: M3U8 & Rekordbox XML](https://github.com/timini/crate-digger/issues/20) | P2 | Later | 5 | #6 |
| 18 | [#17 Central metadata service (PostgreSQL API)](https://github.com/timini/crate-digger/issues/17) | P2 | Later | 4 | #8, #9 |
| 19 | [#18 Sharing & sync client](https://github.com/timini/crate-digger/issues/18) | P2 | Later | 4 | #2, #9, #17 |
| 20 | [#21 Cross-platform packaging & release validation](https://github.com/timini/crate-digger/issues/21) | P2 | Later | 5 | most epics |
| 21 | [#19 Private backup & restore](https://github.com/timini/crate-digger/issues/19) | P3 | Later | 4 | #17 |
| 22 | [#22 DJ pilot & recommendation evaluation](https://github.com/timini/crate-digger/issues/22) | P3 | Later | 5 | #13, #14 |

## Why this order

1. **Foundation and the job system come first.** Stable IDs, migrations, correction precedence and a crash-safe job engine are the hardest things to add later. Every background feature depends on them.
2. **The local review loop comes before any integration.** Library, playback, rating and playlists work offline with mocked candidates, so the spec's "works without the central service" guarantee holds from day one. The rating UX can also be tested with real DJs early.
3. **Identity is pulled forward to Next.** Version matching (original vs remix vs edit) gates unattended acquisition, feature reuse and sharing. Its labelled fixture set is needed to calibrate #12 and #18.
4. **Discovery comes before extras.** Analysis, LLM adapters, Tier 1 sources, acquisition, ranking and replenishment together deliver the core promise: "a fresh, relevant queue ready to hear".
5. **YouTube links are P2.** The spec treats them as references only. Missing links never block review, so they add polish rather than core value.
6. **Export has a lower rank but few dependencies.** It needs only playlists (#6), so it can be pulled forward whenever someone has capacity. It is the bridge into Rekordbox.
7. **Central service, sharing and backup come late.** Local use never depends on them, and they carry release gates: pinned analysis versions before shared uploads, and account isolation before private backup.
8. **Packaging and the pilot come last.** They validate everything else.

## Release gates

These spec §6 gates are called out as blocking checks inside the relevant epics:

- Local core and recovery checks pass → #1, #2, #7
- Validated matching before unattended acquisition → #8, #12
- Pinned analysis versions before shared uploads → #9, #18
- Account isolation before private backup → #17, #19
- Tested exports and recorded target-platform compatibility → #20, #21
