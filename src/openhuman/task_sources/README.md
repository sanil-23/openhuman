# task_sources

Proactive ingestion of work items from external tools. A **task source** is a user-configured pull from a Composio-backed provider (GitHub, Notion, Linear, ClickUp) with a per-provider filter. A periodic poll fetches matching items through the providers' `fetch_tasks` surface; a fetch → dedup → enrich → route pipeline drops a todo card onto the dedicated `task-sources` thread board and, for proactive sources, dispatches a triage turn so an agent can start working immediately. The domain mirrors the `cron` layering: `mod.rs` is export-only, business logic lives in sibling modules, persistence is SQLite, and the RPC surface is wired through `schemas.rs`.

## Responsibilities

- Persist per-source configs (provider + filter + schedule + routing target + optional pinned connection / static executor).
- Periodically poll enabled sources (`periodic.rs`) on a global 10-minute tick honoring per-source `interval_secs` (floored to 60s).
- Translate a typed `FilterSpec` into the provider-agnostic `TaskFetchFilter` (`filter.rs`) and fetch via the registered Composio provider.
- Dedup per source so a source never extracts a card for an item it already has: (1) the per-source ledger skips an unchanged re-fetch via an edit-aware SHA-256 content hash (`store.rs` + `pipeline.rs`); (2) routing scans **this source's own** board cards by task identity and skips/refreshes accordingly (`dedup.rs` + `route.rs`). Dedup is deliberately *not* cross-source — two different sources that both match the same upstream item each keep their own card.
- Deterministically enrich raw tasks into agent-ready ones — urgency heuristic, summary, linked assignee, templated agent prompt (`enrich.rs`).
- Route enriched tasks onto the `task-sources` thread board as todo cards (skipping silently when this source already has a card for the item) and, for proactive sources, dispatch a triage turn through the same path Composio webhooks use (`route.rs`).
- Fire a one-shot fetch when a matching Composio connection is created (`bus.rs`).
- Expose an `openhuman.task_sources_*` RPC surface for CRUD, manual fetch, filter preview, ingested-task listing, and status (`schemas.rs` + `ops.rs`).

## Key files

| File | Role |
| --- | --- |
| `src/openhuman/task_sources/mod.rs` | Export-only: module docstring, `mod`/`pub mod` decls, `pub use` re-exports, and the `all_task_sources_*` controller registry pair. |
| `src/openhuman/task_sources/types.rs` | Serde domain types: `ProviderSlug`, `FilterSpec` (provider-tagged enum), `SourceTarget`, `FetchReason`, `TaskSource`, `TaskSourcePatch`, `EnrichedTask`, `FetchOutcome`. |
| `src/openhuman/task_sources/store.rs` | SQLite persistence (`<workspace>/task_sources/sources.db`): `task_sources` + `ingested_tasks` tables, dedup `content_hash`, card-id ledger, migrate-on-open. |
| `src/openhuman/task_sources/ops.rs` | RPC-facing business logic returning `RpcOutcome<T>`: `list`/`get`/`add`/`update`/`remove`/`fetch`/`list_tasks`/`preview_filter`/`status`. |
| `src/openhuman/task_sources/schemas.rs` | `task_sources` controller schemas + `all_controller_schemas` / `all_registered_controllers` + thin `handle_*` param parsers delegating to `ops.rs`. |
| `src/openhuman/task_sources/pipeline.rs` | `run_source_once` — the infallible fetch → dedup → enrich → route pass shared by poll, manual RPC, and connection hook; publishes domain events. |
| `src/openhuman/task_sources/filter.rs` | `to_fetch_filter` — flattens a `FilterSpec` variant into the shared `TaskFetchFilter`. |
| `src/openhuman/task_sources/enrich.rs` | Deterministic, dependency-free `enrich_task`: urgency heuristic, summary, linked assignee, agent prompt. No LLM call. |
| `src/openhuman/task_sources/dedup.rs` | Pure task identity (`provider:external_id`, url fallback), the board-card identity match, and the "replaceable only while untouched `Todo`" rule. No I/O — unit-tested in isolation. |
| `src/openhuman/task_sources/route.rs` | `route_enriched` (→ `RouteOutcome::Routed`/`Deduped`) / `create_card` / `remove_source_cards` / `board_cards` — appends todo cards to the `task-sources` board (`TASK_SOURCES_THREAD_ID`), does per-source status-aware dedup (scoped via `source_metadata.source_id`), and dispatches a scheduler-gated triage turn for proactive sources. |
| `src/openhuman/task_sources/periodic.rs` | `start_periodic_poll` — global tick scheduler; per-source due-timing in a process-global map; `run_one_tick` is `pub(crate)` for tests. |
| `src/openhuman/task_sources/bus.rs` | `TaskSourcesConnectionSubscriber` + `register_task_sources_subscriber` — one-shot fetch on `ComposioConnectionCreated`. |
| `src/openhuman/task_sources/store_tests.rs` | Sibling test suite for `store.rs`. |
| `src/openhuman/task_sources/pipeline_tests.rs` | Sibling test suite for `pipeline.rs`. |

