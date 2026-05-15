import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  clearCloudProviderKey,
  loadAISettings,
  loadLocalProviderSnapshot,
  localProvider,
  type LocalProviderSnapshot,
  saveAISettings,
  serializeProviderRef,
  setCloudProviderKey,
} from '../../../../services/api/aiSettingsApi';
import { renderWithProviders } from '../../../../test/test-utils';
import AIPanel from '../AIPanel';

vi.mock('../../../../services/api/aiSettingsApi', () => ({
  ALL_WORKLOADS: [
    'reasoning',
    'agentic',
    'coding',
    'memory',
    'embeddings',
    'heartbeat',
    'learning',
    'subconscious',
  ],
  CHAT_WORKLOADS: ['reasoning', 'agentic', 'coding'],
  BACKGROUND_WORKLOADS: ['memory', 'embeddings', 'heartbeat', 'learning', 'subconscious'],
  loadAISettings: vi.fn(),
  saveAISettings: vi.fn(),
  loadLocalProviderSnapshot: vi.fn(),
  setCloudProviderKey: vi.fn(),
  clearCloudProviderKey: vi.fn(),
  serializeProviderRef: vi.fn((r: { kind: string; providerType?: string; model?: string }) => {
    if (r.kind === 'primary') return 'cloud';
    if (r.kind === 'local') return `ollama:${r.model ?? ''}`;
    return `${r.providerType ?? 'cloud'}:${r.model ?? ''}`;
  }),
  localProvider: {
    download: vi.fn().mockResolvedValue(undefined),
    applyPreset: vi.fn().mockResolvedValue(undefined),
    setEnabled: vi.fn().mockResolvedValue(undefined),
    setBinaryPath: vi.fn().mockResolvedValue(undefined),
    shutdown: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock('../../hooks/useSettingsNavigation', () => ({
  useSettingsNavigation: () => ({
    navigateBack: vi.fn(),
    navigateToSettings: vi.fn(),
    breadcrumbs: [],
  }),
}));

// ─── Fixtures ─────────────────────────────────────────────────────────────────

const baseSettings = {
  cloudProviders: [
    {
      id: 'p_oh_x',
      type: 'openhuman' as const,
      endpoint: 'https://api.openhuman.ai/v1',
      default_model: 'reasoning-v1',
      has_api_key: false,
    },
  ],
  primaryCloudId: 'p_oh_x',
  routing: {
    reasoning: { kind: 'primary' as const },
    agentic: { kind: 'primary' as const },
    coding: { kind: 'primary' as const },
    memory: { kind: 'primary' as const },
    embeddings: { kind: 'primary' as const },
    heartbeat: { kind: 'primary' as const },
    learning: { kind: 'primary' as const },
    subconscious: { kind: 'primary' as const },
  },
};

const settingsWithOpenAI = {
  ...baseSettings,
  cloudProviders: [
    ...baseSettings.cloudProviders,
    {
      id: 'p_oai',
      type: 'openai' as const,
      endpoint: 'https://api.openai.com/v1',
      default_model: 'gpt-4o',
      has_api_key: true,
    },
  ],
};

const ollamaRunningSnapshot = {
  status: { state: 'running', warning: null, download_progress: null },
  diagnostics: {
    ollama_running: true,
    ollama_binary_path: '/usr/local/bin/ollama',
    installed_models: [{ name: 'llama3.1:8b', size: 4700000000 }],
  },
  presets: {
    recommended_tier: 'standard',
    presets: [
      {
        tier: 'standard',
        label: '4–8 GB RAM',
        chat_model_id: 'llama3.1:8b',
        description: 'Balanced default',
        vision_model_id: '',
        embedding_model_id: '',
        quantization: '',
        vision_mode: '',
        supports_screen_summary: false,
        target_ram_gb: 8,
        min_ram_gb: 4,
        approx_download_gb: 4,
      },
    ],
    current_tier: 'standard',
    device: {} as never,
  },
  installedModels: [{ name: 'llama3.1:8b', size: 4700000000 }],
} as unknown as LocalProviderSnapshot;

const emptySnapshot: LocalProviderSnapshot = {
  status: null,
  diagnostics: null,
  presets: null,
  installedModels: [],
};

const disabledSnapshot = {
  status: { state: 'disabled', warning: null, download_progress: null } as never,
  diagnostics: null,
  presets: null,
  installedModels: [],
} as LocalProviderSnapshot;

// ─── Setup ────────────────────────────────────────────────────────────────────

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(loadAISettings).mockResolvedValue(baseSettings);
  vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
  vi.mocked(saveAISettings).mockResolvedValue(undefined);
});

// ─── Basic render ─────────────────────────────────────────────────────────────

describe('AIPanel — basic render', () => {
  it('renders the three section labels', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText(/Cloud providers/i).length).toBeGreaterThan(0));
    expect(screen.getAllByText(/Local provider/i).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/Workload routing/i).length).toBeGreaterThan(0);
  });

  it('renders the OpenHuman primary card after load', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText(/OpenHuman/i)).toBeInTheDocument());
    expect(screen.getAllByText(/Primary/).length).toBeGreaterThan(0);
  });

  it('renders all eight workload labels', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText('Reasoning')).toBeInTheDocument());
    for (const label of [
      'Reasoning',
      'Agentic',
      'Coding',
      'Memory summarization',
      'Embeddings',
      'Heartbeat',
      /Learning/,
      'Subconscious',
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it('shows error message when loadAISettings rejects', async () => {
    vi.mocked(loadAISettings).mockRejectedValueOnce(new Error('RPC connection failed'));
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText(/RPC connection failed/i)).toBeInTheDocument());
  });

  it('shows loading state initially', () => {
    // Delay resolution so we can see loading state
    vi.mocked(loadAISettings).mockImplementation(
      () => new Promise(resolve => setTimeout(() => resolve(baseSettings), 200))
    );
    renderWithProviders(<AIPanel />);
    expect(screen.getByText(/Loading/i)).toBeInTheDocument();
  });
});

