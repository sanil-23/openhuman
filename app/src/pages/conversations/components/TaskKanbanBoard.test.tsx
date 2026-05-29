import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { TaskBoard, TaskBoardCard } from '../../../types/turnState';
import { TaskKanbanBoard } from './TaskKanbanBoard';

// Echo i18n keys so we can query by the stable key strings.
vi.mock('../../../lib/i18n/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));

function card(partial: Partial<TaskBoardCard>): TaskBoardCard {
  return {
    id: 'c1',
    title: 'Do thing',
    status: 'todo',
    order: 0,
    updatedAt: '',
    ...partial,
  } as TaskBoardCard;
}

function board(cards: TaskBoardCard[]): TaskBoard {
  return { threadId: 't1', cards, updatedAt: '' };
}

describe('TaskKanbanBoard approval surface', () => {
  it('renders Approve/Reject on an awaiting_approval card and calls onDecidePlan', () => {
    const onDecidePlan = vi.fn();
    render(
      <TaskKanbanBoard
        board={board([card({ id: 'a', status: 'awaiting_approval', title: 'Needs approval' })])}
        onDecidePlan={onDecidePlan}
      />
    );

    fireEvent.click(screen.getByTitle('chat.approval.approve'));
    expect(onDecidePlan).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), true);

    fireEvent.click(screen.getByTitle('chat.approval.deny'));
    expect(onDecidePlan).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), false);
  });

  it('buckets ready→todo and rejected→blocked columns so the cards still render', () => {
    render(
      <TaskKanbanBoard
        board={board([
          card({ id: 'r', status: 'ready', title: 'Ready card' }),
          card({ id: 'x', status: 'rejected', title: 'Rejected card' }),
        ])}
      />
    );

    expect(screen.getByText('Ready card')).toBeInTheDocument();
    expect(screen.getByText('Rejected card')).toBeInTheDocument();
    // An approval-flow card without onDecidePlan shows no approve/reject controls.
    expect(screen.queryByTitle('chat.approval.approve')).toBeNull();
  });
});