## Public surface

Re-exported from `mod.rs`:

- Types: `TaskSource`, `TaskSourcePatch`, `FilterSpec`, `ProviderSlug`, `SourceTarget`, `FetchReason`, `EnrichedTask`, `FetchOutcome`; plus `NormalizedTask` / `TaskFetchFilter` re-exported from the composio providers.
- Functions: `start_periodic_poll`, `run_source_once`.
- Constant: `TASK_SOURCES_THREAD_ID` (`"task-sources"`).
- RPC registry: `all_task_sources_controller_schemas`, `all_task_sources_registered_controllers`, `task_sources_schemas`.

## RPC / controllers

Namespace `task_sources` (methods `openhuman.task_sources_<function>`):

| Function | Description |
| --- | --- |
| `list` | List all configured sources. |
| `get` | Fetch one source by id. |
| `add` | Create a source; missing schedule/target/cap fall back to `[task_sources]` config defaults. |
| `update` | Apply a partial `TaskSourcePatch`. |
| `remove` | Delete a source by id (cascades `ingested_tasks`); also sweeps the source's still-untouched (`Todo`, unclaimed) board cards. Progressed/claimed cards are preserved. |
| `fetch` | Fetch one source now (`FetchReason::Manual`) and route new tasks. |
| `list_tasks` | List recently ingested tasks for a source (newest first, default limit 50). |
| `preview_filter` | Dry-run a filter — fetch matching tasks WITHOUT routing/recording. |
| `status` | Domain master switch + default interval + source counts. |

Handlers parse params and delegate to `ops.rs`; schemas reference `FilterSpec`, `TaskSource`, `TaskSourcePatch`, `FetchOutcome`, `NormalizedTask`. Registered into the global registry via `src/core/all.rs`.

## Agent tools

None. This domain owns no `tools.rs` / agent tools. It *produces* work for agents (todo cards + triage turns) rather than exposing callable tools.

## Events

Publishes (via `publish_global`, domain `"task_sources"`):

- `DomainEvent::TaskSourceFetched` — after a successful fetch pass (counts: fetched/routed/skipped).
- `DomainEvent::TaskSourceTaskIngested` — per newly routed task (provider, external_id, title, urgency).
- `DomainEvent::TaskSourceFetchFailed` — on a failed pass (error string).

Subscribes:

- `DomainEvent::ComposioConnectionCreated` (domain filter `["composio"]`) via `TaskSourcesConnectionSubscriber` — fires a one-shot `ConnectionCreated` fetch for matching enabled sources. Registered once at startup (`register_task_sources_subscriber`, idempotent `OnceLock` handle).

Startup wiring lives in `src/core/jsonrpc.rs` (registers the subscriber and starts the periodic poll).

## Persistence

SQLite at `<workspace_dir>/task_sources/sources.db` (WAL, 5s busy timeout, migrate-on-open):

- **`task_sources`** — configured sources: provider, optional connection_id/name, enabled, filter JSON, interval_secs, target, max_tasks_per_fetch, created_at, last_fetch_at/last_status, assigned_executor.
- **`ingested_tasks`** — per-(source, external_id) dedup ledger: edit-aware `content_hash` (SHA-256 over title/body/status/updated_at/url), normalized task `payload`, `ingested_at`, and `card_id` (board card UUID) so an edited upstream item removes its stale card before re-routing. FK to `task_sources` with `ON DELETE CASCADE`.

Additive idempotent column migrations (`add_column_if_missing`) backfill `ingested_tasks.card_id` and `task_sources.assigned_executor` on older DBs. App-level defaults (enabled flag, default interval, per-fetch cap, auto_proactive) live in config (`TaskSourcesConfig`), not the store.