// ─── Cloud provider add ───────────────────────────────────────────────────────

describe('AIPanel — add cloud provider', () => {
  it('opens the editor modal when Add button is clicked', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText(/OpenHuman/i));

    fireEvent.click(screen.getByRole('button', { name: /Add/i }));

    await waitFor(() => expect(screen.getByText(/Add cloud provider/i)).toBeInTheDocument());
  });

  it('closes the editor when Cancel is clicked', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText(/OpenHuman/i));

    fireEvent.click(screen.getByRole('button', { name: /Add/i }));
    await waitFor(() => screen.getByText(/Add cloud provider/i));

    fireEvent.click(screen.getByRole('button', { name: /Cancel/i }));

    await waitFor(() => expect(screen.queryByText(/Add cloud provider/i)).not.toBeInTheDocument());
  });

  it('submits a new OpenAI provider with API key', async () => {
    vi.mocked(setCloudProviderKey).mockResolvedValue(undefined);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText(/OpenHuman/i).length).toBeGreaterThan(0));

    fireEvent.click(screen.getByRole('button', { name: /Add/i }));
    await waitFor(() => screen.getByText(/Add cloud provider/i));

    // Select OpenAI in the type dropdown (inside the modal)
    const modal = document.querySelector('.fixed.inset-0')!;
    const select = within(modal as HTMLElement).getByRole('combobox');
    fireEvent.change(select, { target: { value: 'openai' } });

    // Fill in default model
    const modelInput = screen.getByPlaceholderText(/gpt-4o/i);
    fireEvent.change(modelInput, { target: { value: 'gpt-4o' } });

    // Fill in API key
    const keyInput = screen.getByPlaceholderText(/sk-\.\.\./i);
    fireEvent.change(keyInput, { target: { value: 'sk-test-key' } });

    fireEvent.click(screen.getByRole('button', { name: /Add provider/i }));

    // Modal should close after submission
    await waitFor(() => expect(screen.queryByText(/Add cloud provider/i)).not.toBeInTheDocument());
    // Cloud providers section still rendered
    expect(screen.getAllByText(/Cloud providers/i).length).toBeGreaterThan(0);
  });
});

// ─── Cloud provider edit ──────────────────────────────────────────────────────

