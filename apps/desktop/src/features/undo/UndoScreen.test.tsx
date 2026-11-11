import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { ActivityItem, ActivityPage, UndoResult } from '../../shared/ipc';
import { IpcCommandError, IpcUnavailableError } from '../../shared/ipc';
import { UndoScreen } from './UndoScreen';

const ipc = vi.hoisted(() => ({
  listActivity: vi.fn(),
  undoEntry: vi.fn(),
  undoBatch: vi.fn(),
  undoAdministratorChange: vi.fn(),
  previewUndoBatch: vi.fn(),
  previewUndoEntry: vi.fn(),
  historyStatus: vi.fn(),
  exportActivity: vi.fn(),
  exportActivityText: vi.fn(),
  clearHistory: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const FLAG = 'Applied registry value HKCU\\X\\Flag = REG_DWORD 0';
const PRE = 'Applied registry value HKCU\\X\\Pre = REG_DWORD 1';

function item(id: string, over: Partial<ActivityItem> = {}): ActivityItem {
  return {
    id,
    time: '2026-11-10T03:01:00.123Z',
    source: 'manual',
    status: 'applied',
    summary: FLAG,
    summaryPhrase: null,
    label: 'show-file-extensions',
    title: 'Show file extensions',
    batch: null,
    entry: null,
    previous: null,
    previousPhrase: null,
    state: 'cannotUndo',
    reason: null,
    reasonPhrase: null,
    administrator: false,
    administratorTweak: null,
    ...over,
  };
}

/** A change that was made and can be undone. */
function undoable(id: string, over: Partial<ActivityItem> = {}): ActivityItem {
  return item(id, { entry: id, state: 'canUndo', previous: '(not set)', ...over });
}

function page(items: ActivityItem[], nextCursor: string | null = null): ActivityPage {
  return { items, nextCursor };
}

function result(over: Partial<UndoResult> = {}): UndoResult {
  return { status: 'restored', restored: 1, failed: 0, failures: [], overwrites: [], ...over };
}

/** A promise the test settles by hand, to observe the state while a command runs. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** The rows of the change list, groups flattened away. */
function rows() {
  // A plain DOM query: role queries walk the accessibility tree and are slow with hundreds of rows.
  return Array.from(document.querySelectorAll('li')).filter(
    (row) => row.querySelector('time') && !row.querySelector('ul'),
  );
}

beforeEach(() => {
  for (const mock of Object.values(ipc)) mock.mockReset();
  ipc.historyStatus.mockResolvedValue({ incomplete: false });
  ipc.previewUndoEntry.mockResolvedValue({ items: [], overwrites: [] });
});

// ---- reading ---------------------------------------------------------------------------------

it('shows a loading state, then every record newest first with time, source, what changed and the previous value', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      item('2', {
        status: 'reverted',
        state: 'restored',
        summary: 'Reverted registry value HKCU\\X\\Flag, restored (not set)',
      }),
      undoable('1', { source: 'automation', previous: 'REG_DWORD 5' }),
    ]),
  );
  render(<UndoScreen />);
  expect(screen.getByRole('status').textContent).toContain('Reading the history');

  await screen.findByRole('list', { name: 'Changes' });
  const [newest, older] = rows();
  // Newest first: the order the shell sent is the order shown.
  expect(newest?.textContent).toContain('Undone');
  expect(newest?.textContent).toContain('restored (not set)');
  expect(older?.textContent).toContain('Applied');
  expect(older?.textContent).toContain('Automation');
  expect(older?.textContent).toContain(FLAG);
  expect(older?.textContent).toContain('Previous value: REG_DWORD 5');
  const time = older?.querySelector('time');
  expect(time?.getAttribute('datetime')).toBe('2026-11-10T03:01:00.123Z');
  expect(time?.textContent).not.toBe('');
  expect(ipc.listActivity).toHaveBeenCalledWith(50);
});

