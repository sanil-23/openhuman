import { fireEvent, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, type Mock, vi } from 'vitest';

import { renderWithProviders } from '../../../test/test-utils';
import IntelligenceSettingsTab from '../IntelligenceSettingsTab';

// The orchestrator hits these RPCs on mount; the global tauriCommands mock
// in setup.ts only stubs auth/service helpers, so we extend it here with
// the local-AI surface the Settings tab uses, plus the memory_tree
// LLM-selector RPCs.
//
// Backend selection moved to Settings → Local AI Model → "Memory
// summarizer" checkbox. This tab now only READS `memory_tree.llm_backend`
// on mount via memoryTreeGetLlm to decide whether to render the local-only
// model picker sections; it never WRITES the backend itself. The Memory
// LLM dropdown still calls memoryTreeSetLlm (with extract_model +
// summariser_model fields), so the setter mock stays.
vi.mock('../../../utils/tauriCommands', () => ({
  isTauri: vi.fn(() => true),
  memoryTreeGetLlm: vi.fn(),
  memoryTreeSetLlm: vi.fn(),
  openhumanLocalAiAssetsStatus: vi
    .fn()
    .mockResolvedValue({
      result: {
        chat: { state: 'NotInstalled', id: '', provider: 'ollama' },
        vision: { state: 'NotInstalled', id: '', provider: 'ollama' },
        embedding: { state: 'NotInstalled', id: '', provider: 'ollama' },
        stt: { state: 'NotInstalled', id: '', provider: 'ollama' },
        tts: { state: 'NotInstalled', id: '', provider: 'ollama' },
        quantization: 'q4_k_m',
      },
    }),
  openhumanLocalAiDiagnostics: vi.fn().mockResolvedValue({
    ollama_running: true,
    ollama_binary_path: '/usr/local/bin/ollama',
    installed_models: [
      { name: 'gemma3:1b-it-qat', size: 1_700_000_000, modified_at: null },
      { name: 'bge-m3', size: 1_300_000_000, modified_at: null },
    ],
    expected: {
      chat_model: 'gemma3:1b-it-qat',
      chat_found: true,
      embedding_model: 'bge-m3',
      embedding_found: true,
      vision_model: '',
      vision_found: false,
    },
    issues: [],
    ok: true,
  }),
  openhumanLocalAiStatus: vi
    .fn()
    .mockResolvedValue({
      result: {
        state: 'Ready',
        model_id: 'gemma3:1b-it-qat',
        chat_model_id: 'gemma3:1b-it-qat',
        vision_model_id: '',
        embedding_model_id: 'bge-m3',
        stt_model_id: '',
        tts_voice_id: '',
        quantization: 'q4_k_m',
        vision_state: 'idle',
        vision_mode: 'off',
        embedding_state: 'Ready',
        stt_state: 'idle',
        tts_state: 'idle',
        provider: 'ollama',
        active_backend: 'cpu',
        last_latency_ms: 142,
      },
    }),
  openhumanLocalAiPresets: vi
    .fn()
    .mockResolvedValue({
      presets: [],
      recommended_tier: 'minimal',
      current_tier: 'minimal',
      device: {
        total_ram_bytes: 16_000_000_000,
        cpu_count: 8,
        cpu_brand: 'Test CPU',
        os_name: 'macos',
        os_version: '14',
        has_gpu: false,
        gpu_description: null,
      },
      local_ai_enabled: false,
    }),
  openhumanLocalAiDownloadAsset: vi
    .fn()
    .mockResolvedValue({
      result: {
        chat: { state: 'Ready', id: 'gemma3:1b-it-qat', provider: 'ollama' },
        vision: { state: 'NotInstalled', id: '', provider: 'ollama' },
        embedding: { state: 'Ready', id: 'bge-m3', provider: 'ollama' },
        stt: { state: 'NotInstalled', id: '', provider: 'ollama' },
        tts: { state: 'NotInstalled', id: '', provider: 'ollama' },
        quantization: 'q4_k_m',
      },
    }),
}));

// Pull mocked references after vi.mock() has hoisted. Cast through unknown
// because the import here is the typed wrapper module shape.
const { memoryTreeGetLlm, memoryTreeSetLlm } =
  (await import('../../../utils/tauriCommands')) as unknown as {
    memoryTreeGetLlm: Mock;
    memoryTreeSetLlm: Mock;
  };