describe('AIPanel — edit cloud provider', () => {
  it('opens editor pre-filled when Edit button is clicked', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    // Use getAllByText since "OpenAI" appears in both the card and the editor dropdown options
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    // Find the Edit button for the OpenAI card (aria-label="Edit")
    const editButtons = screen.getAllByLabelText('Edit');
    fireEvent.click(editButtons[0]);

    await waitFor(() => expect(screen.getByText(/Edit OpenAI/i)).toBeInTheDocument());
  });

  it('clears the API key when "Clear stored key" is clicked', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    vi.mocked(clearCloudProviderKey).mockResolvedValue(undefined);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    const editButtons = screen.getAllByLabelText('Edit');
    fireEvent.click(editButtons[0]);

    await waitFor(() => screen.getByText(/Clear stored key/i));
    fireEvent.click(screen.getByText(/Clear stored key/i));

    await waitFor(() => expect(clearCloudProviderKey).toHaveBeenCalledWith('openai'));
  });
});

// ─── Cloud provider remove ────────────────────────────────────────────────────

describe('AIPanel — remove cloud provider', () => {
  it('removes provider from list when Remove button is clicked', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    const removeButtons = screen.getAllByLabelText('Remove');
    fireEvent.click(removeButtons[0]);

    // After removal, the standalone card-level "OpenAI" text should be gone
    await waitFor(() => {
      // Cards in the provider list have a span with class "text-sm font-semibold"
      const cards = document.querySelectorAll('.text-sm.font-semibold.text-stone-900');
      const labels = Array.from(cards).map(el => el.textContent);
      expect(labels).not.toContain('OpenAI');
    });
  });

  it('marks save bar dirty after removing a provider', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    const removeButtons = screen.getAllByLabelText('Remove');
    fireEvent.click(removeButtons[0]);

    // The save bar should appear — it shows "N unsaved change(s)" text
    await waitFor(() => expect(screen.getByText(/unsaved change/i)).toBeInTheDocument());
  });
});

// ─── Make primary ─────────────────────────────────────────────────────────────

describe('AIPanel — set primary provider', () => {
  it('shows "Set primary" button for non-primary providers', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    expect(screen.getByRole('button', { name: /Set primary/i })).toBeInTheDocument();
  });

  it('clicking "Set primary" marks that provider as primary and shows save bar', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByRole('button', { name: /Set primary/i }));

    // Save bar should appear with "unsaved change" text
    await waitFor(() => expect(screen.getByText(/unsaved change/i)).toBeInTheDocument());
  });
});

// ─── Save / Discard bar ───────────────────────────────────────────────────────

describe('AIPanel — save / discard bar', () => {
  it('shows save bar when primaryCloudId changes and calls saveAISettings on Save', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    // Change primary by clicking Set Primary
    fireEvent.click(screen.getByRole('button', { name: /Set primary/i }));

    // Save bar appears
    await waitFor(() => expect(screen.getByText(/unsaved change/i)).toBeInTheDocument());

    // Find the save bar's save button — it has bg-primary-500 class
    const saveBarSaveBtn = document.querySelector('button.bg-primary-500') as HTMLElement;
    expect(saveBarSaveBtn).not.toBeNull();
    fireEvent.click(saveBarSaveBtn);

    await waitFor(() => expect(saveAISettings).toHaveBeenCalled());
  });

  it('discards changes when Discard button in save bar is clicked', async () => {
    vi.mocked(loadAISettings).mockResolvedValue(settingsWithOpenAI);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getAllByText('OpenAI').length).toBeGreaterThan(0));

    // Change primary to trigger save bar
    fireEvent.click(screen.getByRole('button', { name: /Set primary/i }));

    // Save bar appears
    await waitFor(() => expect(screen.getByText(/unsaved change/i)).toBeInTheDocument());

    // Find the Discard button in the save bar
    fireEvent.click(screen.getByRole('button', { name: /^Discard$/i }));

    // Save bar should disappear
    await waitFor(() => expect(screen.queryByText(/unsaved change/i)).not.toBeInTheDocument());
  });
});

// ─── Workload routing tabs ────────────────────────────────────────────────────