it('shows each row state: can be undone, already undone, failed, cannot be undone with its reason, and an undo record', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      item('5', { status: 'rolledBack', state: 'restored', summary: 'Rolled back one' }),
      item('4', {
        summary: 'Moved 3 files to the Recycle Bin',
        label: null,
        title: null,
        reason: 'Volt saved no earlier value for this action, so it cannot restore it.',
      }),
      item('3', { status: 'failed', state: 'failed', summary: 'Failed to apply two' }),
      item('2', { entry: '2', state: 'undone', previous: 'REG_DWORD 1', summary: 'Applied three' }),
      undoable('1', { summary: 'Applied four' }),
    ]),
  );
  render(<UndoScreen />);
  await screen.findByRole('list', { name: 'Changes' });
  const [restored, cannot, failed, undone, can] = rows();

  // An undo record is not itself undoable and carries no badge.
  expect(restored?.textContent).toContain('Rolled back');
  expect(restored?.textContent).not.toContain('Can be undone');
  expect(within(restored as HTMLElement).queryByRole('button')).toBeNull();

  expect(cannot?.textContent).toContain('Cannot be undone');
  expect(cannot?.textContent).toContain('Volt saved no earlier value for this action');
  expect(within(cannot as HTMLElement).queryByRole('button')).toBeNull();

  expect(failed?.textContent).toContain('Failed');
  expect(within(failed as HTMLElement).queryByRole('button')).toBeNull();

  expect(undone?.textContent).toContain('Already undone');
  expect(undone?.textContent).toContain('Previous value: REG_DWORD 1');
  expect(within(undone as HTMLElement).queryByRole('button')).toBeNull();

  expect(can?.textContent).toContain('Can be undone');
  expect(
    within(can as HTMLElement).getByRole('button', { name: 'Undo this change: Applied four' }),
  ).toBeTruthy();
});

it('groups the records of one batch under the tweak name, with Undo all only while something is undoable', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      undoable('4', { batch: '8', summary: PRE }),
      undoable('3', { batch: '8', summary: FLAG }),
      item('2', { entry: '2', state: 'undone', batch: '7', summary: 'Older one', title: 'Other' }),
      item('1', { entry: '1', state: 'undone', batch: '7', summary: 'Older two', title: 'Other' }),
    ]),
  );
  render(<UndoScreen />);
  await screen.findByRole('list', { name: 'Changes' });

  const live = screen.getByRole('list', { name: 'Show file extensions: records' });
  expect(within(live).getAllByRole('listitem')).toHaveLength(2);
  expect(screen.getAllByText('2 records in one batch')).toHaveLength(2);
  expect(
    screen.getByRole('button', { name: 'Undo all changes of Show file extensions' }),
  ).toBeTruthy();

  // The older batch has nothing left to undo, so its group offers no Undo all.
  expect(screen.getByRole('list', { name: 'Other: records' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Undo all changes of Other' })).toBeNull();
});

it('does not group records that only share a batch with something not next to them', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      undoable('3', { batch: '8', summary: 'One' }),
      undoable('2', { batch: '9', summary: 'Two' }),
      undoable('1', { batch: '8', summary: 'Three' }),
    ]),
  );
  render(<UndoScreen />);
  await screen.findByRole('list', { name: 'Changes' });
  expect(screen.queryByText(/in one batch/)).toBeNull();
  expect(screen.queryByRole('button', { name: /Undo all/ })).toBeNull();
  expect(rows()).toHaveLength(3);
});

it('has a plain empty state and no export buttons', async () => {
  ipc.listActivity.mockResolvedValue(page([]));
  render(<UndoScreen />);
  expect(await screen.findByText('Nothing yet')).toBeTruthy();
  expect(screen.queryByRole('list')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Export' })).toBeNull();
});

it('explains a failed read and reads again on Try again', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockRejectedValueOnce(new Error('database is locked'));
  render(<UndoScreen />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('The history could not be loaded');
  expect(alert.textContent).toContain('database is locked');

  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('list', { name: 'Changes' })).toBeTruthy();
});

it('says the history needs the Volt app when run in a plain browser', async () => {
  ipc.listActivity.mockRejectedValue(new IpcUnavailableError());
  render(<UndoScreen />);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('only available inside the Volt app');
});

it('loads older entries with the cursor and appends them', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValueOnce(page([item('3', { summary: 'Third' })], '3'));
  render(<UndoScreen />);
  const more = await screen.findByRole('button', { name: 'Show older' });

  ipc.listActivity.mockResolvedValueOnce(
    page([item('2', { status: 'failed', state: 'failed', summary: 'Second' })]),
  );
  await user.click(more);
  expect(ipc.listActivity).toHaveBeenLastCalledWith(50, '3');
  await vi.waitFor(() => expect(rows()).toHaveLength(2));
  expect(screen.queryByRole('button', { name: 'Show older' })).toBeNull();
});