describe('IntelligenceSettingsTab', () => {
  // The backend value the mocked memoryTreeGetLlm reports on mount.
  // Tests that need local-mode behavior set this to 'local' before
  // renderWithProviders. There's no in-UI toggle anymore — selection
  // happens via Settings → Local AI Model → Memory summarizer.
  let initialBackend: 'cloud' | 'local';

  beforeEach(() => {
    initialBackend = 'cloud';
    memoryTreeGetLlm.mockReset();
    memoryTreeSetLlm.mockReset();
    memoryTreeGetLlm.mockImplementation(async () => ({ current: initialBackend }));
    // Accept both legacy (bare string) and the new request-object shape.
    memoryTreeSetLlm.mockImplementation(
      async (req: 'cloud' | 'local' | { backend: 'cloud' | 'local' }) => {
        const next = typeof req === 'string' ? req : req.backend;
        return { current: next };
      }
    );
  });

  it('renders the cloud-mode hint when memory tree backend is Cloud', async () => {
    initialBackend = 'cloud';
    renderWithProviders(<IntelligenceSettingsTab />);

    // Hint section appears with the pointer to Local AI Settings.
    await waitFor(() => {
      expect(screen.getByText('Memory model assignment')).toBeInTheDocument();
    });
    expect(screen.getByText(/Memory summarizer/)).toBeInTheDocument();
    // Local-only sections are hidden so cloud users never see Ollama-related UI.
    expect(screen.queryByText('Model assignment')).not.toBeInTheDocument();
    expect(screen.queryByText('Model catalog')).not.toBeInTheDocument();
  });

  it('reveals Model assignment + Catalog when memory tree backend is Local', async () => {
    initialBackend = 'local';
    renderWithProviders(<IntelligenceSettingsTab />);

    await waitFor(() => {
      expect(screen.getByText('Model assignment')).toBeInTheDocument();
    });
    expect(screen.getByText('Model catalog')).toBeInTheDocument();
    // The consolidated Memory LLM dropdown (extract + summarise) is present.
    expect(screen.getByText('Memory LLM')).toBeInTheDocument();
    expect(screen.getByText('Embedder')).toBeInTheDocument();
    // Old separate dropdowns must be absent.
    expect(screen.queryByText('Extract LLM')).not.toBeInTheDocument();
    expect(screen.queryByText('Summariser LLM')).not.toBeInTheDocument();
  });

  it('shows model catalog rows with sizes (in local mode)', async () => {
    initialBackend = 'local';
    renderWithProviders(<IntelligenceSettingsTab />);

    await waitFor(() => {
      expect(screen.getAllByText('qwen2.5:0.5b').length).toBeGreaterThanOrEqual(1);
    });
    expect(screen.getAllByText('gemma3:1b-it-qat').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('gemma3:4b').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('gemma3:12b-it-qat').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('bge-m3').length).toBeGreaterThanOrEqual(1);
    // 3.3 GB is unique to gemma3:4b in the catalog row meta.
    expect(screen.getByText('3.3 GB')).toBeInTheDocument();
  });

  it('renders a Download action for models that are not installed (local mode)', async () => {
    initialBackend = 'local';
    renderWithProviders(<IntelligenceSettingsTab />);

    await waitFor(() => {
      expect(screen.getByText('qwen2.5:0.5b')).toBeInTheDocument();
    });
    const downloadButtons = screen.getAllByRole('button', { name: 'Download' });
    expect(downloadButtons.length).toBeGreaterThanOrEqual(1);
  });

  it('reads memoryTreeGetLlm on mount and never writes the backend from this tab', async () => {
    initialBackend = 'local';
    renderWithProviders(<IntelligenceSettingsTab />);

    // Bootstrap: memoryTreeGetLlm must run once on mount.
    await waitFor(() => {
      expect(memoryTreeGetLlm).toHaveBeenCalled();
    });
    // Wait long enough for any spurious post-mount writes — there should
    // be none, since this tab no longer exposes a backend chooser.
    await waitFor(() => {
      expect(screen.getByText('Model assignment')).toBeInTheDocument();
    });
    // No call to memoryTreeSetLlm with a backend-only payload (the Memory
    // LLM dropdown still calls it on change with extract_model +
    // summariser_model — separate path, exercised by the next test).
    const backendOnlyCalls = memoryTreeSetLlm.mock.calls.filter((args: unknown[]) => {
      const req = args[0];
      if (typeof req === 'string') return true;
      if (req && typeof req === 'object') {
        const obj = req as Record<string, unknown>;
        return 'backend' in obj && !('extract_model' in obj);
      }
      return false;
    });
    expect(backendOnlyCalls).toHaveLength(0);
  });

  it('persists Memory LLM dropdown changes via memoryTreeSetLlm with both extract_model and summariser_model', async () => {
    initialBackend = 'local';
    renderWithProviders(<IntelligenceSettingsTab />);

    await waitFor(() => {
      expect(screen.getByText('Model assignment')).toBeInTheDocument();
    });
    memoryTreeSetLlm.mockClear();

    const memorySelect = screen.getByLabelText(
      'Memory LLM (extract + summarise)'
    ) as HTMLSelectElement;
    fireEvent.change(memorySelect, { target: { value: 'gemma3:12b-it-qat' } });

    await waitFor(() => {
      expect(memoryTreeSetLlm).toHaveBeenCalledWith({
        backend: 'local',
        extract_model: 'gemma3:12b-it-qat',
        summariser_model: 'gemma3:12b-it-qat',
      });
    });
  });
});
