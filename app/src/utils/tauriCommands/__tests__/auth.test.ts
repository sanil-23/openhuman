/**
 * Unit tests for the three new auth provider-credential fns added in #1710:
 *   authStoreProviderCredentials
 *   authRemoveProviderCredentials
 *   authListProviderCredentials
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { callCoreRpc } from '../../../services/coreRpcClient';
import {
  authListProviderCredentials,
  authRemoveProviderCredentials,
  authStoreProviderCredentials,
} from '../auth';

vi.mock('../../../services/coreRpcClient', () => ({ callCoreRpc: vi.fn() }));

vi.mock('../common', () => ({ isTauri: vi.fn(() => true), CommandResponse: undefined }));

const mockCallCoreRpc = vi.mocked(callCoreRpc);

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.resetAllMocks();
});

// ─── authStoreProviderCredentials ─────────────────────────────────────────────

describe('authStoreProviderCredentials', () => {
  it('throws when not running in Tauri', async () => {
    const { isTauri } = await import('../common');
    vi.mocked(isTauri).mockReturnValueOnce(false);
    await expect(
      authStoreProviderCredentials({ provider: 'openai', token: 'sk-test' })
    ).rejects.toThrow(/Not running in Tauri/i);
  });

  it('calls the correct RPC method and passes args through', async () => {
    const expected = {
      result: { id: '1', provider: 'openai', profile_name: 'default', kind: 'token' },
      logs: [],
    };
    mockCallCoreRpc.mockResolvedValueOnce(expected as never);

    const result = await authStoreProviderCredentials({
      provider: 'openai',
      profile: 'default',
      token: 'sk-test',
      setActive: true,
    });

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_store_provider_credentials',
      params: { provider: 'openai', profile: 'default', token: 'sk-test', setActive: true },
    });
    expect(result).toEqual(expected);
  });

  it('calls with minimal args (only provider)', async () => {
    mockCallCoreRpc.mockResolvedValueOnce({ result: {}, logs: [] } as never);

    await authStoreProviderCredentials({ provider: 'anthropic' });

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_store_provider_credentials',
      params: { provider: 'anthropic' },
    });
  });

  it('propagates RPC errors', async () => {
    mockCallCoreRpc.mockRejectedValueOnce(new Error('RPC error'));
    await expect(
      authStoreProviderCredentials({ provider: 'openai', token: 'bad-key' })
    ).rejects.toThrow('RPC error');
  });
});

// ─── authRemoveProviderCredentials ────────────────────────────────────────────

describe('authRemoveProviderCredentials', () => {
  it('throws when not running in Tauri', async () => {
    const { isTauri } = await import('../common');
    vi.mocked(isTauri).mockReturnValueOnce(false);
    await expect(authRemoveProviderCredentials({ provider: 'openai' })).rejects.toThrow(
      /Not running in Tauri/i
    );
  });

  it('calls the correct RPC method', async () => {
    const expected = {
      result: { removed: true, provider: 'openai', profile: 'default' },
      logs: [],
    };
    mockCallCoreRpc.mockResolvedValueOnce(expected as never);

    const result = await authRemoveProviderCredentials({ provider: 'openai', profile: 'default' });

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_remove_provider_credentials',
      params: { provider: 'openai', profile: 'default' },
    });
    expect(result).toEqual(expected);
  });

  it('calls without optional profile arg', async () => {
    mockCallCoreRpc.mockResolvedValueOnce({ result: { removed: true }, logs: [] } as never);

    await authRemoveProviderCredentials({ provider: 'anthropic' });

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_remove_provider_credentials',
      params: { provider: 'anthropic' },
    });
  });

  it('propagates RPC errors', async () => {
    mockCallCoreRpc.mockRejectedValueOnce(new Error('remove failed'));
    await expect(authRemoveProviderCredentials({ provider: 'openai' })).rejects.toThrow(
      'remove failed'
    );
  });
});

// ─── authListProviderCredentials ──────────────────────────────────────────────

describe('authListProviderCredentials', () => {
  it('throws when not running in Tauri', async () => {
    const { isTauri } = await import('../common');
    vi.mocked(isTauri).mockReturnValueOnce(false);
    await expect(authListProviderCredentials()).rejects.toThrow(/Not running in Tauri/i);
  });

  it('calls the correct RPC method without provider filter', async () => {
    const expected = {
      result: [
        { id: '1', provider: 'openai', profile_name: 'default', kind: 'token' },
        { id: '2', provider: 'anthropic', profile_name: 'default', kind: 'token' },
      ],
      logs: [],
    };
    mockCallCoreRpc.mockResolvedValueOnce(expected as never);

    const result = await authListProviderCredentials();

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_list_provider_credentials',
      params: {},
    });
    expect(result).toEqual(expected);
  });

  it('passes provider filter when supplied', async () => {
    mockCallCoreRpc.mockResolvedValueOnce({ result: [], logs: [] } as never);

    await authListProviderCredentials('openai');

    expect(mockCallCoreRpc).toHaveBeenCalledWith({
      method: 'openhuman.auth_list_provider_credentials',
      params: { provider: 'openai' },
    });
  });

  it('returns empty list when no profiles match', async () => {
    mockCallCoreRpc.mockResolvedValueOnce({ result: [], logs: [] } as never);
    const result = await authListProviderCredentials('openrouter');
    expect(result.result).toEqual([]);
  });

  it('propagates RPC errors', async () => {
    mockCallCoreRpc.mockRejectedValueOnce(new Error('list failed'));
    await expect(authListProviderCredentials()).rejects.toThrow('list failed');
  });
});
