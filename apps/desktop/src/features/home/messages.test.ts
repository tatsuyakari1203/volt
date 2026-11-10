import { afterEach, describe, expect, it } from 'vitest';
import { setLocale } from '../../shared/i18n';
import type { DeductionView } from '../../shared/ipc';
import { deductionReason, deductionTitle } from './messages';

afterEach(() => setLocale('en'));

function deduction(id: string, title: string, reason: string): DeductionView {
  return { id, points: 5, title, reason, fix: null };
}

const DISK = deduction(
  'disk.low',
  'Disk space is low',
  '12.4 GB of 476.2 GB is free on C: (2.6%). Windows needs room to install updates and to move memory to disk.',
);

describe('deductions in English', () => {
  it('are shown exactly as the core wrote them', () => {
    expect(deductionTitle(DISK)).toBe('Disk space is low');
    expect(deductionReason(DISK)).toBe(DISK.reason);
  });
});

describe('deductions in Vietnamese', () => {
  it('are translated by id and keep the figures, with a decimal comma', () => {
    setLocale('vi');
    expect(deductionTitle(DISK)).toBe('Dung lượng ổ đĩa thấp');
    const reason = deductionReason(DISK);
    expect(reason).toContain('12,4 GB');
    expect(reason).toContain('476,2 GB');
    expect(reason).toContain('C:');
    expect(reason).toContain('2,6%');
    expect(reason).not.toContain('Windows needs room');
  });

  it('translate the memory, restart and startup reasons', () => {
    setLocale('vi');
    const memory = deduction('memory.high', 'x', '91% of your memory is in use. When it runs out.');
    expect(deductionReason(memory)).toContain('91%');
    const restart = deduction('restart.pending', 'x', 'Windows has updates or installs.');
    expect(deductionReason(restart)).toContain('khởi động lại');
    const startup = deduction('startup.many', 'x', '12 startup entries are enabled. Each one.');
    expect(deductionReason(startup)).toContain('12');
  });

  it("show the core's own text for an id or a sentence it does not know", () => {
    setLocale('vi');
    const unknown = deduction('disk.new', 'A new problem', 'Something new.');
    expect(deductionTitle(unknown)).toBe('A new problem');
    expect(deductionReason(unknown)).toBe('Something new.');
    const odd = deduction('disk.low', 'x', 'A sentence in another shape.');
    expect(deductionReason(odd)).toBe('A sentence in another shape.');
  });
});
