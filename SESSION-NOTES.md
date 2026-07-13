# Session Handoff — pub-rocket-reader

**Read this first in any new session working on this app.** It captures what's done, what's mid-flight, and exactly what to do next — written so a new Claude session (or a human) can pick up without re-deriving the last several hours of decisions.

---

## Architecture recap

- **Tauri v2** app. Rust backend in `src-tauri/src/`, TypeScript frontend in `ui/src/main.ts` + `ui/index.html`.
- **SQLite is the source of truth** (`db.rs`). Every report shown in the UI is rendered *fresh* from structured tables on every request (`db::get_report_cmd`) — nothing is cached as a file that can silently go stale. This was a deliberate fix after a `genre-ranking.json`/`.md` drift bug early in the project.
- **Publisher Rocket automation** via raw CDP WebSocket (`cdp.rs`). No official API — everything is DOM scraping through `Runtime.evaluate` + synthetic mouse/keyboard events.
- Kiro (a separate AI tool David also uses) makes concurrent changes between sessions — check for drift.

---

## What's fully built and working

1. **Full DB migration** — chapter summaries, genre classification, KDP keywords, PR keywords, category results, BISAC — all DB-backed. No more loose files that can drift.
2. **WinningCat CSV importer** (`winningcat.rs`) — populates `kdp_categories` with real Amazon node IDs for Books/Kindle Store. Staleness detection via `last_seen_at`; explicit (never automatic) cleanup of entries missing from a re-import.
3. **Match Categories** — genre-by-genre catalog matching (not pooled), so one dominant genre can't crowd out a weaker one's real fit.
4. **BISAC Classification** — ebook + print, separately scored, specificity-preference heuristic for close calls (no live data source exists for BISAC — flagged honestly every time).
5. **"Best 3 for discoverability" method for KDP categories** — the real method, in `match_categories_by_store` + `rank_by_discoverability` + `compute_discoverability_score`:
   - **Fit gate first**: confidence ≥55%, or independently found by 2+ ranked genres. Non-negotiable — a highly discoverable category that doesn't actually fit is disqualified regardless of numbers.
   - **Every fit-qualified candidate** (capped at 8/store) gets checked live in Publisher Rocket — not just a narrow tiebreak.
   - **Ranked by real discoverability score** (sweet spot 5–30 sales/day scores highest; unverified always loses regardless of fit).
6. **`find_genres_and_categories_for_story`** — current combined command: Rank Genres → KDP Categories (both stores, discoverability method) → BISAC (ebook + print) → Positioning Context → one report (doc_type `genres_and_categories`).
7. **Bug fix**: `commands.rs`'s `cdp:log` event channel was never wired to the frontend — only `genre:log` was. Category Analyzer's DIAG output was going nowhere. Fixed — both channels now feed the Log tab in `main.ts`.
8. **`keyword_search.rs`** (NEW, UNTESTED) — first-pass CDP automation of Publisher Rocket's real **Keyword Search** tool (actual search volume, competition score, estimated AMS earnings per keyword). This is the single biggest capability gap this app had — KDP keywords were always AI-guessed, never measured.

---

## Publisher Rocket's real feature set (confirmed via web research this session)

