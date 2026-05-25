import { useEffect, useState } from 'react';

import {
  type AutonomyLevel,
  isTauri,
  openhumanGetAutonomySettings,
  openhumanUpdateAutonomySettings,
  type TrustedAccess,
  type TrustedRoot,
} from '../../../utils/tauriCommands';
import SettingsHeader from '../components/SettingsHeader';
import { useSettingsNavigation } from '../hooks/useSettingsNavigation';

// The install tool is always available (installs still go through the approval
// gate), so this is fixed rather than a UI knob. The access *tier* and the
// "confine to workspace" toggle are the user-facing controls.
const ALLOW_TOOL_INSTALL = true;

interface PresetOption {
  id: AutonomyLevel;
  title: string;
  description: string;
}

const PRESETS: PresetOption[] = [
  {
    id: 'readonly',
    title: 'Read-only',
    description:
      'Reads files and runs read-only commands to explore — but never writes, edits, or runs anything that changes state.',
  },
  {
    id: 'supervised',
    title: 'Ask before edit',
    description:
      'Creates new files freely, but asks for your approval before editing an existing file, running a command, reaching the network, or installing anything.',
  },
  {
    id: 'full',
    title: 'Full access',
    description:
      'Runs commands with your full user account access — it can read/write anywhere allowed, except credential and system stores. Destructive commands, network access, and installs still ask for approval.',
  },
];