## Dependencies

- `crate::core::all` — `ControllerFuture`, `RegisteredController` for the RPC registry.
- `crate::core::event_bus` — `publish_global`, `subscribe_global`, `DomainEvent`, `EventHandler`, `SubscriptionHandle` for event publish/subscribe.
- `crate::openhuman::config` (+ `config::rpc`) — `Config`, `load_config_with_timeout`; reads the `[task_sources]` block for defaults and the master switch.
- `crate::openhuman::memory_sync::composio::providers` — `get_provider`, `ProviderContext`, `NormalizedTask`, `TaskFetchFilter`, `ComposioProvider::fetch_tasks`; the actual external fetch + normalized task shape.
- `crate::openhuman::agent::triage` — `run_triage`, `apply_decision`, `TriageOutcome`, `TriggerEnvelope`; dispatches the proactive agent turn for `AgentTodoProactive` sources.
- `crate::openhuman::todos` (`todos::ops`) — `add`/`remove`, `BoardLocation`, `CardPatch`; the thread-scoped board cards are stored here. Also references `agent::task_board::TaskBoardCard` for `board_cards`.
- `crate::openhuman::scheduler_gate` — `wait_for_capacity` capacity semaphore; gates proactive triage turns behind background-AI throttling.

## Used by

- `src/core/all.rs` — registers controllers + schemas into the global RPC registry.
- `src/core/jsonrpc.rs` — at startup registers the connection subscriber and starts the periodic poll.
- `src/core/event_bus/events.rs` — defines/classifies the three `TaskSource*` event variants under domain `"task_sources"`.
- `src/openhuman/config/schema/` — `TaskSourcesConfig` block feeding domain defaults.

## Notes / gotchas

- **Periodic cadence is coarse.** `TICK_SECONDS = 600` is the effective lower bound: any `interval_secs` shorter than 10 minutes is rounded up to the tick. A misconfigured `interval_secs = 0` is floored to `MIN_INTERVAL_SECONDS = 60`. The first immediate-fire tick is skipped so startup isn't slammed.
- **Pipeline is infallible at the boundary.** `run_source_once` captures any error into `FetchOutcome::error` (and a failure event) so the scheduler loop never unwinds.
- **Route-then-mark ordering.** A task is marked ingested only after routing succeeds, so a routing failure retries next pass instead of being silently dropped.
- **Edit-aware (per-source) dedup.** `content_hash` includes `url` deliberately (it drives card notes/metadata and external write-back); an unchanged re-fetch is skipped, a changed hash falls through to routing.
- **Per-source, status-aware board dedup (`route.rs` + `dedup.rs`).** The goal: a source never extracts a card for an item it already has on the board. The content-hash ledger alone can't guarantee that — if an item is edited (new hash) and its card has already progressed, the old code leaked a fresh card each pass (observed: one source accumulated 4 cards for one issue). So before creating a card, routing scans **this source's own** cards (matched via `source_metadata.source_id` + task identity `provider:external_id`/url):
  - an existing card for the item has **progressed** past `Todo` (or is claimed by a run) → **skip silently** (count `skipped_dupe`), and sweep any redundant untouched-`Todo` duplicates this source leaked;
  - the source's cards for the item are all untouched `Todo` → remove them and add one fresh card (refreshes edited content, collapses leaks);
  - no card for the item → create one.
  The load-bearing safety rule: dedup only ever removes a card that is `Todo` **and** unclaimed by a run; progressed/resolved cards (incl. `Rejected`, which must not resurface) are never yanked. Dedup is **not** cross-source — two sources matching the same item each keep their own card. Pre-existing progressed duplicates are left as-is; the fix stops *new* duplication rather than retroactively deleting in-flight cards.
- **Static executor routing (G7).** A source's optional `assigned_executor` is pre-stamped onto each card's `assigned_agent` so the dispatcher can run it deterministically without the LLM router. `add` applies it as a follow-up patch to keep `store::add_source`'s signature stable.
- **`route.rs` is the only writer of card `source_metadata`** (provider/source_id/external_id/urgency, plus url and — GitHub-only — repo).
- **`update_source` TOCTOU.** Documented theoretical read-modify-write window across three connections; acceptable at settings-panel scale.
- **Enrichment is intentionally LLM-free** — deterministic and unit-testable; the heavy reasoning happens in the downstream triage turn.
- `clear_all` exists for the E2E `test_reset` RPC.
