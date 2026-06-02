/**
 * /workflows/run — single-purpose workflow runner page.
 *
 * Reached from a workflow's detail drawer ("Run workflow" CTA) or any
 * `?workflow=<id>` deep link. Hosts the WorkflowRunnerBody picker + form +
 * run-now + save-schedule flow without the Connections-page tab chrome.
 *
 * Bookmark-friendly and shareable via `?workflow=<id>` (the body reads the
 * query param and pre-selects the workflow — see WorkflowRunnerBody.tsx).
 */
import { useNavigate } from 'react-router-dom';

import WorkflowRunnerBody from '../components/skills/WorkflowRunnerBody';
import { useT } from '../lib/i18n/I18nContext';

export default function WorkflowsRun() {
  const { t } = useT();
  const navigate = useNavigate();

  return (
    <div className="min-h-full flex flex-col">
      <div className="flex-1 flex items-start justify-center p-4 pt-6">
        <div className="w-full max-w-3xl space-y-4">
          {/* Page header with a "back to Connections" affordance so the
              user can always retreat without clicking the bottom-tab. */}
          <div className="flex items-center gap-3">
            <button
              type="button"
              onClick={() => navigate('/skills')}
              aria-label={t('common.back')}
              className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-stone-600 dark:text-neutral-300 hover:bg-stone-100 dark:hover:bg-neutral-800 transition-colors">
              <span aria-hidden="true">←</span> {t('common.back')}
            </button>
            <h1 className="text-base font-semibold text-stone-900 dark:text-neutral-100">
              {t('skills.run.title')}
            </h1>
          </div>

          <div className="rounded-2xl border border-stone-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 p-6 shadow-soft animate-fade-up">
            <WorkflowRunnerBody />
          </div>
        </div>
      </div>
    </div>
  );
}
