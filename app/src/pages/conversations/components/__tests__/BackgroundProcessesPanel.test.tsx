import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import type {
  SubagentActivity,
  ToolTimelineEntry,
  ToolTimelineEntryStatus,
} from '../../../../store/chatRuntimeSlice';
import {
  BackgroundProcessesPanel,
  type BackgroundProcess,
  selectBackgroundProcesses,
} from '../BackgroundProcessesPanel';

function sub(partial: Partial<SubagentActivity> & { taskId: string }): SubagentActivity {
  return { agentId: 'researcher', toolCalls: [], ...partial };
}

function entry(
  status: ToolTimelineEntryStatus,
  subagent?: SubagentActivity,
  name = 'subagent:researcher',
): ToolTimelineEntry {
  return { id: `e-${subagent?.taskId ?? name}`, name, round: 0, status, subagent };
}

describe('selectBackgroundProcesses', () => {
  it('keeps only detached (mode==="async") sub-agents', () => {
    const timeline: ToolTimelineEntry[] = [
      entry('running', sub({ taskId: 'sub-async', mode: 'async', prompt: 'research towers' })),
      entry('running', sub({ taskId: 'sub-typed', mode: 'typed', prompt: 'inline work' })),
      entry('success', undefined, 'web_search'), // non-subagent tool row
    ];
    const out = selectBackgroundProcesses(timeline);
    expect(out.map(p => p.taskId)).toEqual(['sub-async']);
    expect(out[0].goal).toBe('research towers');
  });

  it('dedupes by taskId and sorts running first', () => {
    const timeline: ToolTimelineEntry[] = [
      entry('success', sub({ taskId: 'sub-done', mode: 'async' })),
      entry('running', sub({ taskId: 'sub-live', mode: 'async' })),
      entry('running', sub({ taskId: 'sub-live', mode: 'async' })), // duplicate row
    ];
    const out = selectBackgroundProcesses(timeline);
    expect(out.map(p => p.taskId)).toEqual(['sub-live', 'sub-done']); // running first, deduped
  });

  it('derives name, tool count and steps', () => {
    const out = selectBackgroundProcesses([
      entry(
        'running',
        sub({
          taskId: 'sub-1',
          mode: 'async',
          displayName: 'Researcher',
          iterations: 3,
          toolCalls: [
            { callId: 'a', toolName: 't', status: 'success' },
            { callId: 'b', toolName: 't', status: 'success' },
          ],
        }),
      ),
    ]);
    expect(out[0]).toMatchObject({ name: 'Researcher', toolCount: 2, iterations: 3, status: 'running' });
  });
});

describe('BackgroundProcessesPanel', () => {
  const procs: BackgroundProcess[] = [
    { taskId: 'sub-1', name: 'Researcher', goal: 'research the Eiffel Tower', status: 'running', toolCount: 16 },
    { taskId: 'sub-2', name: 'Archivist', goal: 'summarize notes', status: 'success', toolCount: 4 },
  ];

  it('renders nothing when closed', () => {
    const { container } = render(
      <BackgroundProcessesPanel open={false} processes={procs} onClose={vi.fn()} onOpenProcess={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('lists processes and opens one on click', async () => {
    const onOpenProcess = vi.fn();
    render(
      <BackgroundProcessesPanel open processes={procs} onClose={vi.fn()} onOpenProcess={onOpenProcess} />,
    );
    const rows = screen.getAllByTestId('background-process-row');
    expect(rows).toHaveLength(2);
    expect(screen.getByText('Researcher')).toBeInTheDocument();
    expect(screen.getByText('research the Eiffel Tower')).toBeInTheDocument();

    await userEvent.click(rows[0]);
    expect(onOpenProcess).toHaveBeenCalledWith('sub-1');
  });

  it('shows an empty state when there are no background tasks', () => {
    render(<BackgroundProcessesPanel open processes={[]} onClose={vi.fn()} onOpenProcess={vi.fn()} />);
    expect(screen.getByText(/No background tasks in this chat/i)).toBeInTheDocument();
  });
});
