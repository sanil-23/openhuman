/**
 * WorkflowsTab
 * ------------
 *
 * The Intelligence page's "Workflows" tab — the single home for installed
 * workflows (the unified primitive: a goal + the procedure to reach it,
 * authored as SKILL.md bundles and served by the `workflows_*` JSON-RPC via
 * `skillsApi`).
 *
 * Owns the full workflow surface that used to live on the Connections page:
 *   - lists discovered workflows as cards,
 *   - opens a detail drawer (with a "Run workflow" CTA → /skills/run),
 *   - create / install-from-URL / uninstall flows.
 *
 * Workflows are intentionally NOT shown on Connections anymore — Connections
 * is for integrations (Composio / channels / MCP); workflows are an
 * intelligence concern.
 */
import debug from 'debug';
import { useCallback, useEffect, useState } from 'react';

import { useT } from '../../lib/i18n/I18nContext';
import { skillsApi, type SkillSummary } from '../../services/api/skillsApi';
import type { ToastNotification } from '../../types/intelligence';
import CreateSkillModal from '../skills/CreateSkillModal';
import UnifiedSkillCard from '../skills/SkillCard';
import SkillDetailDrawer from '../skills/SkillDetailDrawer';
import { BUILT_IN_SKILL_ICONS } from '../skills/skillIcons';
import UninstallSkillConfirmDialog from '../skills/UninstallSkillConfirmDialog';
import { ToastContainer } from './Toast';

const log = debug('intelligence:workflows');

