# a365 v3.0 progress

This log tracks the staged implementation of milestone 3.0. Public cutover was
approved and completed on 2026-08-26.

## Review stages

The combined milestone is intentionally larger than one review-sized change.
Review it in these coherent dependency stages: contract/setup; source identity,
adult preferences, and migrations; Playback; AniList and Timetable; community
surfaces and TUI; then rename, compatibility, packaging, and release automation.
Each later stage depends on the vocabulary and boundaries established by the
earlier stages.

## Contract and tracking

- Complete: confirm the product contract and domain vocabulary.
- Complete: record ADRs 0008 through 0011.
- Complete: add acceptance criteria to issues #103 and #105 through #110.
- Complete: create milestone issues #111 (AniList account and library) and #112 (Anime365 profile and Moments).
- Complete and released: replace the release PAT with short-lived GitHub App
  tokens (#108).

## Implementation checkpoints

- Complete and released: resumable guided AniList/GitHub App setup wizard.
- Complete and released: source-qualified Anime365/H365 catalogues, migrations,
  independent failure boundaries, and adult/privacy preferences (#106, #107).
- Complete and released: playback-first IINA/mpv flow, credential-free loopback
  proxy, native ASS, and opt-in natural-EOF continuation (#103).
- Complete and released: AniList browser OAuth, secure credential storage,
  read-only standard/custom lists, and personalized local-week Timetable
  (#110, #111).
- Complete and released: isolated documented Anime365 profile and public
  profile/Moments adapters, fail-closed adult filtering, and public Moment
  playback (#112).
- Complete and released: keyboard-and-mouse Home/Search/Timetable/Moments/AniList/
  Profile TUI with independent loading and error states (#105).
- Complete and released: public-facing a365 executable/repository/formula/artifact/
  docs/update rename, automatic state and credential migration, and the v3
  `a365dt` compatibility executable (#109).
- Complete locally: clean development build; primary/compatibility executable
	and machine-output smoke checks; Clippy; and 203 project tests (192 passed,
	11 intentionally ignored process workers).
- Complete locally: formatting, workflow YAML, formula syntax, setup-wizard
  syntax, and diff-integrity verification.
- Complete: register the AniList client and least-privilege GitHub App; limit
  its installation to the source and tap repositories; add its ruleset bypass;
  configure the two public variables and private-key secret; replace the old
  PAT; and mint, inspect, and immediately revoke scoped test installation
  tokens through the verified credentials.
- Complete live: use independently scoped installation tokens to create and
  update a temporary draft source PR and to clone/push a temporary Homebrew tap
  branch. The verifier closed the draft, deleted both branches, and revoked
  both tokens; independent cleanup checks found no open PR or remaining test
  ref. The resumable wizard now includes this reversible write dry run.
- Complete live: AniList implicit browser OAuth for client 49510, Viewer
  validation, macOS Keychain storage, authenticated list rendering, and
  personalized current-week Timetable rendering. AniList and Timetable direct
  routes do not require an Anime365 credential merely to load their own
  surfaces.
- Complete live: diagnose AniList's `unsupported_grant_type` response and keep
  the registered loopback callback authoritative instead of overriding it in
  the authorization request. AniList's implicit response omits OAuth state, so
  a365 now validates a fresh one-time local relay nonce instead; regression
  tests cover the accepted authorization URL, relay, binding, denial, and
  timeout.
- Complete live: validate the public Anime365 Moments host and current feed,
  category, pagination, and `data-sources` rendition contracts through the
  parser-aware `doctor` probe. Media playback accepts only documented public
  page and CDN hosts.
- Complete live: remote repository/formula rename, implementation merge, v3.0
  publication, installation verification, and issue closure.

## Public cutover outcomes

- Merged the reviewed implementation as
  [#115](https://github.com/gridness/a365/pull/115), then renamed the source
  repository to `Gridness/a365`. GitHub redirects the old repository URL; the
  GitHub App installation, public variables, private-key secret, and protected
  `main` ruleset survived the rename.
- Merged the Release Please proposal as
  [#116](https://github.com/gridness/a365/pull/116) and published the non-draft
  [v3.0.0 release](https://github.com/gridness/a365/releases/tag/v3.0.0). Its
  three platform archives contain `a365`, the v3 compatibility `a365dt`
  executable, and `LICENSE`.
- The first publication attempt created the v3.0.0 draft but could not comment
  on the release PR with its App token. The permanent recovery in
  [#117](https://github.com/gridness/a365/pull/117) grants that job issue-write
  permission and retries an unfinished current-version draft. Codecov also
  synchronized the renamed repository before the recovered checks passed.
- The successful
  [release run](https://github.com/gridness/a365/actions/runs/32976679482)
  rebuilt and tested all targets, matched all published SHA-256 checksums,
  attested all three archives, and published the existing draft. Independent
  verification repeated the checksum, attestation, archive-content, version,
  and shell-completion checks against the public assets.
- The same run generated and installed the public
  [`a365` formula](https://github.com/gridness/homebrew-oosama/blob/main/Formula/a365.rb),
  exercised both installed executables, removed `Formula/a365dt.rb`, and pushed
  tap commit
  [`cfe6fa2`](https://github.com/gridness/homebrew-oosama/commit/cfe6fa2f30bffed9e056f6b1a118b9a565309465).
- Closed #103 and #105 through #112 as completed after both publication gates
  passed, leaving no open issues and closing the 3.0 milestone.

## Acceptance evidence

| Issue | Local evidence |
| --- | --- |
| #103 | Playback/default-download routing, IINA/mpv adapters, loopback capability proxy whose owned tasks terminate together, native ASS arguments, EOF/error/stop classification, continuation coverage, and Player shutdown IPC tests. |
| #105 | Pure keyboard/mouse/resize/loading/error/cancellation transitions, direct-destination routing, independently loaded optional surfaces, and directly tested RAII terminal entry/cleanup around normal, error, and signal-cancelled handoff. |
| #106 | Source-qualified API/cache/telemetry models and migrations, H365 catalogue/Series/Episode/Translation/embed/failure fixtures, ID-collision tests, per-source freshness, stale-row hiding, and visible source-failure isolation in the TUI. |
| #107 | Complete preference round trips, opt-in success/denial/transient decisions, runtime H365 isolation, fail-closed AniList/Timetable/Moment filters, and adult telemetry redaction. |
| #108 | Four independently minted, single-repository least-privilege job tokens plus the completed reversible source-PR and cross-repository tap-push dry run; the built-in token is read-only and no release PAT remains. |
| #109 | Primary and compatibility binaries, machine-output suppression, old/new/conflict/interrupted/repeated migration cases, renamed package/workflow/formula surfaces, and the verified public cutover. |
| #110 | Fixed-timezone and DST boundaries, exact-week query bounds, authenticated/public GraphQL fixtures, every personalization group, adult filtering, MAL/AniList mapping, in-session TUI caching, and lazy Anime365 access only after a content selection. |
| #111 | Live browser OAuth and authenticated rendering, local nonce relay, browser-denial cancellation, loopback timeout, injected credential stores, read-only mutation refusal, whole list models, filtering, rich entry metadata, and source mapping. |
| #112 | Sanitized current-contract feed/profile fixtures, five profile statuses, explicit one-based pagination, numeric categories, missing fields, fail-closed adult filtering, changed-markup isolation plus a live parser-aware `doctor` probe, trusted-host highest-rendition selection, and ordinary public Player routing without Episode continuation. |

## Public cutover runbook

This sequence was explicitly approved on 2026-08-26. Record each verified
outcome before proceeding to the next boundary:

1. Commit the reviewed local milestone on a non-`main` branch with a
   Conventional Commit breaking-change marker (for example,
   `feat!: deliver a365 v3`), push that branch, and open the implementation PR
   without closing milestone issues. This is what makes Release Please propose
   3.0.0 rather than 2.5.0.
2. Rename `Gridness/a365dt` to `Gridness/a365`; confirm the GitHub App remains
   installed and the `main read-only` ruleset still lists it as a bypass actor.
3. Merge the verified implementation PR. The `main` release workflow will mint
   fresh job-scoped App tokens, create and validate the Release Please PR, and
   publish only after that release commit reaches `main`.
4. Verify all three `a365-v3.0.0-*` archives and checksums, the `a365` and
   compatibility `a365dt` executables, and the GitHub release.
5. Verify `Gridness/homebrew-oosama` contains `Formula/a365.rb`, no longer
   contains `Formula/a365dt.rb`, and installs both executables successfully.
6. Close #103 and #105–#112 only after the release and Homebrew checks pass.
   Keep repository redirects and the compatibility executable through the
   documented version 3 window.
