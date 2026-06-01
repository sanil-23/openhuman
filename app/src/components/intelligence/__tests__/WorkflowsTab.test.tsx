import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { SkillSummary } from '../../../services/api/skillsApi';
import WorkflowsTab from '../WorkflowsTab';

vi.mock('../../../lib/i18n/I18nContext', () => ({ useT: () => ({ t: (k: string) => k }) }));

// SkillDetailDrawer (rendered when a workflow is opened) uses useNavigate.
vi.mock('react-router-dom', () => ({ useNavigate: () => vi.fn() }));

const seeded = (overrides: Partial<SkillSummary>): SkillSummary => ({
  id: 'wf-1',
  name: 'WF 1',
  description: 'A workflow.',
  version: '0.1.0',
  author: null,
  tags: [],
  tools: [],
  prompts: [],
  location: null,
  resources: [],
  scope: 'user',
  legacy: false,
  warnings: [],
  ...overrides,
});

const listSkills = vi.fn();
vi.mock('../../../services/api/skillsApi', async () => {
  const actual = await vi.importActual<typeof import('../../../services/api/skillsApi')>(
    '../../../services/api/skillsApi'
  );
  return { ...actual, skillsApi: { ...actual.skillsApi, listSkills: () => listSkills() } };
});

describe('WorkflowsTab', () => {
  it('lists workflows from skillsApi with create + install entry points', async () => {
    listSkills.mockResolvedValue([
      seeded({ id: 'user-wf', name: 'User WF', scope: 'user' }),
      seeded({ id: 'project-wf', name: 'Project WF', scope: 'project' }),
    ]);
    render(<WorkflowsTab />);

    await waitFor(() => expect(screen.getByText('User WF')).toBeInTheDocument());
    expect(screen.getByText('Project WF')).toBeInTheDocument();
    expect(screen.getByTestId('workflows-list')).toBeInTheDocument();
    expect(screen.getByTestId('workflow-card-user-wf')).toBeInTheDocument();

    // Create + install entry points live here now (not on Connections).
    expect(screen.getByTestId('workflows-create-btn')).toBeInTheDocument();
    expect(screen.getByTestId('workflows-install-btn')).toBeInTheDocument();
  });

  it('renders the empty state when no workflows are installed', async () => {
    listSkills.mockResolvedValue([]);
    render(<WorkflowsTab />);
    await waitFor(() => expect(screen.getByText('workflows.empty.title')).toBeInTheDocument());
    expect(screen.queryByTestId('workflows-list')).not.toBeInTheDocument();
  });
});