// ---- undoing one change ------------------------------------------------------------------------

it('undoes one change from its row, says so and shows the fresh list', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValueOnce(page([undoable('1', { summary: 'Applied it' })]));
  render(<UndoScreen />);
  const undo = await screen.findByRole('button', { name: 'Undo this change: Applied it' });

  const pending = deferred<UndoResult>();
  ipc.undoEntry.mockReturnValue(pending.promise);
  // After the undo the journal says: undone, and an undo record on top.
  ipc.listActivity.mockResolvedValueOnce(
    page([
      item('2', { status: 'reverted', state: 'restored', summary: 'Reverted it' }),
      item('1', { entry: '1', state: 'undone', summary: 'Applied it' }),
    ]),
  );
  await user.click(undo);
  expect(ipc.undoEntry).toHaveBeenCalledWith('1', []);
  expect(undo.getAttribute('aria-busy')).toBe('true');

  pending.resolve(result());
  const notice = await screen.findByText('Undone. The earlier value is back.');
  expect(notice.closest('[role=status]')).toBeTruthy();
  // The list was read again, and the row now says so.
  expect(ipc.listActivity).toHaveBeenCalledTimes(2);
  await vi.waitFor(() => expect(rows()).toHaveLength(2));
  expect(rows()[1]?.textContent).toContain('Already undone');
  expect(screen.queryByRole('button', { name: /Undo this change/ })).toBeNull();
});

it('blocks other undo buttons while one runs', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(
    page([undoable('2', { summary: 'Two' }), undoable('1', { summary: 'One' })]),
  );
  render(<UndoScreen />);
  const [first, second] = [
    await screen.findByRole('button', { name: 'Undo this change: Two' }),
    await screen.findByRole('button', { name: 'Undo this change: One' }),
  ];
  ipc.undoEntry.mockReturnValue(new Promise(() => {}));
  await user.click(first);
  expect((second as HTMLButtonElement).disabled).toBe(true);
});

it('answers "nothing to undo" without calling it an error', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1', { summary: 'Applied it' })]));
  render(<UndoScreen />);
  ipc.undoEntry.mockResolvedValue(result({ status: 'nothingToUndo', restored: 0 }));
  await user.click(await screen.findByRole('button', { name: 'Undo this change: Applied it' }));
  const notice = await screen.findByText(/Nothing to undo here/);
  expect(notice.closest('[role=status]')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
});

