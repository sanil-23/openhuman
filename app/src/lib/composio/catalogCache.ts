/**
 * Client-side cache for the Composio toolkit catalog.
 *
 * Mirrors the backend's 24h cache (see the workspace
 * `COMPOSIO_DYNAMIC_CATALOG_PLAN.md`). The catalog only changes when
 * Composio adds/removes toolkits, so re-fetching it on every Skills-page
 * mount is wasteful. We layer two guards in front of `listToolkits()`:
 *
 *   1. **In-flight dedupe** — N components mounting at once share a single
 *      RPC instead of firing one each. Race-free because the JS event loop
 *      never interleaves the synchronous check-then-assign below.
 *   2. **localStorage TTL (24h)** — survives reloads and serves instantly
 *      on a warm cache; falls back to a live fetch when stale/absent.
 *
 * `invalidateToolkitCatalogCache()` clears both tiers — call it when the
 * Composio client identity changes (backend ↔ direct mode, BYO API key),
 * exactly like the existing `composio:config-changed` refresh path.
 */
import { listToolkits } from './composioApi';
import type { ComposioToolkitsResponse } from './types';

const CACHE_KEY = 'composio:catalog:v1';
const TTL_MS = 24 * 60 * 60 * 1000;

interface CachedCatalog {
  fetchedAt: number;
  response: ComposioToolkitsResponse;
}

/** Module-level in-flight promise so concurrent callers join one fetch. */
let inflight: Promise<ComposioToolkitsResponse> | null = null;
/** In-memory mirror so we avoid a JSON.parse on the hot path. */
let memory: CachedCatalog | null = null;

function readPersisted(): CachedCatalog | null {
  if (memory) return memory;
  try {
    const raw = window.localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CachedCatalog;
    if (
      !parsed ||
      typeof parsed.fetchedAt !== 'number' ||
      !parsed.response ||
      !Array.isArray(parsed.response.toolkits)
    ) {
      return null;
    }
    memory = parsed;
    return parsed;
  } catch {
    return null;
  }
}

function writePersisted(response: ComposioToolkitsResponse): void {
  const entry: CachedCatalog = { fetchedAt: Date.now(), response };
  memory = entry;
  try {
    window.localStorage.setItem(CACHE_KEY, JSON.stringify(entry));
  } catch {
    // Private-mode / quota errors are non-fatal — the in-memory mirror
    // still serves this session.
  }
}

function isFresh(entry: CachedCatalog | null): boolean {
  return entry !== null && Date.now() - entry.fetchedAt < TTL_MS;
}

/**
 * Resolve the toolkit catalog, preferring a fresh client cache.
 *
 * - Fresh cache (< 24h)            → returned immediately, no RPC.
 * - Stale-but-present + fetch ok   → fresh value, cache refreshed.
 * - Stale-but-present + fetch fail → stale value served (graceful degrade).
 * - Cold + fetch fail              → error propagates to the caller.
 */
export async function getToolkitCatalog(): Promise<ComposioToolkitsResponse> {
  const cached = readPersisted();
  if (cached && isFresh(cached)) return cached.response;

  if (inflight) return inflight;
  inflight = listToolkits()
    .then(response => {
      writePersisted(response);
      return response;
    })
    .catch(err => {
      // On failure, fall back to a stale cache if we have one rather than
      // forcing the UI into an error state for a list that rarely changes.
      if (cached) {
        console.warn(
          '[composio-cache] catalog fetch failed; serving stale cache:',
          err instanceof Error ? err.message : String(err)
        );
        return cached.response;
      }
      throw err;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

/** Drop both cache tiers so the next read re-fetches. */
export function invalidateToolkitCatalogCache(): void {
  memory = null;
  inflight = null;
  try {
    window.localStorage.removeItem(CACHE_KEY);
  } catch {
    // ignore
  }
}
