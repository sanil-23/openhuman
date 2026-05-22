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

type Preset = 'readonly' | 'workspace' | 'trusted' | 'full' | 'custom';

interface PresetOption {
  id: Exclude<Preset, 'custom'>;
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
    id: 'workspace',
    title: 'Ask before edit',
    description:
      'Creates new files freely in the workspace, but asks for your approval before editing an existing file, running a command, reaching the network, or installing anything.',
  },
  {
    id: 'trusted',
    title: 'Ask before edit + granted folders',
    description:
      'Same as “Ask before edit”, plus the specific folders you grant below. Everything else outside the workspace stays blocked.',
  },
  {
    id: 'full',
    title: 'Full access',
    description:
      'Runs commands with your full user account access, unconfined — it can read/write anywhere your user can, except credential and system stores. Destructive commands, network access, and installs still ask for approval.',
  },
];

const derivePreset = (
  level: AutonomyLevel,
  workspaceOnly: boolean,
  allowToolInstall: boolean,
  trustedRoots: TrustedRoot[]
): Preset => {
  if (level === 'full' && !workspaceOnly) return 'full';
  if (level === 'readonly' && workspaceOnly && !allowToolInstall) return 'readonly';
  if (level === 'supervised' && workspaceOnly && !allowToolInstall) {
    return trustedRoots.length > 0 ? 'trusted' : 'workspace';
  }
  return 'custom';
};