it('says what could not be restored when an undo fails', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1', { summary: 'Applied it' })]));
  render(<UndoScreen />);
  ipc.undoEntry.mockResolvedValue(
    result({
      status: 'failed',
      restored: 0,
      failed: 1,
      failures: [
        {
          entry: '1',
          what: 'Registry value HKCU\\X\\Flag',
          whatPhrase: null,
          reason: 'the previous state could not be restored (access denied)',
          reasonPhrase: null,
        },
      ],
    }),
  );
  await user.click(await screen.findByRole('button', { name: 'Undo this change: Applied it' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Could not undo this change');
  expect(alert.textContent).toContain('Registry value HKCU\\X\\Flag');
  expect(alert.textContent).toContain('access denied');
});

it('explains a command that failed outright and still reads the list again', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1', { summary: 'Applied it' })]));
  render(<UndoScreen />);
  ipc.undoEntry.mockRejectedValue(
    new IpcCommandError({ kind: 'notFound', message: 'there is no journal entry #1' }),
  );
  await user.click(await screen.findByRole('button', { name: 'Undo this change: Applied it' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Could not undo.');
  expect(alert.textContent).toContain('there is no journal entry #1');
  expect(ipc.listActivity).toHaveBeenCalledTimes(2);
});

// ---- undoing a batch ---------------------------------------------------------------------------

const BATCH = [
  undoable('4', { batch: '8', summary: PRE }),
  undoable('3', { batch: '8', summary: FLAG }),
];

it('asks before undoing a batch, listing what will be restored, and does nothing on Cancel', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page(BATCH));
  ipc.previewUndoBatch.mockResolvedValue({
    items: [
      { entry: '4', what: 'Registry value HKCU\\X\\Pre', restore: 'set back to REG_DWORD 5' },
      {
        entry: '3',
        what: 'Registry value HKCU\\X\\Flag',
        restore: 'removed (it did not exist before)',
      },
    ],
    overwrites: [],
  });
  render(<UndoScreen />);
  await user.click(
    await screen.findByRole('button', { name: 'Undo all changes of Show file extensions' }),
  );

  const dialog = await screen.findByRole('dialog', { name: 'Undo all of these changes?' });
  expect(ipc.previewUndoBatch).toHaveBeenCalledWith('8');
  const list = await within(dialog).findByRole('list', { name: 'What will be restored' });
  const lines = within(list).getAllByRole('listitem');
  expect(lines).toHaveLength(2);
  expect(lines[0]?.textContent).toContain('Registry value HKCU\\X\\Pre');
  expect(lines[0]?.textContent).toContain('Will be set back to REG_DWORD 5.');
  expect(lines[1]?.textContent).toContain('Will be removed (it did not exist before).');
  expect(dialog.textContent).toContain('"Show file extensions"');
  expect(within(dialog).getByRole('button', { name: 'Undo 2 changes' })).toBeTruthy();

  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(ipc.undoBatch).not.toHaveBeenCalled();
});

it('undoes the batch once confirmed and reports a complete undo', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValueOnce(page(BATCH));
  ipc.previewUndoBatch.mockResolvedValue({
    items: [
      { entry: '4', what: 'Registry value HKCU\\X\\Pre', restore: 'set back to REG_DWORD 5' },
      { entry: '3', what: 'Registry value HKCU\\X\\Flag', restore: 'set back to REG_DWORD 1' },
    ],
    overwrites: [],
  });
  render(<UndoScreen />);
  await user.click(
    await screen.findByRole('button', { name: 'Undo all changes of Show file extensions' }),
  );
  ipc.undoBatch.mockResolvedValue(result({ restored: 2 }));
  ipc.listActivity.mockResolvedValueOnce(
    page(BATCH.map((row) => ({ ...row, state: 'undone' as const }))),
  );
  await user.click(await screen.findByRole('button', { name: 'Undo 2 changes' }));

  expect(ipc.undoBatch).toHaveBeenCalledWith('8', []);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(await screen.findByText('Undone. 2 changes are back to how they were.')).toBeTruthy();
  await vi.waitFor(() => expect(ipc.listActivity).toHaveBeenCalledTimes(2));
  await vi.waitFor(() =>
    expect(screen.queryByRole('button', { name: /Undo all changes/ })).toBeNull(),
  );
});

it('reports a partly undone batch with what failed and why', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page(BATCH));
  ipc.previewUndoBatch.mockResolvedValue({
    items: [
      { entry: '4', what: 'Registry value HKCU\\X\\Pre', restore: 'set back to REG_DWORD 5' },
    ],
    overwrites: [],
  });
  render(<UndoScreen />);
  await user.click(
    await screen.findByRole('button', { name: 'Undo all changes of Show file extensions' }),
  );
  ipc.undoBatch.mockResolvedValue(
    result({
      status: 'partial',
      restored: 1,
      failed: 1,
      failures: [
        {
          entry: '4',
          what: 'Registry value HKCU\\X\\Pre',
          whatPhrase: null,
          reason: 'the previous state could not be restored (access denied)',
          reasonPhrase: null,
        },
      ],
    }),
  );
  await user.click(await screen.findByRole('button', { name: 'Undo 1 change' }));

  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Only part of it was undone: 1 restored, 1 not.');
  expect(alert.textContent).toContain(
    'Registry value HKCU\\X\\Pre: the previous state could not be restored (access denied)',
  );
});

it('answers "nothing to undo" for a batch that was undone elsewhere, and cannot be confirmed empty', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page(BATCH));
  ipc.previewUndoBatch.mockResolvedValue({ items: [], overwrites: [] });
  render(<UndoScreen />);
  await user.click(
    await screen.findByRole('button', { name: 'Undo all changes of Show file extensions' }),
  );
  const dialog = await screen.findByRole('dialog');
  expect(await within(dialog).findByText(/Nothing is left to undo in this batch/)).toBeTruthy();
  expect((within(dialog).getByRole('button', { name: 'Undo' }) as HTMLButtonElement).disabled).toBe(
    true,
  );
});

