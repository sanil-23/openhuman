import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  analyzeGameplaySession,
  askGameplaySession,
  draftGameplayClipMetadata,
  listGameplaySessions,
  prepareGameplayFrames,
  registerGameplaySession,
  saveGameplayPreset,
} from '../../services/gameplayReviewService';
import { GameplayReviewWorkspace } from './GameplayReviewWorkspace';

vi.mock('../../services/gameplayReviewService', () => ({
  analyzeGameplaySession: vi.fn(),
  askGameplaySession: vi.fn(),
  draftGameplayClipMetadata: vi.fn(),
  flattenClipCandidates: vi.fn((session: any) => session?.analysis?.clip_candidates ?? []),
  flattenDrafts: vi.fn((session: any) => session?.analysis?.draft_metadata ?? []),
  flattenHighlights: vi.fn((session: any) => session?.analysis?.highlights ?? []),
  formatSpoilerMode: vi.fn((mode: string) => (mode === 'full' ? 'Full spoilers' : 'Light spoilers')),
  listGameplaySessions: vi.fn(),
  normalizeGameplayError: vi.fn((error: unknown) => (error instanceof Error ? error.message : String(error))),
  prepareGameplayFrames: vi.fn(),
  registerGameplaySession: vi.fn(),
  saveGameplayPreset: vi.fn(),
}));

const mockedListGameplaySessions = vi.mocked(listGameplaySessions);
const mockedPrepareGameplayFrames = vi.mocked(prepareGameplayFrames);
const mockedRegisterGameplaySession = vi.mocked(registerGameplaySession);
const mockedAnalyzeGameplaySession = vi.mocked(analyzeGameplaySession);
const mockedAskGameplaySession = vi.mocked(askGameplaySession);
const mockedDraftGameplayClipMetadata = vi.mocked(draftGameplayClipMetadata);
const mockedSaveGameplayPreset = vi.mocked(saveGameplayPreset);

describe('GameplayReviewWorkspace', () => {
  beforeEach(() => {
    mockedListGameplaySessions.mockResolvedValue([]);
    mockedPrepareGameplayFrames.mockReset();
    mockedRegisterGameplaySession.mockReset();
    mockedAnalyzeGameplaySession.mockReset();
    mockedAskGameplaySession.mockReset();
    mockedDraftGameplayClipMetadata.mockReset();
    mockedSaveGameplayPreset.mockReset();
  });

  it('imports selected frames, analyzes the session, and asks follow-up questions', async () => {
    mockedPrepareGameplayFrames.mockResolvedValue([
      {
        source_name: 'frame-1.png',
        file_name: 'frame-1.png',
        image_ref: 'data:image/png;base64,AAA',
        captured_at_ms: 123,
      },
    ]);
    mockedRegisterGameplaySession.mockResolvedValue({
      session_id: 'gameplay-apex-1',
      game_id: 'Apex Legends',
      session_title: 'Ranked climb',
      source_label: '/recordings/apex',
      spoiler_mode: 'light',
      preset_id: 'Apex preset',
      imported_at_ms: 1000,
      analyzed_at_ms: null,
      frames: [
        { file_name: 'frame-1.png', image_ref: 'data:image/png;base64,AAA', captured_at_ms: 123 },
      ],
      analysis: null,
    });
    mockedAnalyzeGameplaySession.mockResolvedValue({
      session_id: 'gameplay-apex-1',
      game_id: 'Apex Legends',
      session_title: 'Ranked climb',
      source_label: '/recordings/apex',
      spoiler_mode: 'light',
      preset_id: 'Apex preset',
      imported_at_ms: 1000,
      analyzed_at_ms: 2000,
      frames: [
        { file_name: 'frame-1.png', image_ref: 'data:image/png;base64,AAA', captured_at_ms: 123 },
      ],
      analysis: {
        recap: 'Gameplay recap',
        highlights: [
          {
            id: 'h1',
            frame_index: 0,
            captured_at_ms: 123,
            title: 'Highlight: clutch fight',
            rationale: 'Clean finish',
            confidence: 0.93,
            kind: 'highlight',
          },
        ],
        clip_candidates: [
          {
            id: 'c1',
            frame_index: 0,
            start_label: 'frame-1.png',
            end_label: 'frame-1.png',
            rationale: 'Clean finish',
            confidence: 0.93,
          },
        ],
        draft_metadata: [
          {
            platform: 'twitch',
            title: 'Apex Legends — Highlight: clutch fight (twitch)',
            description: 'Session: Ranked climb',
            tags: ['apex-legends'],
          },
        ],
        follow_up_questions: ['What was the turning point?'],
        spoiler_note: null,
      },
    });
    mockedAskGameplaySession.mockResolvedValue({
      answer: 'Best clip candidate for Ranked climb: Highlight: clutch fight — Clean finish',
      matched_highlights: [],
      suggested_follow_up: ['What was the turning point?'],
    });
    mockedDraftGameplayClipMetadata.mockResolvedValue([
      {
        platform: 'twitch',
        title: 'Apex Legends — Highlight: clutch fight (twitch)',
        description: 'Session: Ranked climb',
        tags: ['apex-legends'],
      },
    ]);
    mockedSaveGameplayPreset.mockResolvedValue({});

    const { container } = render(<GameplayReviewWorkspace />);

    await waitFor(() => expect(screen.getByText('No saved sessions yet.')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Apex Legends'), {
      target: { value: 'Apex Legends' },
    });
    fireEvent.change(screen.getByPlaceholderText('Ranked climb on Friday night'), {
      target: { value: 'Ranked climb' },
    });
    fireEvent.change(screen.getByPlaceholderText('/recordings/apex/night-01'), {
      target: { value: '/recordings/apex' },
    });
    fireEvent.change(screen.getByPlaceholderText('Apex coaching preset'), {
      target: { value: 'Apex preset' },
    });
    fireEvent.change(screen.getByPlaceholderText('aim, positioning, fight selection'), {
      target: { value: 'aim, positioning' },
    });
    fireEvent.change(screen.getByPlaceholderText('What should the reviewer pay attention to?'), {
      target: { value: 'Play clean and keep it spoiler-safe.' },
    });
    fireEvent.change(screen.getByPlaceholderText('twitch,kick,youtube'), {
      target: { value: 'twitch,kick' },
    });

    const fileInput = container.querySelector('input[type="file"]') as HTMLInputElement | null;
    expect(fileInput).not.toBeNull();
    const file = new File([new Uint8Array([1, 2, 3])], 'frame-1.png', { type: 'image/png' });
    Object.defineProperty(file, 'webkitRelativePath', { value: 'session/frame-1.png' });
    fireEvent.change(fileInput as HTMLInputElement, {
      target: { files: [file] },
    });

    fireEvent.click(screen.getByRole('button', { name: /import and analyze/i }));

    await waitFor(() => expect(mockedPrepareGameplayFrames).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText(/Gameplay recap/)).toBeInTheDocument());
    expect(
      screen.getByText('Highlight: clutch fight', {
        selector: 'div.font-medium.text-stone-900',
      })
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /ask session/i }));
    await waitFor(() => expect(mockedAskGameplaySession).toHaveBeenCalled());
    expect(screen.getByText(/Best clip candidate/)).toBeInTheDocument();
  });
});