const AgentAccessPanel = () => {
  const { navigateBack, breadcrumbs } = useSettingsNavigation();

  const [level, setLevel] = useState<AutonomyLevel>('supervised');
  const [workspaceOnly, setWorkspaceOnly] = useState(false);
  const [trustedRoots, setTrustedRoots] = useState<TrustedRoot[]>([]);

  const [newRootPath, setNewRootPath] = useState('');
  const [newRootAccess, setNewRootAccess] = useState<TrustedAccess>('read');

  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedNote, setSavedNote] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      if (!isTauri()) {
        setIsLoading(false);
        return;
      }
      try {
        const resp = await openhumanGetAutonomySettings();
        if (cancelled) return;
        setLevel(resp.result.level);
        setWorkspaceOnly(resp.result.workspace_only);
        setTrustedRoots(resp.result.trusted_roots ?? []);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load access settings');
      } finally {
        if (!cancelled) setIsLoading(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  // Auto-apply: every change persists immediately (no separate Save button).
  // `allow_tool_install` is fixed; tier, workspace_only and granted folders
  // vary. Pass explicit `next` values (setState is async).
  const persist = async (next: {
    level: AutonomyLevel;
    workspaceOnly: boolean;
    trustedRoots: TrustedRoot[];
  }) => {
    if (!isTauri()) return;
    setError(null);
    setSavedNote(null);
    setIsSaving(true);
    try {
      await openhumanUpdateAutonomySettings({
        level: next.level,
        workspace_only: next.workspaceOnly,
        trusted_roots: next.trustedRoots,
        allow_tool_install: ALLOW_TOOL_INSTALL,
      });
      setSavedNote('Saved — applies on your next message.');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to save access settings');
    } finally {
      setIsSaving(false);
    }
  };

  const selectTier = (next: AutonomyLevel) => {
    setLevel(next);
    void persist({ level: next, workspaceOnly, trustedRoots });
  };

  const toggleWorkspaceOnly = (next: boolean) => {
    setWorkspaceOnly(next);
    void persist({ level, workspaceOnly: next, trustedRoots });
  };

  const addRoot = () => {
    const path = newRootPath.trim();
    if (!path) return;
    if (trustedRoots.some(r => r.path === path)) {
      setNewRootPath('');
      return;
    }
    const nextRoots = [...trustedRoots, { path, access: newRootAccess }];
    setTrustedRoots(nextRoots);
    setNewRootPath('');
    setNewRootAccess('read');
    void persist({ level, workspaceOnly, trustedRoots: nextRoots });
  };

  const removeRoot = (path: string) => {
    const nextRoots = trustedRoots.filter(r => r.path !== path);
    setTrustedRoots(nextRoots);
    void persist({ level, workspaceOnly, trustedRoots: nextRoots });
  };

  return (
    <div>
      <SettingsHeader
        title="Agent OS access"
        showBackButton
        onBack={navigateBack}
        breadcrumbs={breadcrumbs}
      />

      <div className="p-4 space-y-6">
        {!isTauri() && (
          <p className="text-sm text-coral">
            Access settings are only available in the desktop app.
          </p>
        )}

        {isLoading ? (
          <p className="text-sm text-ink-soft">Loading…</p>
        ) : (
          <>
            <section className="space-y-2">
              <h2 className="text-sm font-semibold text-ink">Access mode</h2>
              <div className="grid gap-2">
                {PRESETS.map(p => (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => selectTier(p.id)}
                    className={`text-left rounded-lg border p-3 transition ${
                      level === p.id
                        ? 'border-primary-500 bg-primary-50'
                        : 'border-line hover:border-primary-300'
                    }`}>
                    <div className="flex items-center gap-2">
                      <span
                        className={`inline-block w-3 h-3 rounded-full border ${
                          level === p.id ? 'bg-primary-500 border-primary-500' : 'border-line'
                        }`}
                      />
                      <span className="font-medium text-ink">{p.title}</span>
                      {p.id === 'supervised' && (
                        <span className="text-xs text-ink-soft">(default)</span>
                      )}
                    </div>
                    <p className="mt-1 text-xs text-ink-soft">{p.description}</p>
                  </button>
                ))}
                {level === 'full' && (
                  <p className="rounded border border-coral/40 bg-coral/5 p-2 text-xs text-coral">
                    ⚠ Full access runs commands with your full account access and is{' '}
                    <strong>not sandboxed</strong>. Only enable it when you trust the agent with
                    this machine. Credential and system directories stay blocked, and destructive,
                    network, and install actions still ask for approval.
                  </p>
                )}
              </div>
            </section>

            {/* Workspace confinement — orthogonal to the tier; applies in all modes. */}
            <section className="space-y-1">
              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  className="mt-0.5 cursor-pointer"
                  checked={workspaceOnly}
                  onChange={e => toggleWorkspaceOnly(e.target.checked)}
                />
                <span>
                  <span className="text-sm font-medium text-ink">Confine to workspace</span>
                  <span className="block text-xs text-ink-soft">
                    Restrict the agent to the workspace directory (plus any granted folders),
                    whichever access mode is selected. When off, it can reach anywhere your user can
                    — except the always-blocked credential and system directories.
                  </span>
                </span>
              </label>
            </section>

            {/* Granted folders (trusted roots) — extra read/write reach. */}
            <section className="space-y-2">
              <h2 className="text-sm font-semibold text-ink">Granted folders</h2>
              <p className="text-xs text-ink-soft">
                Folders the agent may read and write, in addition to the workspace. Credential
                stores (~/.ssh, ~/.gnupg, ~/.aws, keychains) and system directories (/etc, /System,
                C:\Windows, …) are always blocked, even inside a granted folder.
              </p>
              {trustedRoots.length === 0 ? (
                <p className="text-xs text-ink-soft">No folders granted.</p>
              ) : (
                <ul className="space-y-1">
                  {trustedRoots.map(r => (
                    <li
                      key={r.path}
                      className="flex items-center justify-between rounded border border-line px-2 py-1">
                      <span className="font-mono text-xs text-ink truncate">{r.path}</span>
                      <span className="flex items-center gap-2">
                        <span className="text-xs text-ink-soft">
                          {r.access === 'readwrite' ? 'read + write' : 'read-only'}
                        </span>
                        <button
                          type="button"
                          onClick={() => removeRoot(r.path)}
                          className="text-xs text-coral hover:underline">
                          Remove
                        </button>
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={newRootPath}
                  onChange={e => setNewRootPath(e.target.value)}
                  placeholder="/absolute/path/to/folder"
                  className="flex-1 rounded border border-line px-2 py-1 text-xs font-mono"
                />
                <select
                  value={newRootAccess}
                  onChange={e => setNewRootAccess(e.target.value as TrustedAccess)}
                  className="rounded border border-line px-2 py-1 text-xs">
                  <option value="read">read-only</option>
                  <option value="readwrite">read + write</option>
                </select>
                <button
                  type="button"
                  onClick={addRoot}
                  className="rounded bg-primary-500 px-3 py-1 text-xs text-white hover:bg-primary-600">
                  Add
                </button>
              </div>
            </section>

            {/* Auto-save status — changes persist on selection; no manual save. */}
            <div className="min-h-[1.25rem] text-sm" aria-live="polite">
              {error ? (
                <span className="text-coral">{error}</span>
              ) : isSaving ? (
                <span className="text-ink-soft">Saving…</span>
              ) : savedNote ? (
                <span className="text-sage">✓ {savedNote}</span>
              ) : (
                <span className="text-ink-soft">Changes apply on your next message.</span>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
};

export default AgentAccessPanel;
