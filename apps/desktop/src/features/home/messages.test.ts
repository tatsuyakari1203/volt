import { afterEach, describe, expect, it } from 'vitest';
import { setLocale } from '../../shared/i18n';
import type { ArgView, DeductionView, PhraseView } from '../../shared/ipc';
import { deductionReason, deductionTitle } from './messages';

afterEach(() => setLocale('en'));

function phrase(code: string, args: Record<string, ArgView> = {}): PhraseView {
  return { code, args };
}

function deduction(
  id: string,
  title: string,
  reason: string,
  reasonPhrase: PhraseView,
): DeductionView {
  return { id, points: 5, title, reason, reasonPhrase, fix: null };
}

const DISK = deduction(
  'disk.low',
  'Disk space is low',
  '12.4 GB of 476.2 GB is free on C: (2.6%). Windows needs room to install updates and to move memory to disk.',
  phrase('health.disk', {
    free: { kind: 'bytes', value: 12.4 * 1024 ** 3 },
    total: { kind: 'bytes', value: 476.2 * 1024 ** 3 },
    mount: { kind: 'text', value: 'C:' },
    share: { kind: 'tenths', value: 26 },
  }),
);

describe('deductions in English', () => {
  it('are shown exactly as the core wrote them', () => {
    expect(deductionTitle(DISK)).toBe('Disk space is low');
    expect(deductionReason(DISK)).toBe(DISK.reason);
  });
});

describe('deductions in Vietnamese', () => {
  it('are translated by id and built from the parameters, with a decimal comma', () => {
    setLocale('vi');
    expect(deductionTitle(DISK)).toBe('Dung lượng ổ đĩa thấp');
    const reason = deductionReason(DISK);
    expect(reason).toContain('12,4 GB');
    expect(reason).toContain('476 GB');
    expect(reason).toContain('ổ C:');
    expect(reason).toContain('(2,6%)');
    expect(reason).not.toContain('Windows needs room');
  });

  it('do not depend on the English sentence at all', () => {
    setLocale('vi');
    const memory = deduction(
      'memory.high',
      'x',
      'not the usual sentence',
      phrase('health.memory', { percent: { kind: 'count', value: 91 } }),
    );
    expect(deductionReason(memory)).toBe(
      'Đang dùng 91% bộ nhớ. Khi hết bộ nhớ, Windows sẽ chậm lại và có thể đóng chương trình.',
    );
    const restart = deduction('restart.pending', 'x', '?', phrase('health.restart'));
    expect(deductionReason(restart)).toContain('khởi động lại');
    const startup = deduction(
      'startup.many',
      'x',
      '?',
      phrase('health.startup', { count: { kind: 'count', value: 12 } }),
    );
    expect(deductionReason(startup)).toContain('Có 12 mục khởi động');
  });

  it("show the core's own text for an id or a code it does not know", () => {
    setLocale('vi');
    const unknown = deduction('disk.new', 'A new problem', 'Something new.', phrase('health.new'));
    expect(deductionTitle(unknown)).toBe('A new problem');
    expect(deductionReason(unknown)).toBe('Something new.');
  });
});
