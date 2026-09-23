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
| Discovery | Discogs, robots, paste, evidence, refresh, feedback seeds | Done with fakes; live Discogs outstanding until the user enters a token. `cd-core` `discovery_tests.rs`: model-only suggestions stay unverified and are refused by acquisition; later evidence verifies and queues an existing suggestion; a failed retrieval is recorded apart from an empty run; pasted text and pages reach the source; positive ratings add seeds; the 6-hour refresh is due only when no run is waiting; verified candidates wait with a visible reason while no download source exists. `cd-connectors` `discovery::tests`: artist to label expansion, request budget, rejected token, no near-match substitution, tracklist parsing, robots rules, public-address checks including redirects, HTML to text, model extraction kept only when grounded, model suggestions verified only when app code finds the track on the cited Discogs release. `DiscoverFrom.test.ts`: failed and empty runs shown differently. Live: Wikipedia page read after a robots check; Ollama extraction from prose found the four named tracks with short excerpts. Evidence is shown on the review card (existing). Open: no screen yet lists unverified suggestions or lets the user acquire one by hand. |
| YouTube | Verified references, alternatives, correction precedence | Done with fakes; live Data API search outstanding until the user enters a key. `cd-core` `youtube_tests.rs`: only a match at 0.7 or above becomes preferred, weaker ones stay alternatives with the track unresolved; user-chosen, user-supplied and rejected links survive later lookups; a lookup without a key waits (status and paused job) and resumes when the connection changes. `cd-connectors` `youtube::tests`: link and duration parsing, scoring (right mix, Topic channels, live, remix and long-set penalties), only videos confirmed by oEmbed are kept, quota exhaustion is retried rather than treated as a bad key, user links must exist. `YoutubeLinks.test.ts`: status shown, choose, reject and replace. Live: oEmbed confirmed a real video and refused a made-up ID. Lookups run as their own job kind with a daily cap of 80, so missing YouTube data never blocks review. |
| Acquisition | Version ambiguity, quality, restart, managed process lifecycle | Pending |
| Ranking | Version guard, cold start, diversity, exploration, timing | Pending |
| Replenishment | Limits, recovery, hold reasons, ready buffer | Pending |
| End to end | Fake services in CI and separately reported live checks | Pending |