it('shows why the batch could not be read and keeps the confirm button off', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page(BATCH));
  ipc.previewUndoBatch.mockRejectedValue(new Error('database is locked'));
  render(<UndoScreen />);
  await user.click(
    await screen.findByRole('button', { name: 'Undo all changes of Show file extensions' }),
  );
  const dialog = await screen.findByRole('dialog');
  const alert = await within(dialog).findByRole('alert');
  expect(alert.textContent).toContain('database is locked');
  expect((within(dialog).getByRole('button', { name: 'Undo' }) as HTMLButtonElement).disabled).toBe(
    true,
  );
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(ipc.undoBatch).not.toHaveBeenCalled();
});

// ---- export ------------------------------------------------------------------------------------

it('saves the log through the shell and says where it went', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  render(<UndoScreen />);
  ipc.exportActivity.mockResolvedValue({
    kind: 'saved',
    path: 'C:\\Users\\me\\log.txt',
    records: 3,
  });
  await user.click(await screen.findByRole('button', { name: 'Export' }));
  expect(ipc.exportActivity).toHaveBeenCalledTimes(1);
  expect(await screen.findByText('Saved 3 records to C:\\Users\\me\\log.txt.')).toBeTruthy();
});

it('says nothing when the save dialog is closed', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  render(<UndoScreen />);
  ipc.exportActivity.mockResolvedValue({ kind: 'cancelled' });
  await user.click(await screen.findByRole('button', { name: 'Export' }));
  await vi.waitFor(() => expect(ipc.exportActivity).toHaveBeenCalled());
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByRole('status')).toBeNull();
});

it('explains a failed save and offers the copy fallback', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  render(<UndoScreen />);
  ipc.exportActivity.mockRejectedValue(
    new IpcCommandError({ kind: 'accessDenied', message: 'could not write C:\\x.txt' }),
  );
  await user.click(await screen.findByRole('button', { name: 'Export' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Could not save the file.');
  expect(alert.textContent).toContain('could not write C:\\x.txt');
  expect(alert.textContent).toContain('You can copy the text instead.');
});

it('copies the log text to the clipboard', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  render(<UndoScreen />);
  ipc.exportActivityText.mockResolvedValue({
    fileName: 'volt-activity-2026-11-10.txt',
    text: '2026-11-10T03:01:00.123Z manual applied one\n',
    records: 1,
  });
  await user.click(await screen.findByRole('button', { name: 'Copy' }));
  expect(await screen.findByText('Copied 1 record to the clipboard.')).toBeTruthy();
  expect(await navigator.clipboard.readText()).toBe(
    '2026-11-10T03:01:00.123Z manual applied one\n',
  );
});

it('explains a copy that failed', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  render(<UndoScreen />);
  ipc.exportActivityText.mockRejectedValue(new Error('the log is unreadable'));
  await user.click(await screen.findByRole('button', { name: 'Copy' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Could not copy the text. the log is unreadable.');
});

// ---- later changes, incomplete history, refresh ------------------------------------------------

it('asks before an undo that would overwrite a later change, and names it', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1', { summary: 'Applied it' })]));
  ipc.previewUndoEntry.mockResolvedValue({
    items: [{ entry: '1', what: 'Registry value HKCUXFlag', restore: 'set back to REG_DWORD 1' }],
    overwrites: [
      {
        entry: '1',
        what: 'Registry value HKCUXFlag',
        laterEntry: '9',
        laterTime: '2026-11-10T04:00:00.000Z',
        laterSource: 'automation',
      },
    ],
  });
  ipc.undoEntry.mockResolvedValue(result());
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: /^Undo/ }));
  const dialog = await screen.findByRole('dialog');
  expect(within(dialog).getByText('A later change will be overwritten')).toBeDefined();
  expect(within(dialog).getByText(/Flag was changed again on/)).toBeDefined();
  expect(ipc.undoEntry).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole('button', { name: 'Undo anyway' }));
  await vi.waitFor(() => expect(ipc.undoEntry).toHaveBeenCalledWith('1', ['9']));
});

