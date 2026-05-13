import { useCallback, useEffect, useMemo, useState } from 'react';

import {
  type Backend,
  capabilityForModel,
  DEFAULT_EXTRACT_MODEL,
  downloadAsset,
  fetchInstalledModels,
  getMemoryTreeLlm,
  type ModelDescriptor,
  REQUIRED_EMBEDDER_MODEL,
  setMemoryTreeLlm,
} from '../../lib/intelligence/settingsApi';
import ModelAssignment from './ModelAssignment';
import ModelCatalog from './ModelCatalog';

/**
 * Settings tab for the Intelligence page.
 *
 * Layout (top → bottom):
 *   1. Model Assignment   — per-role dropdowns (visible only when the
 *                            memory-tree LLM backend is Local)
 *   2. Model Catalog      — full curated list with download / use
 *
 * The Cloud ↔ Local toggle that used to live on this tab moved to
 * Settings → Local AI Model → "Memory summarizer" checkbox. Both UIs
 * wrote `memory_tree.llm_backend`, so collapsing to one removes the
 * duplicate control surface (the two could drift mid-render). This tab
 * still reads that field at mount to decide whether to expose the
 * Ollama model picker sections.
 *
 * The orchestrator owns the cross-section state (cached installed-models
 * + role assignments). Sections themselves stay presentational.
 */
export default function IntelligenceSettingsTab() {
  // Mirrors `memory_tree.llm_backend`. Read-only on this tab now —
  // flipped from Local AI Settings. Used as the visibility gate for the
  // Ollama model picker UI below.
  const [backend, setBackend] = useState<Backend>('cloud');
  // Single Memory LLM that drives both extractor and summariser. Most
  // users want one model for both; the rare case of mixing them is not
  // worth the second dropdown's cognitive cost.
  const [memoryModel, setMemoryModel] = useState<string>(DEFAULT_EXTRACT_MODEL);
  const [installedModels, setInstalledModels] = useState<string[]>([]);

  // One-shot bootstrap — pull current backend and the installed-model list.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        console.debug('[intelligence-settings] bootstrap');
        const [bk, models] = await Promise.all([getMemoryTreeLlm(), fetchInstalledModels()]);
        if (cancelled) return;
        setBackend(bk);
        setInstalledModels(models.map(m => m.name));
      } catch (err) {
        if (!cancelled) {
          // Bootstrap failure leaves the tab on its useState defaults
          // (cloud backend, empty installed list) rather than throwing
          // an unhandled rejection. Subsequent reads will retry the RPCs
          // when the user navigates back.
          console.error('[intelligence-settings] bootstrap failed', err);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Persist Memory LLM changes to config.toml. Fans out to both
  // extractor and summariser keys in a single atomic write — the unified
  // UI is one dropdown, but the underlying schema retains both keys so
  // power users can still split them via the RPC directly if needed.
  const handleMemoryModelChange = useCallback(
    async (id: string) => {
      console.debug('[intelligence-settings] memory model -> %s', id);
      const previous = memoryModel;
      setMemoryModel(id);
      try {
        await setMemoryTreeLlm('local', { extractModel: id, summariserModel: id });
      } catch (err) {
        // Persistence failed → roll back the optimistic UI update so the
        // dropdown reflects the value that's actually saved on disk
        // rather than the one the user just attempted.
        setMemoryModel(previous);
        console.error('[intelligence-settings] persist memory model failed', err);
      }
    },
    [memoryModel]
  );

  const handleDownload = useCallback(async (model: ModelDescriptor) => {
    const cap = capabilityForModel(model);
    if (!cap) {
      console.debug('[intelligence-settings] no capability for model', { id: model.id });
      return;
    }
    try {
      await downloadAsset(cap);
    } catch (err) {
      console.error('[intelligence-settings] model download failed', err);
    } finally {
      // Refresh installed list after any download attempt — even on
      // failure, Ollama may have partially landed assets we should
      // surface; if it hasn't, the next bootstrap tick will catch up.
      const refreshed = await fetchInstalledModels();
      setInstalledModels(refreshed.map(m => m.name));
    }
  }, []);

  const handleUse = useCallback(
    (model: ModelDescriptor) => {
      if (model.roles.includes('extract') || model.roles.includes('summariser')) {
        void handleMemoryModelChange(model.id);
      }
    },
    [handleMemoryModelChange]
  );

  const activeModelIds = useMemo<string[]>(() => {
    const ids = new Set<string>();
    ids.add(memoryModel);
    ids.add(REQUIRED_EMBEDDER_MODEL);
    return [...ids];
  }, [memoryModel]);

  return (
    <div className="space-y-10" data-testid="intelligence-settings-tab">
      {/* Local-model sections are gated on `memory_tree.llm_backend === 'local'`.
          The toggle itself now lives in Settings → Local AI Model →
          "Memory summarizer"; this tab is read-only with respect to
          backend selection. Cloud users see an empty tab (intentional —
          there's no Ollama model picker to surface when memory tree
          isn't running on Ollama). */}
      {backend === 'local' ? (
        <>
          <Section title="Model assignment">
            <ModelAssignment
              installedModelIds={installedModels}
              memoryModel={memoryModel}
              onChangeMemory={handleMemoryModelChange}
            />
          </Section>

          <Section title="Model catalog">
            <ModelCatalog
              installedModelIds={installedModels}
              activeModelIds={activeModelIds}
              onDownload={handleDownload}
              onUse={handleUse}
            />
          </Section>
        </>
      ) : (
        <Section title="Memory model assignment">
          <p className="text-sm text-stone-500">
            Memory tree is running on cloud. To pick a local Ollama model for memory
            summarisation, enable <strong>Memory summarizer</strong> in Settings → Local AI
            Model.
          </p>
        </Section>
      )}
    </div>
  );
}

interface SectionProps {
  title: string;
  children: React.ReactNode;
}

function Section({ title, children }: SectionProps) {
  return (
    <section>
      <h2 className="font-display text-[11px] uppercase tracking-[0.18em] text-stone-400 mb-3">
        {title}
      </h2>
      {children}
    </section>
  );
}
