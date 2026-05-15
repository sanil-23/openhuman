/**
 * Unit tests for aiSettingsApi — the AI-settings façade that sits between
 * AIPanel and the Rust JSON-RPC core.
 *
 * All RPC calls are mocked at the tauriCommands layer so no real IPC is made.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

// ─── Import under test ────────────────────────────────────────────────────────

import {
  authListProviderCredentials,
  authRemoveProviderCredentials,
  authStoreProviderCredentials,
} from '../../../utils/tauriCommands/auth';
import {
  openhumanGetClientConfig,
  openhumanUpdateLocalAiSettings,
  openhumanUpdateModelSettings,
} from '../../../utils/tauriCommands/config';
import {
  openhumanLocalAiApplyPreset,
  openhumanLocalAiDiagnostics,
  openhumanLocalAiDownload,
  openhumanLocalAiPresets,
  openhumanLocalAiSetOllamaPath,
  openhumanLocalAiShutdownOwned,
  openhumanLocalAiStatus,
} from '../../../utils/tauriCommands/localAi';
import {
  type AISettings,
  ALL_WORKLOADS,
  clearCloudProviderKey,
  loadAISettings,
  loadLocalProviderSnapshot,
  localProvider,
  parseProviderString,
  type ProviderRef,
  saveAISettings,
  serializeProviderRef,
  setCloudProviderKey,
  setLocalRuntimeEnabled,
  shutdownLocalProvider,
} from '../aiSettingsApi';

// ─── Mock tauriCommands ───────────────────────────────────────────────────────

vi.mock('../../../utils/tauriCommands/auth', () => ({
  authListProviderCredentials: vi.fn(),
  authStoreProviderCredentials: vi.fn(),
  authRemoveProviderCredentials: vi.fn(),
}));

vi.mock('../../../utils/tauriCommands/config', () => ({
  openhumanGetClientConfig: vi.fn(),
  openhumanUpdateModelSettings: vi.fn(),
  openhumanUpdateLocalAiSettings: vi.fn(),
}));

vi.mock('../../../utils/tauriCommands/localAi', () => ({
  openhumanLocalAiStatus: vi.fn(),
  openhumanLocalAiDiagnostics: vi.fn(),
  openhumanLocalAiPresets: vi.fn(),
  openhumanLocalAiApplyPreset: vi.fn(),
  openhumanLocalAiDownload: vi.fn(),
  openhumanLocalAiSetOllamaPath: vi.fn(),
  openhumanLocalAiShutdownOwned: vi.fn(),
}));

// ─── Typed mock helpers ───────────────────────────────────────────────────────

const mockGetClientConfig = vi.mocked(openhumanGetClientConfig);
const mockAuthList = vi.mocked(authListProviderCredentials);
const mockAuthStore = vi.mocked(authStoreProviderCredentials);
const mockAuthRemove = vi.mocked(authRemoveProviderCredentials);
const mockUpdateModelSettings = vi.mocked(openhumanUpdateModelSettings);
const mockUpdateLocalAiSettings = vi.mocked(openhumanUpdateLocalAiSettings);
const mockLocalAiStatus = vi.mocked(openhumanLocalAiStatus);
const mockLocalAiDiagnostics = vi.mocked(openhumanLocalAiDiagnostics);
const mockLocalAiPresets = vi.mocked(openhumanLocalAiPresets);
const mockLocalAiApplyPreset = vi.mocked(openhumanLocalAiApplyPreset);
const mockLocalAiDownload = vi.mocked(openhumanLocalAiDownload);
const mockSetOllamaPath = vi.mocked(openhumanLocalAiSetOllamaPath);
const mockShutdownOwned = vi.mocked(openhumanLocalAiShutdownOwned);

// ─── Base fixtures ────────────────────────────────────────────────────────────

const baseClientConfig = {
  cloud_providers: [
    { id: 'p_oh', type: 'openhuman', endpoint: 'https://api.openhuman.ai/v1', default_model: '' },
    { id: 'p_oai', type: 'openai', endpoint: 'https://api.openai.com/v1', default_model: 'gpt-4o' },
  ],
  primary_cloud: 'p_oh',
  reasoning_provider: 'cloud',
  agentic_provider: 'cloud',
  coding_provider: 'openai:gpt-4o',
  memory_provider: 'ollama:llama3.1:8b',
  embeddings_provider: '',
  heartbeat_provider: null,
  learning_provider: 'openrouter:mistral-7b',
  subconscious_provider: 'openhuman',
};

beforeEach(() => {
  vi.clearAllMocks();
});

// ─── parseProviderString ──────────────────────────────────────────────────────

describe('parseProviderString', () => {
  it('returns primary for null', () => {
    expect(parseProviderString(null)).toEqual({ kind: 'primary' });
  });

  it('returns primary for empty string', () => {
    expect(parseProviderString('')).toEqual({ kind: 'primary' });
  });

  it('returns primary for "cloud" sentinel', () => {
    expect(parseProviderString('cloud')).toEqual({ kind: 'primary' });
  });

  it('parses ollama prefix', () => {
    expect(parseProviderString('ollama:llama3.1:8b')).toEqual({
      kind: 'local',
      model: 'llama3.1:8b',
    });
  });

  it('parses bare "openhuman" without trailing colon', () => {
    expect(parseProviderString('openhuman')).toEqual({
      kind: 'cloud',
      providerType: 'openhuman',
      model: '',
    });
  });

  it('parses "openai:gpt-4o"', () => {
    expect(parseProviderString('openai:gpt-4o')).toEqual({
      kind: 'cloud',
      providerType: 'openai',
      model: 'gpt-4o',
    });
  });

  it('parses "anthropic:claude-3-opus"', () => {
    expect(parseProviderString('anthropic:claude-3-opus')).toEqual({
      kind: 'cloud',
      providerType: 'anthropic',
      model: 'claude-3-opus',
    });
  });

  it('parses "openrouter:mistral-7b"', () => {
    expect(parseProviderString('openrouter:mistral-7b')).toEqual({
      kind: 'cloud',
      providerType: 'openrouter',
      model: 'mistral-7b',
    });
  });

  it('parses "custom:some-model"', () => {
    expect(parseProviderString('custom:some-model')).toEqual({
      kind: 'cloud',
      providerType: 'custom',
      model: 'some-model',
    });
  });

  it('falls back to primary for unrecognised prefix', () => {
    expect(parseProviderString('unknown:xyz')).toEqual({ kind: 'primary' });
  });

  it('trims whitespace', () => {
    expect(parseProviderString('  openai:gpt-4o  ')).toEqual({
      kind: 'cloud',
      providerType: 'openai',
      model: 'gpt-4o',
    });
  });
});

// ─── serializeProviderRef ─────────────────────────────────────────────────────

describe('serializeProviderRef', () => {
  it('serializes primary to "cloud"', () => {
    const ref: ProviderRef = { kind: 'primary' };
    expect(serializeProviderRef(ref)).toBe('cloud');
  });

  it('serializes local ref', () => {
    const ref: ProviderRef = { kind: 'local', model: 'llama3.1:8b' };
    expect(serializeProviderRef(ref)).toBe('ollama:llama3.1:8b');
  });

  it('serializes cloud ref with model', () => {
    const ref: ProviderRef = { kind: 'cloud', providerType: 'openai', model: 'gpt-4o' };
    expect(serializeProviderRef(ref)).toBe('openai:gpt-4o');
  });

  it('serializes bare openhuman cloud ref (no model) as sentinel', () => {
    const ref: ProviderRef = { kind: 'cloud', providerType: 'openhuman', model: '' };
    expect(serializeProviderRef(ref)).toBe('openhuman');
  });

  it('serializes openhuman cloud ref WITH model using colon form', () => {
    const ref: ProviderRef = { kind: 'cloud', providerType: 'openhuman', model: 'reasoning-v1' };
    expect(serializeProviderRef(ref)).toBe('openhuman:reasoning-v1');
  });

  it('round-trips: parse then serialize gives same string', () => {
    const cases = ['cloud', 'openai:gpt-4o', 'ollama:llama3.1:8b', 'anthropic:claude-3-5-sonnet'];
    for (const s of cases) {
      expect(serializeProviderRef(parseProviderString(s))).toBe(s);
    }
  });
});

// ─── ALL_WORKLOADS export ─────────────────────────────────────────────────────

describe('ALL_WORKLOADS', () => {
  it('contains all 8 workload ids', () => {
    expect(ALL_WORKLOADS).toHaveLength(8);
    expect(ALL_WORKLOADS).toContain('reasoning');
    expect(ALL_WORKLOADS).toContain('agentic');
    expect(ALL_WORKLOADS).toContain('coding');
    expect(ALL_WORKLOADS).toContain('memory');
    expect(ALL_WORKLOADS).toContain('embeddings');
    expect(ALL_WORKLOADS).toContain('heartbeat');
    expect(ALL_WORKLOADS).toContain('learning');
    expect(ALL_WORKLOADS).toContain('subconscious');
  });
});

// ─── loadAISettings ───────────────────────────────────────────────────────────

describe('loadAISettings', () => {
  it('joins config + auth profiles and derives has_api_key', async () => {
    mockGetClientConfig.mockResolvedValue({ result: baseClientConfig } as never);
    mockAuthList.mockResolvedValue({
      result: [{ id: '1', provider: 'openai', profile_name: 'default', kind: 'token' }],
    } as never);

    const settings = await loadAISettings();

    expect(settings.cloudProviders).toHaveLength(2);
    const oh = settings.cloudProviders.find(p => p.type === 'openhuman')!;
    const oai = settings.cloudProviders.find(p => p.type === 'openai')!;
    expect(oh.has_api_key).toBe(false);
    expect(oai.has_api_key).toBe(true);
    expect(settings.primaryCloudId).toBe('p_oh');
  });

  it('degrades gracefully when authListProviderCredentials throws', async () => {
    mockGetClientConfig.mockResolvedValue({ result: baseClientConfig } as never);
    mockAuthList.mockRejectedValue(new Error('no profiles file'));

    const settings = await loadAISettings();

    // All providers get has_api_key: false — panel still renders.
    expect(settings.cloudProviders.every(p => !p.has_api_key)).toBe(true);
  });

  it('correctly parses mixed routing strings', async () => {
    mockGetClientConfig.mockResolvedValue({ result: baseClientConfig } as never);
    mockAuthList.mockResolvedValue({ result: [] } as never);

    const settings = await loadAISettings();

    expect(settings.routing.reasoning).toEqual({ kind: 'primary' });
    expect(settings.routing.coding).toEqual({
      kind: 'cloud',
      providerType: 'openai',
      model: 'gpt-4o',
    });
    expect(settings.routing.memory).toEqual({ kind: 'local', model: 'llama3.1:8b' });
    expect(settings.routing.learning).toEqual({
      kind: 'cloud',
      providerType: 'openrouter',
      model: 'mistral-7b',
    });
    expect(settings.routing.subconscious).toEqual({
      kind: 'cloud',
      providerType: 'openhuman',
      model: '',
    });
  });

  it('throws when openhumanGetClientConfig rejects', async () => {
    mockGetClientConfig.mockRejectedValue(new Error('RPC failed'));
    mockAuthList.mockResolvedValue({ result: [] } as never);

    await expect(loadAISettings()).rejects.toThrow('RPC failed');
  });
});

// ─── saveAISettings ───────────────────────────────────────────────────────────

describe('saveAISettings', () => {
  const makeSettings = (overrides: Partial<AISettings> = {}): AISettings => ({
    cloudProviders: [
      {
        id: 'p_oh',
        type: 'openhuman',
        endpoint: 'https://api.openhuman.ai/v1',
        default_model: '',
        has_api_key: false,
      },
    ],
    primaryCloudId: 'p_oh',
    routing: {
      reasoning: { kind: 'primary' },
      agentic: { kind: 'primary' },
      coding: { kind: 'primary' },
      memory: { kind: 'primary' },
      embeddings: { kind: 'primary' },
      heartbeat: { kind: 'primary' },
      learning: { kind: 'primary' },
      subconscious: { kind: 'primary' },
    },
    ...overrides,
  });

  it('is a no-op when nothing changed', async () => {
    mockUpdateModelSettings.mockResolvedValue(undefined as never);
    const s = makeSettings();
    await saveAISettings(s, s);
    expect(mockUpdateModelSettings).not.toHaveBeenCalled();
  });

  it('sends cloud_providers patch when list changes', async () => {
    mockUpdateModelSettings.mockResolvedValue(undefined as never);
    const prev = makeSettings();
    const next = makeSettings({
      cloudProviders: [
        ...prev.cloudProviders,
        {
          id: 'p_oai',
          type: 'openai',
          endpoint: 'https://api.openai.com/v1',
          default_model: 'gpt-4o',
          has_api_key: false,
        },
      ],
    });

    await saveAISettings(prev, next);

    expect(mockUpdateModelSettings).toHaveBeenCalledOnce();
    const patch = mockUpdateModelSettings.mock.calls[0][0];
    expect(patch.cloud_providers).toHaveLength(2);
    // has_api_key should be stripped from the wire payload
    expect(patch.cloud_providers![0]).not.toHaveProperty('has_api_key');
  });

  it('sends primary_cloud patch when primaryCloudId changes', async () => {
    mockUpdateModelSettings.mockResolvedValue(undefined as never);
    const prev = makeSettings();
    const next = makeSettings({ primaryCloudId: 'p_other' });

    await saveAISettings(prev, next);

    const patch = mockUpdateModelSettings.mock.calls[0][0];
    expect(patch.primary_cloud).toBe('p_other');
  });

  it('sends provider string patch when routing row changes', async () => {
    mockUpdateModelSettings.mockResolvedValue(undefined as never);
    const prev = makeSettings();
    const next = makeSettings({
      routing: {
        ...prev.routing,
        coding: { kind: 'cloud', providerType: 'openai', model: 'gpt-4o' },
      },
    });

    await saveAISettings(prev, next);

    const patch = mockUpdateModelSettings.mock.calls[0][0];
    expect(patch.coding_provider).toBe('openai:gpt-4o');
  });

  it('sends empty-string for primary_cloud when next.primaryCloudId is null', async () => {
    mockUpdateModelSettings.mockResolvedValue(undefined as never);
    const prev = makeSettings({ primaryCloudId: 'p_oh' });
    const next = makeSettings({ primaryCloudId: null as unknown as string });

    await saveAISettings(prev, next);

    const patch = mockUpdateModelSettings.mock.calls[0][0];
    expect(patch.primary_cloud).toBe('');
  });

  it('propagates errors from openhumanUpdateModelSettings', async () => {
    mockUpdateModelSettings.mockRejectedValue(new Error('save failed'));
    const prev = makeSettings();
    const next = makeSettings({ primaryCloudId: 'p_other' });

    await expect(saveAISettings(prev, next)).rejects.toThrow('save failed');
  });
});

// ─── setCloudProviderKey ──────────────────────────────────────────────────────

describe('setCloudProviderKey', () => {
  it('calls authStoreProviderCredentials with correct args', async () => {
    mockAuthStore.mockResolvedValue({ result: {}, logs: [] } as never);

    await setCloudProviderKey('openai', 'sk-test');

    expect(mockAuthStore).toHaveBeenCalledWith({
      provider: 'openai',
      profile: 'default',
      token: 'sk-test',
      setActive: true,
    });
  });

  it('throws for openhuman provider type', async () => {
    await expect(setCloudProviderKey('openhuman', 'tok')).rejects.toThrow(
      /keys are not configurable/i
    );
    expect(mockAuthStore).not.toHaveBeenCalled();
  });

  it('propagates authStoreProviderCredentials error', async () => {
    mockAuthStore.mockRejectedValue(new Error('store failed'));
    await expect(setCloudProviderKey('anthropic', 'sk-abc')).rejects.toThrow('store failed');
  });
});

// ─── clearCloudProviderKey ────────────────────────────────────────────────────

describe('clearCloudProviderKey', () => {
  it('calls authRemoveProviderCredentials', async () => {
    mockAuthRemove.mockResolvedValue({
      result: { removed: true, provider: 'openai', profile: 'default' },
      logs: [],
    } as never);

    await clearCloudProviderKey('openai');

    expect(mockAuthRemove).toHaveBeenCalledWith({ provider: 'openai', profile: 'default' });
  });

  it('is a no-op for openhuman', async () => {
    await clearCloudProviderKey('openhuman');
    expect(mockAuthRemove).not.toHaveBeenCalled();
  });

  it('propagates authRemoveProviderCredentials error', async () => {
    mockAuthRemove.mockRejectedValue(new Error('remove failed'));
    await expect(clearCloudProviderKey('anthropic')).rejects.toThrow('remove failed');
  });
});

// ─── setLocalRuntimeEnabled ───────────────────────────────────────────────────

describe('setLocalRuntimeEnabled', () => {
  it('calls openhumanUpdateLocalAiSettings with both flags true', async () => {
    mockUpdateLocalAiSettings.mockResolvedValue(undefined as never);

    await setLocalRuntimeEnabled(true);

    expect(mockUpdateLocalAiSettings).toHaveBeenCalledWith({
      runtime_enabled: true,
      opt_in_confirmed: true,
    });
  });

  it('calls openhumanUpdateLocalAiSettings with both flags false', async () => {
    mockUpdateLocalAiSettings.mockResolvedValue(undefined as never);

    await setLocalRuntimeEnabled(false);

    expect(mockUpdateLocalAiSettings).toHaveBeenCalledWith({
      runtime_enabled: false,
      opt_in_confirmed: false,
    });
  });

  it('propagates errors', async () => {
    mockUpdateLocalAiSettings.mockRejectedValue(new Error('update failed'));
    await expect(setLocalRuntimeEnabled(true)).rejects.toThrow('update failed');
  });
});

// ─── shutdownLocalProvider ────────────────────────────────────────────────────

describe('shutdownLocalProvider', () => {
  it('disables runtime then shuts down owned process', async () => {
    mockUpdateLocalAiSettings.mockResolvedValue(undefined as never);
    mockShutdownOwned.mockResolvedValue(undefined as never);

    await shutdownLocalProvider();

    expect(mockUpdateLocalAiSettings).toHaveBeenCalledWith({
      runtime_enabled: false,
      opt_in_confirmed: false,
    });
    expect(mockShutdownOwned).toHaveBeenCalled();
  });

  it('propagates error from setLocalRuntimeEnabled', async () => {
    mockUpdateLocalAiSettings.mockRejectedValue(new Error('disable failed'));
    await expect(shutdownLocalProvider()).rejects.toThrow('disable failed');
    // shutdownOwned should NOT be called when disable fails
    expect(mockShutdownOwned).not.toHaveBeenCalled();
  });
});

// ─── loadLocalProviderSnapshot ────────────────────────────────────────────────

describe('loadLocalProviderSnapshot', () => {
  const statusResult = { state: 'running', warning: null, download_progress: null };
  const diagResult = {
    ollama_running: true,
    ollama_binary_path: '/usr/local/bin/ollama',
    installed_models: [{ name: 'llama3.1:8b', size: 4700000000 }],
  };
  const presetsResult = {
    recommended_tier: 'standard',
    presets: [
      { tier: 'standard', label: 'Balanced', chat_model_id: 'llama3.1:8b', description: '' },
    ],
  };

  it('returns full snapshot when all calls succeed', async () => {
    mockLocalAiStatus.mockResolvedValue({ result: statusResult } as never);
    mockLocalAiDiagnostics.mockResolvedValue(diagResult as never);
    mockLocalAiPresets.mockResolvedValue(presetsResult as never);

    const snap = await loadLocalProviderSnapshot();

    expect(snap.status).toEqual(statusResult);
    expect(snap.diagnostics).toEqual(diagResult);
    expect(snap.presets).toEqual(presetsResult);
    expect(snap.installedModels).toEqual([{ name: 'llama3.1:8b', size: 4700000000 }]);
  });

  it('gracefully handles status failure (null status)', async () => {
    mockLocalAiStatus.mockRejectedValue(new Error('not available'));
    mockLocalAiDiagnostics.mockResolvedValue(diagResult as never);
    mockLocalAiPresets.mockResolvedValue(presetsResult as never);

    const snap = await loadLocalProviderSnapshot();

    expect(snap.status).toBeNull();
    expect(snap.installedModels).toEqual([{ name: 'llama3.1:8b', size: 4700000000 }]);
  });

  it('gracefully handles diagnostics failure (empty installedModels)', async () => {
    mockLocalAiStatus.mockResolvedValue({ result: statusResult } as never);
    mockLocalAiDiagnostics.mockRejectedValue(new Error('diag failed'));
    mockLocalAiPresets.mockResolvedValue(presetsResult as never);

    const snap = await loadLocalProviderSnapshot();

    expect(snap.diagnostics).toBeNull();
    expect(snap.installedModels).toEqual([]);
  });

  it('gracefully handles presets failure (null presets)', async () => {
    mockLocalAiStatus.mockResolvedValue({ result: statusResult } as never);
    mockLocalAiDiagnostics.mockResolvedValue(diagResult as never);
    mockLocalAiPresets.mockRejectedValue(new Error('presets failed'));

    const snap = await loadLocalProviderSnapshot();

    expect(snap.presets).toBeNull();
  });

  it('all three can fail simultaneously', async () => {
    mockLocalAiStatus.mockRejectedValue(new Error('a'));
    mockLocalAiDiagnostics.mockRejectedValue(new Error('b'));
    mockLocalAiPresets.mockRejectedValue(new Error('c'));

    const snap = await loadLocalProviderSnapshot();

    expect(snap.status).toBeNull();
    expect(snap.diagnostics).toBeNull();
    expect(snap.presets).toBeNull();
    expect(snap.installedModels).toEqual([]);
  });
});

// ─── localProvider namespace ──────────────────────────────────────────────────

describe('localProvider', () => {
  it('localProvider.download delegates to openhumanLocalAiDownload', async () => {
    mockLocalAiDownload.mockResolvedValue(undefined as never);
    await localProvider.download(true);
    expect(mockLocalAiDownload).toHaveBeenCalledWith(true);
  });

  it('localProvider.applyPreset delegates to openhumanLocalAiApplyPreset', async () => {
    mockLocalAiApplyPreset.mockResolvedValue(undefined as never);
    await localProvider.applyPreset('standard');
    expect(mockLocalAiApplyPreset).toHaveBeenCalledWith('standard');
  });

  it('localProvider.setEnabled delegates to setLocalRuntimeEnabled', async () => {
    mockUpdateLocalAiSettings.mockResolvedValue(undefined as never);
    await localProvider.setEnabled(false);
    expect(mockUpdateLocalAiSettings).toHaveBeenCalledWith({
      runtime_enabled: false,
      opt_in_confirmed: false,
    });
  });

  it('localProvider.setBinaryPath delegates to openhumanLocalAiSetOllamaPath', async () => {
    mockSetOllamaPath.mockResolvedValue(undefined as never);
    await localProvider.setBinaryPath('/usr/local/bin/ollama');
    expect(mockSetOllamaPath).toHaveBeenCalledWith('/usr/local/bin/ollama');
  });

  it('localProvider.shutdown disables runtime and shuts down owned process', async () => {
    mockUpdateLocalAiSettings.mockResolvedValue(undefined as never);
    mockShutdownOwned.mockResolvedValue(undefined as never);
    await localProvider.shutdown();
    expect(mockShutdownOwned).toHaveBeenCalled();
  });
});