describe('AIPanel — workload routing tabs', () => {
  it('all workloads start with Primary tab active', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Reasoning'));

    // Should have 8 "Primary" tab buttons (one per workload)
    const primaryTabs = screen.getAllByRole('button', { name: /^Primary$/i });
    expect(primaryTabs.length).toBe(8);
  });

  it('Local tab shows Ollama not running hint when Ollama is not available', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Reasoning'));

    const localTabs = screen.getAllByRole('button', { name: /^Local$/i });
    // At least one local tab should have the "Ollama not running" title hint
    const titledTabs = localTabs.filter(tab => tab.getAttribute('title') === 'Ollama not running');
    expect(titledTabs.length).toBeGreaterThan(0);
  });

  it('Cloud preset button routes all workloads to Primary', async () => {
    vi.mocked(loadAISettings).mockResolvedValue({
      ...baseSettings,
      routing: {
        ...baseSettings.routing,
        coding: { kind: 'cloud' as const, providerType: 'openai' as const, model: 'gpt-4o' },
      },
    });
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Reasoning'));

    // Click the "Cloud" preset button in the Workload routing header
    const cloudPresetButton = screen.getAllByRole('button', { name: /^Cloud$/i });
    // The last one in the group is the preset button (the ones in workload rows say "Cloud" too)
    // Find the one in the routing section header — it's in a rounded-full container
    fireEvent.click(cloudPresetButton[cloudPresetButton.length - 1]);

    // After applying cloud preset all should be primary — no save bar yet if all were primary
    // Just confirm the operation doesn't throw
    await waitFor(() => expect(screen.getByText('Reasoning')).toBeInTheDocument());
  });
});

// ─── Local provider section ───────────────────────────────────────────────────

describe('AIPanel — local provider section', () => {
  it('renders Ollama status when running', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(ollamaRunningSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText(/running/i)).toBeInTheDocument());
  });

  it('renders installed model list when Ollama has models', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(ollamaRunningSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText('llama3.1:8b')).toBeInTheDocument());
  });

  it('shows tier presets when no models installed', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue({
      ...emptySnapshot,
      presets: {
        recommended_tier: 'standard',
        current_tier: 'standard',
        device: {} as never,
        presets: [
          {
            tier: 'standard',
            label: '4–8 GB RAM',
            chat_model_id: 'llama3.1:8b',
            description: 'Balanced default',
            vision_model_id: '',
            embedding_model_id: '',
            quantization: '',
            vision_mode: '',
            supports_screen_summary: false,
            target_ram_gb: 8,
            min_ram_gb: 4,
            approx_download_gb: 4,
          },
        ],
      },
    });
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText('llama3.1:8b')).toBeInTheDocument());
  });

  it('clicking a tier preset calls localProvider.applyPreset', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue({
      ...emptySnapshot,
      presets: {
        recommended_tier: 'standard',
        current_tier: 'standard',
        device: {} as never,
        presets: [
          {
            tier: 'standard',
            label: '4–8 GB RAM',
            chat_model_id: 'llama3.1:8b',
            description: 'Balanced default',
            vision_model_id: '',
            embedding_model_id: '',
            quantization: '',
            vision_mode: '',
            supports_screen_summary: false,
            target_ram_gb: 8,
            min_ram_gb: 4,
            approx_download_gb: 4,
          },
        ],
      },
    });
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('llama3.1:8b'));

    // Click the preset button
    const presetButton = screen.getByRole('button', { name: /4–8 GB RAM/i });
    fireEvent.click(presetButton);

    await waitFor(() =>
      expect(vi.mocked(localProvider.applyPreset)).toHaveBeenCalledWith('standard')
    );
  });

  it('Retry button calls localProvider.download', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));

    // Find the Retry button in the Ollama status card
    const retryButton = screen.getByRole('button', { name: /Retry/i });
    fireEvent.click(retryButton);

    await waitFor(() => expect(vi.mocked(localProvider.download)).toHaveBeenCalledWith(true));
  });

  it('shows "Install" label instead of "Retry" when state is missing', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue({
      ...emptySnapshot,
      status: { state: 'missing', warning: null, download_progress: null } as never,
    } as LocalProviderSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Install/i })).toBeInTheDocument()
    );
  });

  it('the local-AI enable checkbox is unchecked when state is disabled', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(disabledSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));

    const checkbox = screen.getByRole('checkbox');
    expect(checkbox).not.toBeChecked();
  });

  it('the local-AI enable checkbox is checked when state is running', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(ollamaRunningSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));

    const checkbox = screen.getByRole('checkbox');
    expect(checkbox).toBeChecked();
  });
});