export default function WorkflowsTab() {
  const { t } = useT();
  const [workflows, setWorkflows] = useState<SkillSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<SkillSummary | null>(null);
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [editingWorkflow, setEditingWorkflow] = useState<SkillSummary | null>(null);
  const [uninstallCandidate, setUninstallCandidate] = useState<SkillSummary | null>(null);
  const [toasts, setToasts] = useState<ToastNotification[]>([]);

  const addToast = useCallback((toast: Omit<ToastNotification, 'id'>) => {
    const newToast: ToastNotification = { ...toast, id: `toast-${Date.now()}-${Math.random()}` };
    setToasts(prev => [...prev, newToast]);
  }, []);
  const removeToast = useCallback((id: string) => {
    setToasts(prev => prev.filter(toast => toast.id !== id));
  }, []);

  const refresh = useCallback(async (): Promise<SkillSummary[]> => {
    try {
      const list = await skillsApi.listSkills();
      log('listWorkflows ok count=%d', list.length);
      setWorkflows(list);
      return list;
    } catch (err) {
      log('listWorkflows error %s', err instanceof Error ? err.message : String(err));
      return [];
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const list = await refresh();
      if (cancelled) return;
      void list;
    })();
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  const isEmpty = workflows.length === 0 && !loading;

  return (
    <div className="space-y-4">
      {/* Header + actions */}
      <div className="flex items-center justify-between gap-2">
        <p className="min-w-0 text-xs text-stone-500 dark:text-neutral-400">
          {t('workflows.subtitle')}
        </p>
        <div className="flex flex-shrink-0 items-center gap-2">
          <button
            type="button"
            data-testid="workflows-create-btn"
            onClick={() => setCreateModalOpen(true)}
            className="rounded-lg bg-primary-500 px-3 py-2 text-xs font-semibold text-white shadow-soft transition-colors hover:bg-primary-600 focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-1">
            {t('workflows.createNew')}
          </button>
        </div>
      </div>

      {/* Loading skeleton */}
      {loading && workflows.length === 0 ? (
        <div className="space-y-2 animate-pulse" data-testid="workflows-loading">
          {[1, 2, 3].map(i => (
            <div key={i} className="h-20 rounded-2xl bg-stone-100 dark:bg-neutral-800" />
          ))}
        </div>
      ) : null}

      {/* Empty state */}
      {isEmpty ? (
        <div className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-10 text-center shadow-soft animate-fade-up">
          <h2 className="text-sm font-semibold text-stone-900 dark:text-neutral-100">
            {t('workflows.empty.title')}
          </h2>
          <p className="mt-1 text-xs text-stone-500 dark:text-neutral-400">
            {t('workflows.empty.body')}
          </p>
          <button
            type="button"
            onClick={() => setCreateModalOpen(true)}
            className="mt-4 rounded-lg bg-primary-500 px-4 py-2 text-xs font-semibold text-white shadow-soft hover:bg-primary-600 focus:outline-none focus:ring-2 focus:ring-primary-500">
            {t('workflows.createNew')}
          </button>
        </div>
      ) : null}

      {/* Workflow list */}
      {workflows.length > 0 ? (
        <div
          className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-3 shadow-soft animate-fade-up"
          data-testid="workflows-list">
          <div className="space-y-2">
            {workflows.map(wf => {
              const scopeLabel = wf.legacy
                ? t('scope.legacy')
                : wf.scope === 'user'
                  ? t('scope.user')
                  : wf.scope === 'project'
                    ? t('scope.project')
                    : t('scope.legacy');
              const scopeColor = wf.legacy
                ? 'text-stone-600 dark:text-neutral-300'
                : wf.scope === 'user'
                  ? 'text-sage-600'
                  : wf.scope === 'project'
                    ? 'text-amber-600'
                    : 'text-stone-600 dark:text-neutral-300';
              const canUninstall = wf.scope === 'user' && !wf.legacy;
              return (
                <UnifiedSkillCard
                  key={wf.id}
                  icon={BUILT_IN_SKILL_ICONS.screenIntelligence}
                  title={wf.name}
                  description={wf.description}
                  statusLabel={scopeLabel}
                  statusColor={scopeColor}
                  ctaLabel={t('common.seeAll')}
                  testId={`workflow-card-${wf.id}`}
                  ctaTestId={`workflow-open-${wf.id}`}
                  onCtaClick={() => {
                    log('open drawer workflowId=%s', wf.id);
                    setSelected(wf);
                  }}
                  secondaryActions={
                    canUninstall
                      ? [
                          {
                            label: t('workflows.delete'),
                            testId: `workflow-uninstall-${wf.id}`,
                            icon: (
                              <svg
                                className="h-3.5 w-3.5"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2"
                                viewBox="0 0 24 24">
                                <path
                                  strokeLinecap="round"
                                  strokeLinejoin="round"
                                  d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14z"
                                />
                              </svg>
                            ),
                            onClick: () => setUninstallCandidate(wf),
                          },
                        ]
                      : undefined
                  }
                />
              );
            })}
          </div>
        </div>
      ) : null}

      {/* Detail drawer (with Run workflow CTA) */}
      {selected && (
        <SkillDetailDrawer
          skill={selected}
          onClose={() => setSelected(null)}
          onEdit={wf => {
            setSelected(null);
            setEditingWorkflow(wf);
            setCreateModalOpen(true);
          }}
        />
      )}

      {/* Create / edit modal */}
      {createModalOpen && (
        <CreateSkillModal
          editing={editingWorkflow ?? undefined}
          onClose={() => {
            setCreateModalOpen(false);
            setEditingWorkflow(null);
          }}
          onCreated={wf => {
            log('saved workflowId=%s edit=%s', wf.id, !!editingWorkflow);
            setCreateModalOpen(false);
            setEditingWorkflow(null);
            // Upsert: replace an existing row on edit, append on create.
            setWorkflows(prev =>
              prev.some(s => s.id === wf.id)
                ? prev.map(s => (s.id === wf.id ? wf : s))
                : [...prev, wf]
            );
            setSelected(wf);
            void refresh();
          }}
        />
      )}

      {/* Uninstall confirmation */}
      {uninstallCandidate && (
        <UninstallSkillConfirmDialog
          skill={uninstallCandidate}
          onClose={() => setUninstallCandidate(null)}
          onUninstalled={result => {
            log('uninstalled name=%s', result.name);
            addToast({
              type: 'success',
              title: t('workflows.delete'),
              message: `"${result.name}" ${t('common.success')}`,
            });
            setSelected(prev => (prev && prev.id === result.name ? null : prev));
            setWorkflows(prev => prev.filter(s => s.id !== result.name));
            void refresh();
          }}
        />
      )}

      <ToastContainer notifications={toasts} onRemove={removeToast} />
    </div>
  );
}
