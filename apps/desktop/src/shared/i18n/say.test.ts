import { afterEach, describe, expect, it } from 'vitest';
import type { ArgView, PhraseView } from '../ipc/bindings';
import { say, sayCapitalised } from './say';
import { setLocale } from './translate';

afterEach(() => setLocale('en'));

function phrase(code: string, args: Record<string, ArgView> = {}): PhraseView {
  return { code, args };
}

const text = (value: string): ArgView => ({ kind: 'text', value });
const count = (value: number): ArgView => ({ kind: 'count', value });

const APPLIED = phrase('summary.applied', {
  subject: { kind: 'phrase', value: phrase('subject.registryValue', { target: text('HKCU\\X') }) },
  value: { kind: 'phrase', value: phrase('value.sz', { count: count(5) }) },
});

describe('say', () => {
  it('shows the core sentence in English, whatever the phrase says', () => {
    expect(say(APPLIED, 'Applied registry value HKCU\\X = REG_SZ (5 chars)')).toBe(
      'Applied registry value HKCU\\X = REG_SZ (5 chars)',
    );
  });

  it('writes Vietnamese from the code and the parameters, nested phrases included', () => {
    setLocale('vi');
    expect(say(APPLIED, 'english')).toBe('Đã áp dụng giá trị registry HKCU\\X = REG_SZ (5 ký tự)');
  });

  it('falls back to the core sentence without a phrase or with a code it does not know', () => {
    setLocale('vi');
    expect(say(null, 'Stored before phrases')).toBe('Stored before phrases');
    expect(say(phrase('summary.future'), 'A newer sentence')).toBe('A newer sentence');
    const nested = phrase('summary.applied', {
      subject: { kind: 'phrase', value: phrase('subject.future') },
      value: { kind: 'phrase', value: phrase('value.exists') },
    });
    expect(say(nested, 'Applied a future subject = exists')).toBe(
      'Applied a future subject = exists',
    );
  });

  it('formats sizes and numbers for the language, and joins lists', () => {
    setLocale('vi');
    const memory = phrase('summary.memory.free', {
      before: { kind: 'size', value: 1.5 * 1024 ** 3 },
      after: { kind: 'size', value: 2 * 1024 ** 3 },
      standbyBefore: { kind: 'size', value: 1024 ** 2 },
      standbyAfter: { kind: 'size', value: 0 },
    });
    expect(say(memory, 'x')).toBe('bộ nhớ trống từ 1,5 GB đến 2,0 GB, standby từ 1,0 MB đến 0 B');
    const skipped = phrase('scan.skipped', {
      parts: {
        kind: 'list',
        value: {
          sep: ', ',
          items: [
            phrase('scan.tooNew', { count: count(12) }),
            phrase('scan.links', { count: count(1) }),
          ],
        },
      },
    });
    expect(say(skipped, 'x')).toBe('12 mục quá mới, 1 liên kết được bỏ qua');
  });

  it('writes nothing for an optional part the core left empty', () => {
    setLocale('vi');
    const ended = phrase('summary.process', {
      name: text('chrome'),
      ended: count(3),
      total: count(4),
      admin: text(''),
      gone: { kind: 'phrase', value: phrase('summary.process.gone', { count: count(1) }) },
      failed: text(''),
    });
    expect(say(ended, 'x')).toBe(
      'Đã kết thúc chrome (không thể hoàn tác): 3/4 tiến trình; 1 đã tự thoát từ trước',
    );
  });

  it('starts a subject with a capital letter', () => {
    setLocale('vi');
    const what = phrase('subject.startupEntry', { name: text('Teams') });
    expect(sayCapitalised(what, 'Startup entry Teams')).toBe('Mục khởi động Teams');
    setLocale('en');
    expect(sayCapitalised(what, 'startup entry Teams')).toBe('Startup entry Teams');
  });
});

describe('say for the last sentences the core wrote in English only', () => {
  it('writes why a line cannot be undone and what a tweak will change in Vietnamese', () => {
    setLocale('vi');
    expect(
      say(phrase('undo.unconfirmed'), 'Volt has not confirmed that this change happened.'),
    ).toBe('Volt chưa xác nhận thay đổi này đã diễn ra.');
    expect(say(phrase('preview.keyValues', { count: count(1) }), 'key exists, 1 value')).toBe(
      'khóa tồn tại, 1 giá trị',
    );
    expect(say(phrase('preview.text', { text: text('REG_DWORD 1') }), 'x')).toBe('REG_DWORD 1');
    expect(say(phrase('preview.command', { command: text('sc stop X') }), 'x')).toBe(
      'lệnh sc stop X',
    );
  });
});