const refusal = () =>
  result({
    status: 'needsAcknowledgement',
    restored: 0,
    overwrites: [
      {
        entry: '1',
        what: 'Flag',
        whatPhrase: { code: 'subject.registryValue', args: {} },
        laterEntry: '9',
        laterTime: '2026-11-10T04:00:00.000Z',
        laterSource: 'manual',
      },
    ],
  });

it('opens the warning again with the new overwrites when the core refuses an undo', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  ipc.previewUndoEntry.mockResolvedValueOnce({ items: [], overwrites: [] }).mockResolvedValueOnce({
    items: [{ entry: '1', what: 'Flag', restore: 'set back to REG_DWORD 1' }],
    overwrites: refusal().overwrites,
  });
  ipc.undoEntry.mockResolvedValueOnce(refusal()).mockResolvedValue(result());
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: /^Undo/ }));
  const dialog = await screen.findByRole('dialog');
  expect(within(dialog).getByText('A later change will be overwritten')).toBeDefined();
  await user.click(within(dialog).getByRole('button', { name: 'Undo anyway' }));
  await vi.waitFor(() => expect(ipc.undoEntry).toHaveBeenLastCalledWith('1', ['9']));
});

it('falls back to the notice when the warning cannot be shown again', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  ipc.previewUndoEntry
    .mockResolvedValueOnce({ items: [], overwrites: [] })
    .mockRejectedValueOnce(new Error('gone'));
  ipc.undoEntry.mockResolvedValue(refusal());
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: /^Undo/ }));
  expect(await screen.findByText(/Nothing was undone: a later change/)).toBeDefined();
});

it('does not undo when the user cancels the overwrite warning', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([undoable('1', { summary: 'Applied it' })]));
  ipc.previewUndoEntry.mockResolvedValue({
    items: [{ entry: '1', what: 'Registry value X', restore: 'removed' }],
    overwrites: [
      {
        entry: '1',
        what: 'Registry value X',
        laterEntry: '9',
        laterTime: '2026-11-10T04:00:00.000Z',
        laterSource: 'manual',
      },
    ],
  });
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: /^Undo/ }));
  await user.click(await screen.findByRole('button', { name: 'Cancel' }));
  expect(ipc.undoEntry).not.toHaveBeenCalled();
});

it('shows a notice while the history is known to be incomplete', async () => {
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  ipc.historyStatus.mockResolvedValue({ incomplete: true });
  render(<UndoScreen />);
  expect(await screen.findByText(/This history is incomplete/)).toBeDefined();
});

it('keeps every loaded row, also beyond one page of 200, when the list is read again after an undo', async () => {
  // 210 rows are just enough to need a second page of 200 on the re-read; fireEvent, not
  // user-event, because every simulated pointer step is slow with that many rows on a busy machine.
  const many = (from: number, to: number) =>
    Array.from({ length: to - from + 1 }, (_, i) =>
      item(String(to - i), { summary: `Row ${to - i}` }),
    );
  const first = many(111, 220);
  first[0] = undoable('220', { summary: 'Row 220' });
  ipc.listActivity.mockResolvedValueOnce(page(first, '111'));
  ipc.listActivity.mockResolvedValueOnce(page(many(11, 110), '11'));
  render(<UndoScreen />);
  await screen.findByText('Row 220');
  fireEvent.click(await screen.findByText('Show older'));
  await vi.waitFor(() => expect(rows()).toHaveLength(210));
  ipc.undoEntry.mockResolvedValue(result());
  // After the undo there is one more record at the top; the shell caps a page at 200.
  const after = [item('221', { summary: 'Row 221', status: 'reverted' }), ...many(11, 220)];
  ipc.listActivity.mockResolvedValueOnce(page(after.slice(0, 200), '22'));
  ipc.listActivity.mockResolvedValueOnce(page(after.slice(200), null));
  const undoButton = Array.from(document.querySelectorAll('button')).find((b) =>
    /^Undo/.test(b.textContent ?? ''),
  );
  fireEvent.click(undoButton as HTMLElement);
  await screen.findByText('Row 221');
  expect(ipc.listActivity).toHaveBeenLastCalledWith(200, '22');
  await vi.waitFor(() => expect(rows()).toHaveLength(211));
});

