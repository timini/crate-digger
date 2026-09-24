# Milestone 3: end-to-end discovery

The scope and order follow `docs/handover.md`. Core remains offline. Network
adapters and credential access live in `crates/connectors`. Routine tests use
fake transports and credential stores. Live checks are explicit.

## Steps and commits

1. Connections and credentials (#15): native keychain, non-secret settings,
   connection tests, skippable onboarding, seeds and resource limits.
2. LLM adapters (#10): OpenAI-compatible and Anthropic protocols, validated
   structured output, bounded app-owned tools and hostile-input tests. Test
   the installed Ollama model locally; cloud providers use fakes.
3. Cultural discovery (#11): Discogs relationships, bounded public pages with
   robots rules, pasted text, stored evidence, feedback seeds and refresh.
4. YouTube references (#16): Data API search, oEmbed verification, alternatives,
   confidence and durable user corrections.
5. Soulseek (#12): pinned and checksum-verified managed slskd, lifecycle and
   keychain credentials, external-instance option, durable transfers, explicit
   ambiguous-result selection and calibrated automatic matching.
6. Ranking (#13): version-compatible taste clusters, dislike penalty, evidence,
   diversity, cold start and 20% exploration. Report synthetic calibration and
   a 1,000-candidate timing measurement without claiming real-world quality.
7. Replenishment (#14): bounded top-up, pending-work accounting, scheduler
   budgets, review hold reasons and buffer availability.
8. Acceptance: local full checks, fake-service end-to-end recovery tests and
   explicit live checks. Record unavailable live services as outstanding.

## Verification requirements

Each step has focused tests for failure and recovery as well as success.
`scripts/check.sh` must pass before a commit. Use `CARGO_INCREMENTAL=0` and
check free disk space before compiling. Each step receives its own commit.

No credentials enter SQLite, logs, test fixtures or committed reports. The user
enters service credentials in the app. LLM suggestions without independently
retrieved evidence cannot authorize acquisition. Retrieved text cannot select
arbitrary tools, URLs, commands or files. User corrections survive refreshes.

## Acceptance evidence

Implementation and measurements are pending. This table will record actual
checks as they complete; the plan itself is not acceptance evidence.

| Area | Required evidence | Status |
| --- | --- | --- |
| Connections | Keychain errors, secret isolation, skippable UI | Done: `cd-connectors` probe and credential tests (missing credential stops before any request, each service gets only its own secret, rejected credentials are not echoed); `Connections.test.ts` (secret only sent to the keychain command, seeds work without connections). Resource limits stay in Settings, not onboarding. |
| LLM | Both protocols, schema rejection, prompt injection, live Ollama | Done: `cd-connectors` `llm` tests (request and reply shapes for both protocols with fakes, one retry then failure on invalid replies, unknown tools and out-of-bounds arguments never run, injected text cannot close its wrapper or change the prompt or tool set, last turn may only finish). Live on Ollama 0.34.2 with qwen2.5-coder 7B (`tests/live_llm.rs`, ignored in CI): structured check passes; with a hostile page the agent used only its tool, but copied the injected artist into its answer. Model output is therefore never trusted on its own; step 3 requires independent evidence before a suggestion is verified. |
| Discovery | Discogs, robots, paste, evidence, refresh, feedback seeds | Done. Live Discogs on 2026-09-23 with the user's token (`tests/live_discogs.rs`, ignored in CI): seeds Kerri Chandler (artist) and Underground Resistance (label) gave 20 candidates in 4.6 s using 12 of 40 requests, each with its Discogs release as evidence. The run found one wording bug (an artist's self-named label), since fixed. `cd-core` `discovery_tests.rs`: model-only suggestions stay unverified and are refused by acquisition; later evidence verifies and queues an existing suggestion; a failed retrieval is recorded apart from an empty run; pasted text and pages reach the source; positive ratings add seeds; the 6-hour refresh is due only when no run is waiting; verified candidates wait with a visible reason while no download source exists. `cd-connectors` `discovery::tests`: artist to label expansion, request budget, rejected token, no near-match substitution, tracklist parsing, robots rules, public-address checks including redirects, HTML to text, model extraction kept only when grounded, model suggestions verified only when app code finds the track on the cited Discogs release. `DiscoverFrom.test.ts`: failed and empty runs shown differently. Live: Wikipedia page read after a robots check; Ollama extraction from prose found the four named tracks with short excerpts. Evidence is shown on the review card (existing). Open: no screen yet lists unverified suggestions or lets the user acquire one by hand. |
| YouTube | Verified references, alternatives, correction precedence | Done with fakes; live Data API search outstanding until the user enters a key. `cd-core` `youtube_tests.rs`: only a match at 0.7 or above becomes preferred, weaker ones stay alternatives with the track unresolved; user-chosen, user-supplied and rejected links survive later lookups; a lookup without a key waits (status and paused job) and resumes when the connection changes. `cd-connectors` `youtube::tests`: link and duration parsing, scoring (right mix, Topic channels, live, remix and long-set penalties), only videos confirmed by oEmbed are kept, quota exhaustion is retried rather than treated as a bad key, user links must exist. `YoutubeLinks.test.ts`: status shown, choose, reject and replace. Live: oEmbed confirmed a real video and refused a made-up ID. Lookups run as their own job kind with a daily cap of 80, so missing YouTube data never blocks review. |
| Acquisition | Version ambiguity, quality, restart, managed process lifecycle | Done except a live download, which waits for the user's Soulseek login. Matching rule and calibration: `docs/acquisition-calibration.md` (25 labelled cases, 0 wrong automatic downloads, build fails on any). `cd-core` `acquisition::tests`: with unattended off even a clear match is offered as a recommended choice; unattended picks the best acceptable copy; ambiguous lengths always ask; declining fails the candidate without a rating; nothing usable fails with a reason; a remotely queued transfer hands its worker back without using an attempt and resumes the same transfer; a failed source is not tried again. `jobs::tests::waiting_reschedules_without_using_attempts`. `cd-connectors` `slskd::tests`: search mapping (locked and non-audio files dropped, peer details kept), fallback search without the mix, enqueue reuses a live transfer and URL-encodes user names, transfer states, finished file found by name and size, rejected key; `install::tests`: pinned checksums, verified install replaces the old one, wrong checksum and unsafe archive paths refused. UI: `DownloadChoices.test.ts`. Live on this Mac (`tests/live_slskd.rs`): the real 0.26.0 release downloaded, matched its pin, started on a random loopback port, answered with the generated key, refused a wrong key, and stopped with no process left; slskd's log shows no secrets. Deliberate change from the plan: the model gets a Soulseek search tool (at most three searches) but no enqueue or status tools. It can widen a search that found nothing usable; the matching rule and the user still decide what downloads, and app code runs transfers. |
| Ranking | Version guard, cold start, diversity, exploration, timing | Done on synthetic listeners; real-rating calibration outstanding. `docs/ranking-calibration.md`: weights chosen on five synthetic training worlds and reported on five held-out worlds (precision at 20: 0.85 against 0.82 for evidence alone and 0.81 for one average vector; the smaller liked style keeps about half the top 20 against 8% with one average vector). Full database rerank of 1,000 candidates against 300 ratings at 1,280 dimensions: 0.19 s (release, Apple M4). `cd-core` `ranking::tests`: separate tastes stay separate, dislikes suppress similar sounds and not artists, cold start ranks by evidence and seeds, every fifth position is exploration when available and none during cold start, no back-to-back artist when avoidable, other analysis versions never mix, ratings reorder the queue and undo restores the previous scores exactly. Ratings also feed seed expansion (step 3). |
| Replenishment | Limits, recovery, hold reasons, ready buffer | Done. `cd-core` `replenish::tests`: top-up only below the threshold and counting work in progress, one discovery run at a time, distinct hold codes (sign-in, outage, daily and storage limits, failed or empty discovery, choices waiting), availability as a share of samples. Limits are enforced by the existing scheduler (daily, storage and concurrency tests from milestone 1). The app samples the queue each minute, tops it up when automatic discovery is on, and shows holds and 7-day availability in Review (`QueueHealth.test.ts`). |
| End to end | Fake services in CI and separately reported live checks | Partly done. In CI with fakes: discovery through acquisition, validation and analysis to ready (`review::tests::discovery_to_ready_through_jobs`), plus each step's tests above. Live on this Mac: Ollama structured output, prompt injection and extraction; a public page read after robots.txt; YouTube oEmbed; slskd installed from the pinned release, started and stopped. Outstanding, waiting for the user's credentials entered in the app: Discogs discovery, YouTube search, a Soulseek sign-in and download, and a full live run from seeds to a ready track. |
