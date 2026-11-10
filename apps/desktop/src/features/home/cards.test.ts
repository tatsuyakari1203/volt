import { describe, expect, it } from 'vitest';
import type { MetricsView } from '../../shared/ipc';
import { formatBytes } from '../../shared/text/size';
import { buildCards, NO_VALUE } from './cards';
import { driveName, formatPercent } from './format';
import { fixHint, fixLink, headline, levelLabel } from './messages';

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

describe('formatBytes', () => {
  it('writes sizes the way the Windows shell does', () => {
    expect(formatBytes(0)).toBe('0 MB');
    expect(formatBytes(512 * MIB)).toBe('512 MB');
    expect(formatBytes(GIB)).toBe('1 GB');
    expect(formatBytes(9.9 * GIB)).toBe('9.9 GB');
    expect(formatBytes(16 * GIB)).toBe('16 GB');
    expect(formatBytes(182.4 * GIB)).toBe('182 GB');
    expect(formatBytes(1.5 * 1024 * GIB)).toBe('1.5 TB');
  });

  it('shows a dash instead of a made-up size for a value that is not a size', () => {
    expect(formatBytes(Number.NaN)).toBe('—');
    expect(formatBytes(-1)).toBe('—');
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('—');
  });

  it('rounds percentages and names drives', () => {
    expect(formatPercent(61.9)).toBe('62%');
    expect(formatPercent(0)).toBe('0%');
    expect(driveName('C:\\')).toBe('C:');
    expect(driveName('C:')).toBe('C:');
  });
});

function reading(over: Partial<MetricsView> = {}): MetricsView {
  return {
    takenAt: '2026-11-10T08:30:01.000Z',
    cpuPercent: 12.5,
    memory: { totalBytes: 16 * GIB, usedBytes: 8 * GIB, availableBytes: 8 * GIB, usedPercent: 50 },
    disks: [
      {
        mount: 'C:\\',
        totalBytes: 500 * GIB,
        usedBytes: 300 * GIB,
        freeBytes: 200 * GIB,
        usedPercent: 60,
      },
    ],
    gpu: null,
    ...over,
  };
}

describe('buildCards', () => {
  it('always returns the four cards in order', () => {
    expect(buildCards(reading()).map((card) => card.id)).toEqual(['cpu', 'memory', 'gpu', 'disk']);
  });

  it('marks a null CPU and a missing GPU as empty with a dash, never a number', () => {
    const [cpu, memory, gpu, disk] = buildCards(reading({ cpuPercent: null }));
    expect(cpu).toMatchObject({ value: NO_VALUE, empty: true, lines: ['No reading yet'] });
    expect(gpu).toMatchObject({ value: NO_VALUE, empty: true, lines: ['No GPU detected'] });
    expect(memory?.empty).toBe(false);
    expect(disk?.empty).toBe(false);
  });

  it('keeps a real zero: 0 percent CPU is a reading, not an empty card', () => {
    const [cpu] = buildCards(reading({ cpuPercent: 0 }));
    expect(cpu).toMatchObject({ value: '0%', empty: false });
  });

  it('shows no disk reading when there are no disks', () => {
    const [, , , disk] = buildCards(reading({ disks: [] }));
    expect(disk).toMatchObject({ value: NO_VALUE, empty: true, lines: ['No disk reading'] });
  });

  it('picks the system disk case-insensitively and falls back to the first disk, naming it', () => {
    const disks = [
      { mount: 'D:\\', totalBytes: 100 * GIB, usedBytes: 0, freeBytes: 100 * GIB, usedPercent: 0 },
      { mount: 'C:\\', totalBytes: 200 * GIB, usedBytes: 0, freeBytes: 150 * GIB, usedPercent: 0 },
    ];
    expect(buildCards(reading({ disks }), 'c:')[3]?.lines).toEqual(['Free of 200 GB on C:']);
    expect(buildCards(reading({ disks }), 'Z:\\')[3]?.lines).toEqual(['Free of 100 GB on D:']);
    expect(buildCards(reading({ disks }))[3]?.lines).toEqual(['Free of 100 GB on D:']);
  });

  it('drops the video memory line when only one of its two numbers is known', () => {
    const [, , gpu] = buildCards(
      reading({
        gpu: {
          name: 'Card',
          utilisationPercent: 10,
          vramUsedBytes: GIB,
          vramTotalBytes: null,
          vramUsedPercent: null,
        },
      }),
    );
    expect(gpu).toMatchObject({ value: '10%', lines: ['Card'], empty: false });
  });
});

describe('messages', () => {
  it('maps each fix to a button, except Restart Windows which is a sentence', () => {
    expect(fixLink('openCleaner')).toEqual({ target: 'clean', label: 'Open Cleaner' });
    expect(fixLink('openMemory')).toEqual({ target: 'memory', label: 'Open Memory' });
    expect(fixLink('openStartup')).toEqual({ target: 'startup', label: 'Open Startup' });
    expect(fixLink('restartWindows')).toBeNull();
    expect(fixLink(null)).toBeNull();
    expect(fixHint('restartWindows')).toMatch(/Restart Windows/);
    expect(fixHint('openCleaner')).toBeNull();
  });

  it('labels every level and words the headline by grade and count', () => {
    expect(levelLabel('good')).toBe('Good');
    expect(levelLabel('unknown')).toBe('Not checked');
    const base = { score: 90, deductions: [], unchecked: [], systemMount: 'C:\\', checkedAt: '' };
    expect(headline({ ...base, level: 'good' }).detail).toBe('Nothing is lowering the score.');
    const one = {
      id: 'restart.pending',
      points: 10,
      title: 't',
      reason: 'r',
      fix: 'restartWindows' as const,
    };
    expect(headline({ ...base, level: 'fair', deductions: [one] })).toEqual({
      title: 'Your PC could be better',
      detail: '1 thing lowers the score. Each one is listed below.',
    });
  });
});
