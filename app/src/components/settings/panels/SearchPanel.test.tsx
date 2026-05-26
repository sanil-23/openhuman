/**
 * Tests for SearchPanel — the "Allowed websites" (web-access firewall) section.
 *
 * Covers: loading the host allowlist into the editor, the "Allow all sites"
 * toggle (persists `allow_all`), and saving an edited host list (persists
 * `allowed_domains` + `allow_all: false`).
 */
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import { renderWithProviders } from '../../../test/test-utils';
import SearchPanel from './SearchPanel';

// ---------------------------------------------------------------------------
// Hoisted mocks
// ---------------------------------------------------------------------------
const hoisted = vi.hoisted(() => ({ getSearchSettings: vi.fn(), updateSearchSettings: vi.fn() }));

vi.mock('../../../utils/tauriCommands/config', () => ({
  openhumanGetSearchSettings: (...a: unknown[]) => hoisted.getSearchSettings(...a),
  openhumanUpdateSearchSettings: (...a: unknown[]) => hoisted.updateSearchSettings(...a),
}));

// Identity translator so we can query by the stable i18n keys.
vi.mock('../../../lib/i18n/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));

vi.mock('../hooks/useSettingsNavigation', () => ({
  useSettingsNavigation: () => ({ navigateBack: vi.fn(), breadcrumbs: [] }),
}));

// Authed (non-local) session so the panel behaves normally.
vi.mock('../../../utils/localSession', () => ({ isLocalSessionToken: () => false }));

function settings(overrides: Record<string, unknown> = {}) {
  return {
    engine: 'managed',
    effective_engine: 'managed',
    max_results: 5,
    timeout_secs: 15,
    parallel_configured: false,
    brave_configured: false,
    allowed_domains: ['reuters.com'],
    allow_all: false,
    ...overrides,
  };
}

describe('SearchPanel — allowed websites', () => {
  beforeEach(() => {
    hoisted.getSearchSettings.mockReset();
    hoisted.updateSearchSettings.mockReset();
    hoisted.getSearchSettings.mockResolvedValue({ result: settings() });
    hoisted.updateSearchSettings.mockResolvedValue({ result: {} });
  });

  test('loads the explicit host list into the editor', async () => {
    renderWithProviders(<SearchPanel embedded />);
    // The textarea mounts empty, then a sync effect fills it from settings on
    // the next tick — wait for the value rather than asserting immediately.
    await waitFor(() => {
      const ta = screen.getByPlaceholderText(
        'settings.search.allowedSitesPlaceholder'
      ) as HTMLTextAreaElement;
      expect(ta.value).toBe('reuters.com');
    });
  });

  test('toggling "Allow all sites" persists allow_all: true', async () => {
    renderWithProviders(<SearchPanel embedded />);
    await screen.findByPlaceholderText('settings.search.allowedSitesPlaceholder');

    const toggle = screen.getByRole('switch', { name: 'settings.search.allowAllAria' });
    expect(toggle).toHaveAttribute('aria-checked', 'false');

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(hoisted.updateSearchSettings).toHaveBeenCalledWith({ allow_all: true })
    );
  });

  test('saving an edited host list persists allowed_domains + allow_all: false', async () => {
    renderWithProviders(<SearchPanel embedded />);
    const textarea = await screen.findByPlaceholderText('settings.search.allowedSitesPlaceholder');

    fireEvent.change(textarea, { target: { value: 'github.com\n  apnews.com  \n\n' } });
    fireEvent.click(screen.getByText('settings.search.allowedSitesSave'));

    await waitFor(() =>
      expect(hoisted.updateSearchSettings).toHaveBeenCalledWith({
        allowed_domains: ['github.com', 'apnews.com'],
        allow_all: false,
      })
    );
  });

  test('hides the editor when allow-all is already on', async () => {
    hoisted.getSearchSettings.mockResolvedValue({
      result: settings({ allowed_domains: ['*'], allow_all: true }),
    });
    renderWithProviders(<SearchPanel embedded />);

    const toggle = await screen.findByRole('switch', { name: 'settings.search.allowAllAria' });
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByPlaceholderText('settings.search.allowedSitesPlaceholder')).toBeNull();
  });
});
