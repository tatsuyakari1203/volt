import { expect, it } from 'vitest';
import type { ActivityItem } from '../../shared/ipc';
import { groupHistory } from './history';

function item(id: string, over: Partial<ActivityItem> = {}): ActivityItem {
  return {
    id,
    time: '2026-11-10T03:01:00.123Z',
    source: 'manual',
    status: 'applied',
    summary: `Record ${id}`,
    label: null,
    title: null,
    batch: null,
    entry: null,
    previous: null,
    state: 'cannotUndo',
    reason: null,
    ...over,
  };
}

it('keeps records without a batch as plain rows, in order', () => {
  const entries = groupHistory([item('3'), item('2'), item('1')]);
  expect(entries.map((entry) => entry.kind)).toEqual(['row', 'row', 'row']);
});

it('groups neighbouring records of one batch and names the group by the tweak title', () => {
  const entries = groupHistory([
    item('4', { batch: '2', label: 'tweak.x' }),
    item('3', { batch: '2', label: 'tweak.x', title: 'Show file extensions' }),
    item('2'),
    item('1', { batch: '1' }),
  ]);
  expect(entries).toHaveLength(3);
  const [group, plain, alone] = entries;
  expect(group?.kind).toBe('group');
  if (group?.kind === 'group') {
    expect(group.batch).toBe('2');
    expect(group.items.map((row) => row.id)).toEqual(['4', '3']);
    expect(group.title).toBe('Show file extensions');
  }
  expect(plain?.kind).toBe('row');
  expect(alone?.kind).toBe('row');
});

it('falls back to the label, then to a generic name, for a group title', () => {
  const [byLabel] = groupHistory([
    item('2', { batch: '1', label: 'tweak.x' }),
    item('1', { batch: '1', label: 'tweak.x' }),
  ]);
  expect(byLabel?.kind === 'group' && byLabel.title).toBe('tweak.x');
  const [unnamed] = groupHistory([item('2', { batch: '1' }), item('1', { batch: '1' })]);
  expect(unnamed?.kind === 'group' && unnamed.title).toBe('Changes made together');
});

it('never groups records that are not next to each other', () => {
  const entries = groupHistory([
    item('3', { batch: '1' }),
    item('2', { batch: '2' }),
    item('1', { batch: '1' }),
  ]);
  expect(entries.map((entry) => entry.kind)).toEqual(['row', 'row', 'row']);
});