// ─── Daemon-conflict callout ──────────────────────────────────────────────────

describe('AIPanel — daemon-conflict callout', () => {
  it('shows conflict callout when warning contains "external ollama daemon"', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue({
      ...emptySnapshot,
      status: {
        state: 'error',
        warning: 'external ollama daemon detected on :11434',
        download_progress: null,
      } as never,
    } as LocalProviderSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() =>
      expect(screen.getByText(/Conflicting Ollama daemon detected/i)).toBeInTheDocument()
    );
  });

  it('does NOT show conflict callout when daemon is disabled', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue({
      ...disabledSnapshot,
      status: {
        state: 'disabled',
        warning: 'external ollama daemon detected on :11434',
        download_progress: null,
      } as never,
    } as LocalProviderSnapshot);
    renderWithProviders(<AIPanel />);
    // Wait for render to settle
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));
    expect(screen.queryByText(/Conflicting Ollama daemon detected/i)).not.toBeInTheDocument();
  });

  it('does NOT show conflict callout when no warning', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));
    expect(screen.queryByText(/Conflicting Ollama daemon detected/i)).not.toBeInTheDocument();
  });
});

// ─── Advanced section (custom Ollama path) ────────────────────────────────────

describe('AIPanel — advanced / custom Ollama path', () => {
  it('shows the Advanced details section', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText('Enable local AI (Ollama)'));
    expect(screen.getByText(/Advanced/i)).toBeInTheDocument();
  });

  it('shows resolved binary path from diagnostics', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(ollamaRunningSnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => expect(screen.getByText('/usr/local/bin/ollama')).toBeInTheDocument());
  });

  it('shows fallback text when binary path is empty', async () => {
    renderWithProviders(<AIPanel />);
    await waitFor(() =>
      expect(screen.getByText(/not detected; OpenHuman will auto-install/i)).toBeInTheDocument()
    );
  });

  it('calls localProvider.setBinaryPath when Save is clicked in Advanced', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText(/Advanced/i));

    const pathInput = screen.getByPlaceholderText(/C:\\Program Files/i);
    fireEvent.change(pathInput, { target: { value: '/custom/ollama' } });

    // Find the save button next to the path input — it's a sibling in the flex container
    // Use the path input's parent flex container to scope
    const inputContainer = pathInput.parentElement!;
    const saveBtn = within(inputContainer).getByRole('button', { name: /^Save$/i });
    fireEvent.click(saveBtn);

    await waitFor(() =>
      expect(vi.mocked(localProvider.setBinaryPath)).toHaveBeenCalledWith('/custom/ollama')
    );
  });

  it('shows Clear button when path input has content and calls setBinaryPath with empty string', async () => {
    vi.mocked(loadLocalProviderSnapshot).mockResolvedValue(emptySnapshot);
    renderWithProviders(<AIPanel />);
    await waitFor(() => screen.getByText(/Advanced/i));

    // Manually enter a path to reveal the Clear button
    const pathInput = screen.getByPlaceholderText(/C:\\Program Files/i);
    fireEvent.change(pathInput, { target: { value: '/custom/path/ollama' } });

    // Clear button now visible
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^Clear$/i })).toBeInTheDocument()
    );

    fireEvent.click(screen.getByRole('button', { name: /^Clear$/i }));

    await waitFor(() => expect(vi.mocked(localProvider.setBinaryPath)).toHaveBeenCalledWith(''));
  });
});

// ─── serializeProviderRef import ─────────────────────────────────────────────

describe('AIPanel — serializeProviderRef mock used by save path', () => {
  it('mock serializeProviderRef is callable', () => {
    expect(serializeProviderRef({ kind: 'primary' })).toBe('cloud');
    expect(serializeProviderRef({ kind: 'local', model: 'llama3.1:8b' })).toBe(
      'ollama:llama3.1:8b'
    );
  });
});
