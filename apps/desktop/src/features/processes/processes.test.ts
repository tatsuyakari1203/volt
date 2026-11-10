import { describe, expect, it } from 'vitest';
import type { EndAppResult, ProcessGroupView } from '../../shared/ipc';
import { cpuText, endSummary, visibleGroups } from './processes';

function group(id: string, memoryBytes: number, cpuPercent: number | null): ProcessGroupView {
  return {
    id,
    name: id,
    path: null,
    processCount: 1,
    endCount: 1,
    memoryBytes,
    cpuPercent,
    endability: 'allowed',
  };
}

const groups = [
  group('code.exe', 900, 1),
  group('chrome.exe', 500, 30),
  group('notepad.exe', 5, null),
  group('chrome-helper.exe', 500, 2),
];

describe('visibleGroups', () => {
  it('sorts by memory by default, ties by name, and does not change its input', () => {
    const copy = [...groups];
    expect(visibleGroups(groups, '', 'memory').map((g) => g.id)).toEqual([
      'code.exe',
      'chrome-helper.exe',
      'chrome.exe',
      'notepad.exe',
    ]);
    expect(groups).toEqual(copy);
  });

  it('sorts by CPU with memory as the tie-break, a missing reading counting as 0', () => {
    expect(visibleGroups(groups, '', 'cpu').map((g) => g.id)).toEqual([
      'chrome.exe',
      'chrome-helper.exe',
      'code.exe',
      'notepad.exe',
    ]);
  });

  it('filters by a fragment of the name, ignoring case and spaces around it', () => {
    expect(visibleGroups(groups, '  CHROME ', 'memory').map((g) => g.id)).toEqual([
      'chrome-helper.exe',
      'chrome.exe',
    ]);
    expect(visibleGroups(groups, 'nothing', 'memory')).toEqual([]);
  });
});

describe('cpuText', () => {
  it('shows a dash without a reading and keeps small numbers readable', () => {
    expect(cpuText(null)).toBe('–');
    expect(cpuText(0)).toBe('0%');
    expect(cpuText(0.04)).toBe('0%');
    expect(cpuText(1.26)).toBe('1.3%');
    expect(cpuText(12.4)).toBe('12%');
    expect(cpuText(100)).toBe('100%');
  });
});

describe('endSummary', () => {
  const base: EndAppResult = {
    name: 'chrome.exe',
    ended: 3,
    needsAdministrator: 0,
    alreadyGone: 0,
    failed: 0,
    processes: [],
    recorded: true,
  };

  it('says what ended and is no problem when everything did', () => {
    expect(endSummary(base)).toEqual({ text: 'Ended chrome.exe: 3 processes.', problem: false });
  });

  it('names the processes that need administrator rights as a problem', () => {
    const summary = endSummary({ ...base, ended: 1, needsAdministrator: 2 });
    expect(summary.problem).toBe(true);
    expect(summary.text).toContain('Ended chrome.exe: 1 process.');
    expect(summary.text).toContain('2 processes need administrator rights');
  });

  it('says so when nothing ended', () => {
    const summary = endSummary({ ...base, ended: 0, needsAdministrator: 3 });
    expect(summary.text.startsWith('Nothing was ended for chrome.exe.')).toBe(true);
    expect(endSummary({ ...base, ended: 0, alreadyGone: 3 }).text).toContain('already closed');
  });

  it('flags a missing activity record', () => {
    expect(endSummary({ ...base, recorded: false }).problem).toBe(true);
  });
});