const AgentAccessPanel = () => {
  const { navigateBack, breadcrumbs } = useSettingsNavigation();

  const [level, setLevel] = useState<AutonomyLevel>('supervised');
  const [workspaceOnly, setWorkspaceOnly] = useState(true);
  const [trustedRoots, setTrustedRoots] = useState<TrustedRoot[]>([]);
  const [allowToolInstall, setAllowToolInstall] = useState(false);

  const [showAdvanced, setShowAdvanced] = useState(false);
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
        const a = resp.result;
        setLevel(a.level);
        setWorkspaceOnly(a.workspace_only);
        setTrustedRoots(a.trusted_roots ?? []);
        setAllowToolInstall(a.allow_tool_install);
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

  const activePreset = derivePreset(level, workspaceOnly, allowToolInstall, trustedRoots);

  const applyPreset = (preset: Exclude<Preset, 'custom'>) => {
    setSavedNote(null);
    switch (preset) {
      case 'readonly':
        setLevel('readonly');
        setWorkspaceOnly(true);
        setAllowToolInstall(false);
        break;
      case 'workspace':
        setLevel('supervised');
        setWorkspaceOnly(true);
        setAllowToolInstall(false);
        break;
      case 'trusted':
        setLevel('supervised');
        setWorkspaceOnly(true);
        setAllowToolInstall(false);
        setShowAdvanced(true);
        break;
      case 'full':
        setLevel('full');
        setWorkspaceOnly(false);
        setAllowToolInstall(true);
        break;
    }
  };

  const addRoot = () => {
    const path = newRootPath.trim();
    if (!path) return;
    if (trustedRoots.some(r => r.path === path)) {
      setNewRootPath('');
      return;
    }
    setTrustedRoots([...trustedRoots, { path, access: newRootAccess }]);
    setNewRootPath('');
    setNewRootAccess('read');
  };

  const removeRoot = (path: string) => {
    setTrustedRoots(trustedRoots.filter(r => r.path !== path));
  };

  const save = async () => {
    if (!isTauri()) return;
    setError(null);
    setSavedNote(null);
    setIsSaving(true);
    try {
      await openhumanUpdateAutonomySettings({
        level,
        workspace_only: workspaceOnly,
        trusted_roots: trustedRoots,
        allow_tool_install: allowToolInstall,
      });
      setSavedNote('Saved. New conversations use the updated access mode.');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to save access settings');
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div>
      <SettingsHeader
        title="Agent access"
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
                    onClick={() => applyPreset(p.id)}
                    className={`text-left rounded-lg border p-3 transition ${
                      activePreset === p.id
                        ? 'border-ocean bg-ocean/5'
                        : 'border-line hover:border-ocean/50'
                    }`}>
                    <div className="flex items-center gap-2">
                      <span
                        className={`inline-block w-3 h-3 rounded-full border ${
                          activePreset === p.id ? 'bg-ocean border-ocean' : 'border-line'
                        }`}
                      />
                      <span className="font-medium text-ink">{p.title}</span>
                      {p.id === 'workspace' && (
                        <span className="text-xs text-ink-soft">(default)</span>
                      )}
                    </div>
                    <p className="mt-1 text-xs text-ink-soft">{p.description}</p>
                  </button>
                ))}
                {activePreset === 'custom' && (
                  <p className="text-xs text-amber">
                    Custom configuration (set via Advanced or config.toml).
                  </p>
                )}
                {activePreset === 'full' && (
                  <p className="rounded border border-coral/40 bg-coral/5 p-2 text-xs text-coral">
                    ⚠ Full access runs commands with your full account access and is{' '}
                    <strong>not sandboxed</strong>. Only enable it when you trust the agent with
                    this machine. Credential and system directories stay blocked, and destructive,
                    network, and install actions still ask for approval.
                  </p>
                )}
              </div>
            </section>

            {/* Trusted roots editor — relevant for Trusted Roots / custom modes. */}
            <section className="space-y-2">
              <h2 className="text-sm font-semibold text-ink">
                Granted folders (outside workspace)
              </h2>
              <p className="text-xs text-ink-soft">
                Each folder is reachable in addition to the workspace. Credential stores (~/.ssh,
                ~/.gnupg, ~/.aws, keychains) and system directories (/etc, /System, C:\Windows, …)
                are always blocked, even inside a granted folder.
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
                  className="rounded bg-ocean px-3 py-1 text-xs text-white hover:bg-ocean/90">
                  Add
                </button>
              </div>
            </section>

            <section className="space-y-2">
              <button
                type="button"
                onClick={() => setShowAdvanced(v => !v)}
                className="text-xs text-ocean hover:underline">
                {showAdvanced ? '▾ Advanced' : '▸ Advanced'}
              </button>
              {showAdvanced && (
                <div className="space-y-3 rounded-lg border border-line p-3">
                  <label className="flex items-center justify-between text-sm">
                    <span className="text-ink">Confine to workspace (workspace_only)</span>
                    <input
                      type="checkbox"
                      checked={workspaceOnly}
                      onChange={e => setWorkspaceOnly(e.target.checked)}
                    />
                  </label>
                  <label className="flex items-center justify-between text-sm">
                    <span className="text-ink">Allow OS package installs (install_tool)</span>
                    <input
                      type="checkbox"
                      checked={allowToolInstall}
                      onChange={e => setAllowToolInstall(e.target.checked)}
                    />
                  </label>
                  <label className="flex items-center justify-between text-sm">
                    <span className="text-ink">Autonomy level</span>
                    <select
                      value={level}
                      onChange={e => setLevel(e.target.value as AutonomyLevel)}
                      className="rounded border border-line px-2 py-1 text-xs">
                      <option value="readonly">read-only</option>
                      <option value="supervised">supervised</option>
                      <option value="full">full</option>
                    </select>
                  </label>
                </div>
              )}
            </section>

            {error && <p className="text-sm text-coral">{error}</p>}
            {savedNote && <p className="text-sm text-sage">{savedNote}</p>}

            <button
              type="button"
              onClick={save}
              disabled={isSaving || !isTauri()}
              className="rounded-lg bg-ocean px-4 py-2 text-sm text-white hover:bg-ocean/90 disabled:opacity-50">
              {isSaving ? 'Saving…' : 'Save access mode'}
            </button>
          </>
        )}
      </div>
    </div>
  );
};

export default AgentAccessPanel;
