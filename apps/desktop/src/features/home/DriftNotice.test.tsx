import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DriftView } from '../../shared/ipc';
import { DriftNotice } from './DriftNotice';

const ipc = vi.hoisted(() => ({ listTweakDrift: vi.fn() }));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const one: DriftView = { id: 'a', state: { kind: 'notApplied' }, steps: [] };

beforeEach(() => {
  ipc.listTweakDrift.mockReset();
});

describe('the notice about tweaks that changed', () => {
  it('shows nothing when no tweak drifted', async () => {
    ipc.listTweakDrift.mockResolvedValue([]);
    render(<DriftNotice onReview={vi.fn()} />);
    await waitFor(() => expect(ipc.listTweakDrift).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('region')).toBeNull();
  });

  it('shows nothing when the check fails: Home does not depend on it', async () => {
    ipc.listTweakDrift.mockRejectedValue(new Error('boom'));
    render(<DriftNotice onReview={vi.fn()} />);
    await waitFor(() => expect(ipc.listTweakDrift).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('region')).toBeNull();
  });

  it('counts the changed tweaks and leads to the Tweaks page', async () => {
    ipc.listTweakDrift.mockResolvedValue([one, { ...one, id: 'b' }]);
    const onReview = vi.fn();
    render(<DriftNotice onReview={onReview} />);
    const notice = await screen.findByRole('region', { name: 'Tweaks that changed' });
    expect(notice.textContent).toContain('2 tweaks changed since Volt turned them on');
    expect(notice.textContent).toContain('does not affect the score');
    await userEvent.setup().click(screen.getByRole('button', { name: 'Review tweaks' }));
    expect(onReview).toHaveBeenCalledTimes(1);
  });
});
