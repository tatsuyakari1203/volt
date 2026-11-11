import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LeftoverRemovalView, LeftoverScanView, LeftoverView } from '../../shared/ipc';
import { LeftoversDialog } from './LeftoversDialog';

const ipc = vi.hoisted(() => ({
  scanLeftovers: vi.fn(),
  removeLeftovers: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const KB = 1024;

function find(over: Partial<LeftoverView> & Pick<LeftoverView, 'id' | 'location'>): LeftoverView {
  return {
    kind: 'folder',
    confidence: 'medium',
    selectedByDefault: false,
    evidence: ['Named exactly like Contoso Editor'],
    evidencePhrases: [
      { code: 'leftover.why.sameName', args: { name: { kind: 'text', value: 'x' } } },
    ],
    sizeBytes: 0,
    items: 0,
    locked: null,
    lockedPhrase: null,
    ...over,
  };
}

const scan: LeftoverScanView = {
  appName: 'Contoso Editor',
  incomplete: false,
  items: [
    find({
      id: 'folder:C:\\Apps\\Contoso',
      location: 'C:\\Apps\\Contoso',
      confidence: 'high',
      selectedByDefault: true,
      sizeBytes: 300 * KB,
      items: 12,
      evidence: ['Recorded as the install folder of Contoso Editor before it was uninstalled'],
    }),
    find({
      id: 'key:HKCU\\Software\\Contoso',
      kind: 'registryKey',
      location: 'HKCU\\Software\\Contoso',
      confidence: 'high',
      selectedByDefault: true,
      items: 3,
      evidence: ['The uninstall entry of Contoso Editor named this key'],
    }),
    find({
      id: 'folder:C:\\Users\\me\\AppData\\Roaming\\Contoso Editor',
      location: 'C:\\Users\\me\\AppData\\Roaming\\Contoso Editor',
      sizeBytes: 10 * KB,
      items: 2,
    }),
    find({
      id: 'key:HKLM\\Software\\Contoso',
      kind: 'registryKey',
      location: 'HKLM\\Software\\Contoso',
      confidence: 'high',
      locked: 'A machine-wide key: removing it needs the administrator helper',
      lockedPhrase: { code: 'leftover.lock.administrator', args: {} },
    }),
  ],
};

const removal: LeftoverRemovalView = {
  removed: [
    {
      id: 'folder:C:\\Apps\\Contoso',
      kind: 'folder',
      location: 'C:\\Apps\\Contoso',
      sizeBytes: 300 * KB,
    },
    { id: 'key:HKCU\\Software\\Contoso', kind: 'registryKey', location: 'x', sizeBytes: 0 },
  ],
  skipped: [],
  removedBytes: 300 * KB,
  historyIncomplete: false,
};

function open() {
  const onClose = vi.fn();
  render(<LeftoversDialog app={{ id: 'win32:m64:x', name: 'Contoso Editor' }} onClose={onClose} />);
  return onClose;
}

beforeEach(() => {
  ipc.scanLeftovers.mockReset();
  ipc.removeLeftovers.mockReset();
  ipc.scanLeftovers.mockResolvedValue(scan);
});

describe('the leftover scan', () => {
  it('shows each find with its confidence and the evidence, and ticks only high confidence', async () => {
    open();
    const dialog = await screen.findByRole('dialog', { name: 'Leftovers of Contoso Editor' });
    expect(ipc.scanLeftovers).toHaveBeenCalledWith('win32:m64:x');
    const rows = Array.from(within(dialog).getByRole('list', { name: 'Leftovers found' }).children);
    expect(rows).toHaveLength(4);
    const install = rows[0] as HTMLElement;
    expect(within(install).getByText('High confidence')).toBeTruthy();
    expect(within(install).getByText(/Recorded as the install folder/)).toBeTruthy();
    expect(within(install).getByText('300.0 KB · 12 files')).toBeTruthy();
    const check = (location: string) =>
      screen.getByRole('checkbox', { name: `Select ${location}` }) as HTMLInputElement;
    expect(check('C:\\Apps\\Contoso').checked).toBe(true);
    expect(check('HKCU\\Software\\Contoso').checked).toBe(true);
    // Below high confidence nothing is ticked.
    expect(check('C:\\Users\\me\\AppData\\Roaming\\Contoso Editor').checked).toBe(false);
    expect(screen.getByText('2 items selected')).toBeTruthy();
  });

  it('shows a machine-wide key as found but locked, with the reason, and cannot tick it', async () => {
    open();
    await screen.findByRole('dialog');
    const locked = screen.getByRole('checkbox', {
      name: 'Select HKLM\\Software\\Contoso',
    }) as HTMLInputElement;
    expect(locked.disabled).toBe(true);
    expect(locked.checked).toBe(false);
    const row = locked.closest('li') as HTMLElement;
    expect(within(row).getByText('Locked')).toBeTruthy();
    expect(within(row).getByText(/needs the administrator helper/)).toBeTruthy();
  });

  it('says so when nothing was found', async () => {
    ipc.scanLeftovers.mockResolvedValue({ ...scan, items: [] });
    open();
    expect(
      await screen.findByText('Nothing was found that Volt can tie to Contoso Editor.'),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Review removal' })).toBeNull();
  });

  it('says why the scan failed', async () => {
    ipc.scanLeftovers.mockRejectedValue(new Error('no record'));
    const onClose = open();
    expect(await screen.findByText('Could not do that: no record')).toBeTruthy();
    await userEvent.setup().click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalled();
  });
});

describe('removing leftovers', () => {
  it('asks again with the sizes and removes only what is ticked', async () => {
    const user = userEvent.setup();
    ipc.removeLeftovers.mockResolvedValue(removal);
    open();
    await screen.findByRole('dialog');
    await user.click(
      screen.getByRole('checkbox', {
        name: 'Select C:\\Users\\me\\AppData\\Roaming\\Contoso Editor',
      }),
    );
    await user.click(screen.getByRole('button', { name: 'Review removal' }));
    const confirm = screen.getByRole('dialog', { name: 'Remove these leftovers?' });
    expect(within(confirm).getByText(/Recycle Bin/)).toBeTruthy();
    const listed = within(within(confirm).getByRole('list', { name: 'Leftovers to remove' }))
      .getAllByRole('listitem')
      .map((item) => item.textContent);
    expect(listed).toHaveLength(3);
    expect(within(confirm).getByText('3 items selected, 310.0 KB in folders.')).toBeTruthy();
    expect(ipc.removeLeftovers).not.toHaveBeenCalled();

    await user.click(within(confirm).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(ipc.removeLeftovers).toHaveBeenCalledTimes(1));
    expect(ipc.removeLeftovers).toHaveBeenCalledWith('win32:m64:x', [
      'folder:C:\\Apps\\Contoso',
      'key:HKCU\\Software\\Contoso',
      'folder:C:\\Users\\me\\AppData\\Roaming\\Contoso Editor',
    ]);
    const done = await screen.findByRole('dialog', { name: 'Leftovers removed' });
    expect(within(done).getByText(/Removed 2 items/)).toBeTruthy();
    expect(within(done).getByText(/restored from the Undo page/)).toBeTruthy();
  });

  it('removes nothing when Back is pressed and the selection stays', async () => {
    const user = userEvent.setup();
    open();
    await screen.findByRole('dialog');
    await user.click(screen.getByRole('button', { name: 'Review removal' }));
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByText('2 items selected')).toBeTruthy();
    expect(ipc.removeLeftovers).not.toHaveBeenCalled();
  });

  it('cannot review with nothing ticked', async () => {
    const user = userEvent.setup();
    open();
    await screen.findByRole('dialog');
    await user.click(screen.getByRole('checkbox', { name: 'Select C:\\Apps\\Contoso' }));
    await user.click(screen.getByRole('checkbox', { name: 'Select HKCU\\Software\\Contoso' }));
    expect(
      (screen.getByRole('button', { name: 'Review removal' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('lists what was left alone, with the reason', async () => {
    const user = userEvent.setup();
    ipc.removeLeftovers.mockResolvedValue({
      ...removal,
      removed: [],
      removedBytes: 0,
      skipped: [
        {
          id: 'folder:C:\\Apps\\Contoso',
          reason: 'It changed since the scan',
          reasonPhrase: { code: 'leftover.skip.changed', args: {} },
        },
      ],
    });
    open();
    await screen.findByRole('dialog');
    await user.click(screen.getByRole('button', { name: 'Review removal' }));
    await user.click(screen.getByRole('button', { name: 'Remove' }));
    const done = await screen.findByRole('dialog', { name: 'Leftovers removed' });
    expect(within(done).getByText('C:\\Apps\\Contoso: It changed since the scan')).toBeTruthy();
  });
});