it('clears the history after a confirmation that says undoable changes stay', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValueOnce(page([undoable('1'), item('2')]));
  ipc.clearHistory.mockResolvedValue({ removed: 1, kept: 1 });
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: 'Clear' }));
  expect(await screen.findByText(/Changes that can still be undone stay/)).toBeDefined();
  expect(ipc.clearHistory).not.toHaveBeenCalled();
  ipc.listActivity.mockResolvedValue(page([undoable('1')]));
  await user.click(screen.getByRole('button', { name: 'Clear history' }));
  expect(await screen.findByText(/Cleared 1 record\./)).toBeDefined();
  expect(
    screen.getByText(/1 record stays in the list because its change can still be undone/),
  ).toBeDefined();
  expect(ipc.clearHistory).toHaveBeenCalledTimes(1);
});

it('does not clear when the confirmation is cancelled', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([item('2')]));
  render(<UndoScreen />);
  await user.click(await screen.findByRole('button', { name: 'Clear' }));
  await user.click(await screen.findByRole('button', { name: 'Cancel' }));
  expect(ipc.clearHistory).not.toHaveBeenCalled();
});

it('names a memory or process record by its title, and leaves an unknown one by its status', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      item('3', { summary: 'Ended 2 apps', label: 'process.end', title: 'Ended apps' }),
      item('2', {
        summary: 'Trimmed working sets',
        label: 'memory.clean',
        title: 'Memory cleaning',
      }),
      item('1', { summary: 'Something', label: 'unknown', title: null }),
    ]),
  );
  render(<UndoScreen />);
  await screen.findByRole('list', { name: 'Changes' });
  const [ended, memory, other] = rows();
  expect(ended?.textContent).toContain('Ended apps: Applied');
  expect(memory?.textContent).toContain('Memory cleaning: Applied');
  expect(other?.textContent).not.toContain(': Applied');
});

/** A row the elevated helper wrote, mirrored by the shell. */
function administrator(id: string, over: Partial<ActivityItem> = {}): ActivityItem {
  return item(id, {
    administrator: true,
    administratorTweak: 'turn-off-error-reporting',
    label: 'turn-off-error-reporting',
    title: 'Turn off Windows Error Reporting',
    summary: 'Applied registry value HKLMXFlag = REG_DWORD 1',
    state: 'canUndo',
    ...over,
  });
}

it('marks the rows the helper made and undoes one through the helper, not the journal', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([administrator('admin-1')]));
  render(<UndoScreen />);
  const row = (await screen.findByText(/Applied registry value HKLM/)).closest('li') as HTMLElement;
  expect(within(row).getByText('Administrator')).toBeTruthy();
  ipc.undoAdministratorChange.mockResolvedValue(result());
  await user.click(
    within(row).getByRole('button', { name: /Undo this change: Applied registry value HKLM/ }),
  );
  expect(ipc.undoAdministratorChange).toHaveBeenCalledWith('turn-off-error-reporting');
  expect(ipc.undoEntry).not.toHaveBeenCalled();
  expect(await screen.findByText(/Undone/)).toBeTruthy();
});

it('shows a helper row read-only, with the reason, when the helper is not available', async () => {
  ipc.listActivity.mockResolvedValue(
    page([
      administrator('admin-1', {
        state: 'cannotUndo',
        reason: 'Administrator changes can be undone only from a Volt installed for all users.',
      }),
    ]),
  );
  render(<UndoScreen />);
  const row = (await screen.findByText(/Applied registry value HKLM/)).closest('li') as HTMLElement;
  expect(within(row).getByText('Administrator')).toBeTruthy();
  expect(within(row).queryByRole('button')).toBeNull();
  expect(row.textContent).toContain('installed for all users');
});

it('says what happened when the confirmation of an administrator undo was not given', async () => {
  const user = userEvent.setup();
  ipc.listActivity.mockResolvedValue(page([administrator('admin-1')]));
  render(<UndoScreen />);
  for (const [status, text] of [
    ['notConfirmed', /declined or timed out/],
    ['blocked', /Try again in 10 minutes/],
    ['timedOut', /stopped waiting for the administrator helper/],
    ['declined', /cancelled the administrator prompt/],
  ] as const) {
    ipc.undoAdministratorChange.mockResolvedValueOnce(result({ status, restored: 0 }));
    await user.click(await screen.findByRole('button', { name: /Undo this change/ }));
    expect((await screen.findByText(text)).textContent).toMatch(text);
  }
});