Four core tools: **Keyword Search** (real search volume/competition/earnings), **Category Search** (fully automated already), **Competitor Analyzer** (partially automated), **AMS Keywords** (not automated). Plus **Reverse ASIN** (feed a competitor's ASIN, get every keyword *that book* ranks for — not built yet, high value, not started) and **historic category trend data** (not built, not started).

**Confirmed: PR is Amazon-only.** No Kobo, B&N, Apple Books, etc. This is a hard limit of the tool, not a gap in this app's implementation — it's why the "non-KDP" track has to be AI-reasoned and clearly labeled as such, same honesty posture as BISAC.

---

## UI design principle established this session (apply to all future button decisions)

A function gets its own separate button **only if**:
- (a) it needs information from the user that has **no sensible default**, or
- (b) it has an unusually different **cost/fragility** compared to everything else (e.g., a multi-minute CSV export flow vs. a fast AI call).

Otherwise it's a step inside the main pipeline, not a button. This is how "10 buttons" got argued down to 2.

---

## FINAL DECIDED SCOPE — NOT YET IMPLEMENTED IN CODE

This is the actual next work. Everything below was agreed in conversation but the code has not been written yet except where noted.

### Target: 2 buttons total
1. **Analyze** (+ a "force re-summarize" checkbox) — one click, does everything:
   - If checkbox checked: `db::delete_chapter_summaries` first (already built), then regenerate all chapters.
   - Chapter summaries (skip already-done unless forced — `chapter_summary_exists` check already handles this).
   - `phase2_analyze` → `genre_data`.
   - Rank Genres.
   - KDP Categories — best-3-discoverability method (already built), auto-falls-back to Find Categories (PR) discovery internally if the catalog is empty for a store (no separate button, no user action).
   - BISAC (ebook + print, already built).
   - **NEW — not yet written:** auto-derived Keyword Search. No manual typing from the user, ever — David was explicit about this. Seeds are pulled from data the app already has:
     - `derive_keyword_seeds()` — **not written yet.** Should pull 2–3 short seed terms: the genre tag (`genre_data.industry_ebook`, trimmed to ~3 words) + the leaf segment of the top 1–2 discoverability-ranked KDP categories.
     - Call `keyword_search::search_keyword()` (already built, untested) for each seed via `spawn_blocking`, aggregate into a deduped pool, persist via `db::replace_keyword_search_results` (already built).
     - **NEW — not yet written:** `call_keyword_optimizer_with_real_data()` — upgraded version of `call_keyword_optimizer` that takes the real PR pool + `genre_data`, and **prefers real search-volume-backed keywords over AI guesses** for the 7 KDP slots. Falls back to pure reasoning only for slots the real pool doesn't cover well. Each keyword's rationale should note whether it came from real PR data or reasoning.
   - **NEW — not yet written:** non-KDP discovery keywords (10 phrases for Apple Books/Kobo/Google Play/B&N/BookBub/Goodreads/general web). The DB layer exists (`db::DiscoveryKeywordEntry`, `save_discovery_keywords`, `load_discovery_keywords` — all built), but the actual AI-generation function was never written. Must be clearly labeled as AI-reasoned, not measured (PR has zero coverage of these platforms).
   - One combined report, all sections together.
2. **Analyze Competition** — unchanged, stays exactly as-is. Restored after initially (wrongly) dropping it — it's a genuinely different question (competitors) with genuinely different cost (multi-minute CSV flow), so it earns its own button under the stated principle.

### Everything else gets removed from the UI
Match Categories, Find Categories (PR), Verify Mapped Categories, Rank Genres (standalone), KDP Keywords (standalone), PR Keywords (standalone), Summaries (standalone), Full Analysis, Run Analysis, and the **Keyword Search test-input row** (David rejected manual keyword entry — remove this UI entirely, keep only the underlying automation). Backend Rust commands can stay registered (harmless) or be cleaned up later — not urgent either way.

### Concrete build steps for next session, in order
1. Write `derive_keyword_seeds()`.
2. Write the async seed-search-and-aggregate wrapper (pattern-match `rank_by_discoverability`'s `spawn_blocking` usage).
3. Write `call_keyword_optimizer_with_real_data()`.
4. Write the non-KDP discovery-keywords generator function.
5. Rename/rebuild `find_genres_and_categories_for_story` into the final combined command (new name suggestion: `run_analysis` or `analyze_story`) — add `force_resummarize: bool` to its request struct, wire in steps 1–4, restructure the report to add KDP Keywords + Non-KDP Keywords sections.
6. Register new command in `lib.rs`.
7. Full rewrite of `index.html`'s analyzer panel: strip to 2 buttons + 1 checkbox.
8. Full rewrite of the relevant `main.ts` sections: remove dead button handlers and interfaces, wire the new Analyze button (include `force_resummarize` in the request payload), update `AnalysisState`/`check_analysis_state` to match the minimal button set.

---

## ⚠️ Known risk — read before wiring Keyword Search into the mandatory pipeline

`keyword_search.rs` has **never been live-tested** against the real Publisher Rocket app. Tab navigation, the search-trigger button, and the results-table column detection are all first-pass guesses from documented PR behavior, not DOM inspection — same starting point Category Search had before it needed the search-icon-click fix and the virtualized-table fix.

**Recommendation:** test `keyword_search.rs` in isolation first (temporarily re-add a manual trigger, or invoke it directly) *before* wiring it into the mandatory Analyze pipeline, since once it's in the main pipeline a scraping bug there will degrade or fail *every* Analyze run. At minimum, wrap the seed-search step in defensive error handling so a Keyword Search failure falls back gracefully to pure AI reasoning for the KDP keywords, rather than failing the whole run — consistent with how `ai_pick_bisac`/`call_keyword_optimizer` already fail gracefully elsewhere.

---

## Key files touched this session
`db.rs` (major), `genre_analyzer.rs` (major), `winningcat.rs` (new), `keyword_search.rs` (new, untested), `commands.rs` (struct/bug fix), `competition_analyzer.rs` (DB touch), `lib.rs` (registrations), `index.html` + `main.ts` (many iterations, mid-cutover), `tauri.conf.json` (path bug fix), `data/bisac-fiction.json` (new seed data).

## Principles worth carrying forward
- Honest fit first, discoverability second — never the reverse, and never blend them into one opaque score.
- Where no live data source exists, say so plainly in the report; use defensible structural heuristics, never present reasoning as measurement.
- PR gets touched only where it's genuinely irreplaceable (live competitive/search data) — everything else runs against the database first.
- Never regenerate something already done unless explicitly forced by the user.
- Separate button only if: needs info with no default, or meaningfully different cost/fragility.
