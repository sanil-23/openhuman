import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ClaudeCodeConnect } from '../ClaudeCodeStatusCard';

const authProbe = vi.fn();
const loginLaunch = vi.fn();

vi.mock('../../../../../utils/tauriCommands/config', () => ({
  // Resolves to the BARE AuthStatus (no `{ result }` envelope), matching the
  // real wrapper.
  openhumanClaudeCodeAuthStatus: () => authProbe(),
  openhumanClaudeCodeLoginLaunch: () => loginLaunch(),
}));

const noop = () => {};

describe('ClaudeCodeConnect', () => {
  beforeEach(() => {
    authProbe.mockReset();
    loginLaunch.mockReset();
    loginLaunch.mockResolvedValue('cmd');
    authProbe.mockResolvedValue({ source: 'none', last_checked: 0 });
  });

  it('disconnected: shows the "Claude Code" button + "Not connected" summary, no probe', () => {
    render(<ClaudeCodeConnect connected={false} onConnect={noop} onDisconnect={noop} />);
    expect(screen.getByRole('button', { name: /Claude Code/i })).toBeInTheDocument();
    expect(screen.getByText(/Not connected/i)).toBeInTheDocument();
    expect(authProbe).not.toHaveBeenCalled();
    // No modal until the button is clicked.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('connected: inline summary shows the subscription email + plan', async () => {
    authProbe.mockResolvedValueOnce({
      source: 'subscription',
      account_email: 'jamie@example.com',
      subscription_type: 'max',
      expires_at: null,
      last_checked: 1,
    });
    render(<ClaudeCodeConnect connected onConnect={noop} onDisconnect={noop} />);
    await waitFor(() => {
      expect(screen.getByText(/Signed in as jamie@example\.com \(Max\)/)).toBeInTheDocument();
    });
  });

  it('clicking the button opens a modal with Enable when disconnected', async () => {
    const onConnect = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<ClaudeCodeConnect connected={false} onConnect={onConnect} onDisconnect={noop} />);
    await user.click(screen.getByRole('button', { name: /^Claude Code$/i }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: /Enable Claude Code/i }));
    expect(onConnect).toHaveBeenCalledTimes(1);
  });

  it('modal: sign-in launches the CLI login, Disconnect calls onDisconnect', async () => {
    authProbe.mockResolvedValue({ source: 'none', last_checked: 0 });
    const onDisconnect = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<ClaudeCodeConnect connected onConnect={noop} onDisconnect={onDisconnect} />);
    await user.click(screen.getByRole('button', { name: /^Claude Code$/i }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: /Sign in with Claude/i }));
    expect(loginLaunch).toHaveBeenCalledTimes(1);
    await user.click(within(dialog).getByRole('button', { name: /Disconnect/i }));
    expect(onDisconnect).toHaveBeenCalledTimes(1);
  });

  it('modal: unknown auth renders "couldn\'t determine" (never signed-out)', async () => {
    authProbe.mockResolvedValue({
      source: 'unknown',
      reason: '`claude auth status` exited 1',
      last_checked: 0,
    });
    const user = userEvent.setup();
    render(<ClaudeCodeConnect connected onConnect={noop} onDisconnect={noop} />);
    await user.click(screen.getByRole('button', { name: /^Claude Code$/i }));
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => {
      expect(within(dialog).getByText(/Couldn't determine sign-in state/i)).toBeInTheDocument();
    });
    expect(within(dialog).queryByText(/^Not signed in\.$/i)).not.toBeInTheDocument();
  });

  it('modal: missing binary shows the install hint', async () => {
    authProbe.mockResolvedValue({
      source: 'unknown',
      reason: '`claude` CLI not found on PATH',
      last_checked: 0,
    });
    const user = userEvent.setup();
    render(<ClaudeCodeConnect connected onConnect={noop} onDisconnect={noop} />);
    await user.click(screen.getByRole('button', { name: /^Claude Code$/i }));
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => {
      expect(within(dialog).getByText(/Claude Code CLI not found/i)).toBeInTheDocument();
    });
    expect(
      within(dialog).getByText(/npm install -g @anthropic-ai\/claude-code/)
    ).toBeInTheDocument();
  });

  it('modal closes via the close button', async () => {
    const user = userEvent.setup();
    render(<ClaudeCodeConnect connected onConnect={noop} onDisconnect={noop} />);
    await user.click(screen.getByRole('button', { name: /^Claude Code$/i }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: /Close/i }));
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });
  });
});
