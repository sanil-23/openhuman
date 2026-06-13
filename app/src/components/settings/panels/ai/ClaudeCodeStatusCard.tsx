import { useCallback, useEffect, useState } from 'react';
import { LuKeyRound, LuX } from 'react-icons/lu';

import {
  type ClaudeCodeAuthStatus,
  openhumanClaudeCodeAuthStatus,
  openhumanClaudeCodeLoginLaunch,
  openhumanClaudeCodeSetFullAccess,
  openhumanClaudeCodeSettings,
} from '../../../../utils/tauriCommands/config';

/**
 * Claude Code CLI connect control — the peer of the Codex connect button.
 *
 * Inline: a "Claude Code" button + a one-line status summary. Clicking the
 * button opens a modal with the actual controls (enable/disable, sign-in /
 * reconnect, install hint).
 *
 * Auth is probed via `claude auth status --json` (cross-platform: covers the
 * macOS Keychain as well as the Linux/Windows file stores) or
 * `ANTHROPIC_API_KEY`. We do NOT spawn the slow `claude --version` probe — a
 * missing/old binary surfaces as `unknown` from the auth probe, rendered as a
 * compact install hint rather than "signed out".
 */
export function ClaudeCodeConnect({
  connected,
  busy = false,
  onConnect,
  onDisconnect,
}: {
  connected: boolean;
  busy?: boolean;
  onConnect: () => void | Promise<void>;
  onDisconnect: () => void | Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [auth, setAuth] = useState<ClaudeCodeAuthStatus | null>(null);
  const [authLoading, setAuthLoading] = useState(false);
  const [acting, setActing] = useState(false);

  const probeAuth = useCallback(async () => {
    setAuthLoading(true);
    try {
      // Resolves to the BARE AuthStatus (no `{ result }` envelope) — see the
      // wrapper in tauriCommands/config.ts.
      const resp = await openhumanClaudeCodeAuthStatus();
      setAuth(resp);
    } catch {
      setAuth(null);
    } finally {
      setAuthLoading(false);
    }
  }, []);

  // Probe once connected so the inline summary + modal reflect sign-in state.
  useEffect(() => {
    if (connected) {
      void probeAuth();
    } else {
      setAuth(null);
    }
  }, [connected, probeAuth]);

  const runConnect = async () => {
    setActing(true);
    try {
      await onConnect();
    } finally {
      setActing(false);
    }
  };
  const runDisconnect = async () => {
    setActing(true);
    try {
      await onDisconnect();
    } finally {
      setActing(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-2">
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="inline-flex items-center gap-2 rounded-lg border border-stone-200 bg-white px-3 py-2 text-xs font-medium text-stone-900 transition-colors hover:bg-stone-50 disabled:cursor-wait disabled:opacity-60 dark:border-neutral-800 dark:bg-neutral-900 dark:text-neutral-100 dark:hover:bg-neutral-800">
        <LuKeyRound className="h-3.5 w-3.5" />
        Claude Code
      </button>
      <span className="text-xs text-stone-500 dark:text-neutral-400">
        <InlineSummary connected={connected} auth={auth} loading={authLoading} />
      </span>

      {open && (
        <ClaudeCodeModal
          connected={connected}
          busy={busy || acting}
          auth={auth}
          authLoading={authLoading}
          onClose={() => setOpen(false)}
          onConnect={runConnect}
          onDisconnect={runDisconnect}
          onRecheck={probeAuth}
        />
      )}
    </div>
  );
}

/** Title-case a raw subscription type (`"max"` → `"Max"`) for display. */
function formatSubscriptionType(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return trimmed;
  return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
}

/** Heuristic: does an `unknown` reason indicate the binary is missing? */
function looksNotInstalled(reason: string | null): boolean {
  if (!reason) return false;
  const r = reason.toLowerCase();
  return r.includes('not found') || r.includes('not installed') || r.includes('path');
}

/** One-line status shown next to the inline "Claude Code" button. */
function InlineSummary({
  connected,
  auth,
  loading,
}: {
  connected: boolean;
  auth: ClaudeCodeAuthStatus | null;
  loading: boolean;
}) {
  if (!connected) {
    return <>Not connected — routes chat through your local Claude Code CLI.</>;
  }
  if (!auth) {
    return <>{loading ? 'Checking sign-in…' : 'Connected.'}</>;
  }
  if (auth.source === 'subscription') {
    const who = auth.account_email ?? 'Claude subscription';
    const plan = auth.subscription_type
      ? ` (${formatSubscriptionType(auth.subscription_type)})`
      : '';
    return (
      <span className="text-emerald-600 dark:text-emerald-400">
        Signed in as {who}
        {plan}
      </span>
    );
  }
  if (auth.source === 'api_key_env') {
    return <span className="text-emerald-600 dark:text-emerald-400">Using ANTHROPIC_API_KEY</span>;
  }
  if (auth.source === 'unknown') {
    return (
      <span className="text-amber-600 dark:text-amber-400">
        {looksNotInstalled(auth.reason) ? 'CLI not installed' : 'Sign-in state unknown'}
      </span>
    );
  }
  return <span className="text-amber-600 dark:text-amber-400">Connected · not signed in</span>;
}

/**
 * Modal with the actual Claude Code controls: enable/disable the provider,
 * sign in / reconnect via the CLI, and install guidance.
 */
function ClaudeCodeModal({
  connected,
  busy,
  auth,
  authLoading,
  onClose,
  onConnect,
  onDisconnect,
  onRecheck,
}: {
  connected: boolean;
  busy: boolean;
  auth: ClaudeCodeAuthStatus | null;
  authLoading: boolean;
  onClose: () => void;
  onConnect: () => void | Promise<void>;
  onDisconnect: () => void | Promise<void>;
  onRecheck: () => void | Promise<void>;
}) {
  const [launching, setLaunching] = useState(false);

  // Persisted full-access toggle (bypassPermissions vs the default acceptEdits).
  // `null` until loaded so the switch can render a disabled placeholder.
  const [fullAccess, setFullAccess] = useState<boolean | null>(null);
  const [savingAccess, setSavingAccess] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const s = await openhumanClaudeCodeSettings();
        if (!cancelled) setFullAccess(s.full_access);
      } catch {
        // Fail safe to OFF (acceptEdits) if the read fails.
        if (!cancelled) setFullAccess(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const toggleFullAccess = async (next: boolean) => {
    setSavingAccess(true);
    setFullAccess(next); // optimistic
    try {
      const s = await openhumanClaudeCodeSetFullAccess(next);
      setFullAccess(s.full_access);
    } catch {
      setFullAccess(!next); // revert on failure
    } finally {
      setSavingAccess(false);
    }
  };

  const launchLogin = async () => {
    setLaunching(true);
    try {
      await openhumanClaudeCodeLoginLaunch();
    } finally {
      setLaunching(false);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Claude Code CLI"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-4"
      onClick={onClose}>
      <div
        className="w-full max-w-md rounded-2xl border border-stone-200 bg-white p-6 shadow-soft dark:border-neutral-800 dark:bg-neutral-900"
        onClick={e => e.stopPropagation()}>
        <div className="mb-4 flex items-start justify-between gap-3">
          <div>
            <h3 className="text-base font-semibold text-stone-900 dark:text-neutral-100">
              Claude Code CLI
            </h3>
            <p className="mt-1 max-w-sm text-xs leading-5 text-stone-500 dark:text-neutral-400">
              Routes chat, agentic and reasoning workloads through your locally-installed Claude
              Code CLI. No API key — it uses the CLI's own login.
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700 dark:text-neutral-500 dark:hover:bg-neutral-800 dark:hover:text-neutral-200">
            <LuX className="h-4 w-4" />
          </button>
        </div>

        {/* Connection */}
        <div className="flex items-center justify-between gap-3 rounded-lg border border-stone-200 px-3 py-2 dark:border-neutral-800">
          <div className="text-xs">
            <div className="font-medium text-stone-900 dark:text-neutral-100">Connection</div>
            <div
              className={
                connected
                  ? 'text-emerald-600 dark:text-emerald-400'
                  : 'text-stone-500 dark:text-neutral-400'
              }>
              {connected ? 'Enabled' : 'Not enabled'}
            </div>
          </div>
          {connected ? (
            <button
              type="button"
              onClick={() => void onDisconnect()}
              disabled={busy}
              className="rounded-md border border-rose-300 px-2.5 py-1 text-xs font-medium text-rose-600 hover:bg-rose-50 disabled:opacity-50 dark:border-rose-500/40 dark:text-rose-400 dark:hover:bg-rose-500/10">
              {busy ? 'Disconnecting…' : 'Disconnect'}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void onConnect()}
              disabled={busy}
              className="rounded-md bg-neutral-900 px-2.5 py-1 text-xs font-medium text-white hover:bg-neutral-700 disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-neutral-300">
              {busy ? 'Enabling…' : 'Enable Claude Code'}
            </button>
          )}
        </div>

        {/* Authentication */}
        <div className="mt-3 rounded-lg border border-stone-200 px-3 py-2 dark:border-neutral-800">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-xs font-medium text-stone-900 dark:text-neutral-100">
              Authentication
            </span>
            <button
              type="button"
              onClick={() => void onRecheck()}
              disabled={authLoading}
              className="text-xs text-neutral-500 hover:text-neutral-900 disabled:opacity-50 dark:text-neutral-400 dark:hover:text-neutral-100">
              {authLoading ? 'Checking…' : 'Recheck'}
            </button>
          </div>
          <AuthDetail auth={auth} loading={authLoading} />
          <div className="mt-2">
            <button
              type="button"
              onClick={() => void launchLogin()}
              disabled={launching}
              className="rounded-md border border-neutral-300 px-2.5 py-1 text-xs font-medium text-neutral-700 hover:bg-neutral-100 disabled:opacity-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800">
              {launching
                ? 'Opening terminal…'
                : auth?.source === 'none'
                  ? 'Sign in with Claude'
                  : 'Reconnect'}
            </button>
            <p className="mt-1.5 text-[11px] text-stone-500 dark:text-neutral-400">
              Opens a terminal running <code>claude login</code>. After it completes, click{' '}
              <strong>Recheck</strong>.
            </p>
          </div>
        </div>

        {/* Permissions — full access vs. the default acceptEdits posture. */}
        <div className="mt-3 rounded-lg border border-stone-200 px-3 py-2 dark:border-neutral-800">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-xs font-medium text-stone-900 dark:text-neutral-100">
                Full access
              </div>
              <p className="mt-0.5 text-[11px] leading-4 text-stone-500 dark:text-neutral-400">
                {fullAccess
                  ? 'Claude Code can run commands, use the network, and spawn subagents.'
                  : 'Accept edits only — auto-applies file edits, gates commands & network.'}
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={fullAccess === true}
              aria-label="Full access"
              disabled={fullAccess === null || savingAccess}
              onClick={() => void toggleFullAccess(!fullAccess)}
              className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:cursor-wait disabled:opacity-50 ${
                fullAccess
                  ? 'bg-emerald-500 dark:bg-emerald-500'
                  : 'bg-stone-300 dark:bg-neutral-700'
              }`}>
              <span
                className={`inline-block h-4 w-4 transform rounded-full bg-white shadow transition-transform ${
                  fullAccess ? 'translate-x-4' : 'translate-x-0.5'
                }`}
              />
            </button>
          </div>
          <p className="mt-1.5 text-[11px] leading-4 text-stone-400 dark:text-neutral-500">
            {isMac()
              ? 'On macOS, ~/.openhuman stays protected by the sandbox in either mode.'
              : 'Full access is unconfined on this platform — enable only if you trust the workspace.'}
          </p>
        </div>
      </div>
    </div>
  );
}

/** Best-effort macOS detection for the permissions copy (UA-based). */
function isMac(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent || '');
}

/** Detailed auth line inside the modal. */
function AuthDetail({ auth, loading }: { auth: ClaudeCodeAuthStatus | null; loading: boolean }) {
  if (!auth) {
    return (
      <p className="text-xs text-neutral-500 dark:text-neutral-400">
        {loading ? 'Checking sign-in…' : 'Enable Claude Code to check sign-in.'}
      </p>
    );
  }
  if (auth.source === 'subscription') {
    const who = auth.account_email ?? 'Claude subscription';
    const plan = auth.subscription_type
      ? ` (${formatSubscriptionType(auth.subscription_type)})`
      : '';
    return (
      <p className="text-xs text-emerald-600 dark:text-emerald-400">
        Signed in as {who}
        {plan}
      </p>
    );
  }
  if (auth.source === 'api_key_env') {
    return (
      <p className="text-xs text-emerald-600 dark:text-emerald-400">
        Using <code>ANTHROPIC_API_KEY</code> from the environment.
      </p>
    );
  }
  if (auth.source === 'unknown') {
    if (looksNotInstalled(auth.reason)) {
      return (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          Claude Code CLI not found — install via{' '}
          <code>npm install -g @anthropic-ai/claude-code</code>.
        </p>
      );
    }
    return (
      <p className="text-xs text-amber-600 dark:text-amber-400">
        Couldn't determine sign-in state. Your <code>claude</code> CLI may predate{' '}
        <code>auth status</code> — try Reconnect, then Recheck.
      </p>
    );
  }
  return <p className="text-xs text-amber-600 dark:text-amber-400">Not signed in.</p>;
}
