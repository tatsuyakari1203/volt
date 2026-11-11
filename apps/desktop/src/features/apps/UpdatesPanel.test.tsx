import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PhraseView, UpdateCheckView, UpdateResultView } from '../../shared/ipc';
import { UpdatesPanel } from './UpdatesPanel';

const ipc = vi.hoisted(() => ({ checkAppUpdates: vi.fn(), updateApp: vi.fn() }));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const phrase = (code: string): PhraseView => ({ code, args: {} });

const found: UpdateCheckView = {
  skipped: 0,
  problem: null,
  updates: [
    {
      id: 'Acme.Editor',
      name: 'Example Editor',
      current: '1.2.3',
      available: '1.3.0',
      source: 'winget',
    },
    {
      id: 'Contoso.Tool',
      name: 'Sample Tool',
      current: '4.0',
      available: '4.1.2',
      source: 'winget',
    },
    { id: 'Fabrikam.Player', name: 'Demo Player', current: '2.0', available: '2.1', source: null },
  ],
};

function result(over: Partial<UpdateResultView> & Pick<UpdateResultView, 'id'>): UpdateResultView {
  return {
    name: over.id,
    from: '1',
    to: '2',
    outcome: 'updated',
    reason: null,
    reasonPhrase: null,
    recorded: true,
    ...over,
  };
}

beforeEach(() => {
  ipc.checkAppUpdates.mockReset();
  ipc.updateApp.mockReset();
});

describe('UpdatesPanel', () => {
  it('says it uses the network and checks nothing until the button is pressed', () => {
    render(<UpdatesPanel />);
    expect(screen.getByText(/uses the internet/i)).toBeTruthy();
    expect(screen.getByText(/never checks in the background/i)).toBeTruthy();
    expect(ipc.checkAppUpdates).not.toHaveBeenCalled();
  });

  it('reports a problem with its fix instead of a list', async () => {
    ipc.checkAppUpdates.mockResolvedValue({
      updates: [],
      skipped: 0,
      problem: {
        message: 'winget was not found on this PC',
        messagePhrase: phrase('updates.problem.missing'),
        fix: 'Install or repair App Installer from the Microsoft Store, then check again.',
        fixPhrase: phrase('updates.fix.missing'),
        agreements: false,
      },
    } satisfies UpdateCheckView);
    render(<UpdatesPanel />);
    await userEvent.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(await screen.findByText('winget was not found on this PC')).toBeTruthy();
    expect(screen.getByText(/Install or repair App Installer/)).toBeTruthy();
    expect(screen.queryByRole('list', { name: 'Available updates' })).toBeNull();
  });

  it('installs the chosen updates one at a time, with a result for each and a stop', async () => {
    ipc.checkAppUpdates.mockResolvedValue(found);
    const release: Array<(value: UpdateResultView) => void> = [];
    ipc.updateApp.mockImplementation(
      () => new Promise<UpdateResultView>((resolve) => release.push(resolve)),
    );
    render(<UpdatesPanel />);
    await userEvent.click(screen.getByRole('button', { name: 'Check for updates' }));
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Select Example Editor' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Select Sample Tool' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Select Demo Player' }));
    await userEvent.click(screen.getByRole('button', { name: 'Update selected (3)' }));

    await waitFor(() => expect(ipc.updateApp).toHaveBeenCalledTimes(1));
    expect(ipc.updateApp).toHaveBeenCalledWith('Acme.Editor');
    await userEvent.click(screen.getByRole('button', { name: 'Stop after this one' }));
    release[0]?.(result({ id: 'Acme.Editor', outcome: 'needsRestart' }));

    expect(await screen.findByText('Updated. Restart Windows to finish.')).toBeTruthy();
    expect(screen.getAllByText('Cancelled')).toHaveLength(2);
    expect(ipc.updateApp).toHaveBeenCalledTimes(1);
  });

  it('shows why an update failed', async () => {
    ipc.checkAppUpdates.mockResolvedValue(found);
    ipc.updateApp.mockResolvedValue(
      result({
        id: 'Acme.Editor',
        outcome: 'failed',
        reason: 'winget or the installer stopped with code 1603',
        reasonPhrase: phrase('updates.fail.code'),
      }),
    );
    render(<UpdatesPanel />);
    await userEvent.click(screen.getByRole('button', { name: 'Check for updates' }));
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Select Example Editor' }));
    await userEvent.click(screen.getByRole('button', { name: 'Update selected (1)' }));
    expect(
      await screen.findByText('Not updated: winget or the installer stopped with code 1603'),
    ).toBeTruthy();
    expect(screen.getByText(/Volt cannot undo an update/)).toBeTruthy();
  });
});
