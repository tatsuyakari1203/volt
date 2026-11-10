import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '../../shared/i18n';
import type { CleaningRuleInfo, CleanRunResult } from '../../shared/ipc';
import { ToastProvider } from '../../shared/ui';
import { CleanScreen } from './CleanScreen';
import { resultMessage } from './messages';

const ipc = vi.hoisted(() => ({
  listCleaningRules: vi.fn(),
  scanCleaning: vi.fn(),
  cancelCleaningScan: vi.fn(),
  scanCleaningRules: vi.fn(),
  onCleanScanProgress: vi.fn(),
  cleanSelectedRules: vi.fn(),
  getRecycleBinStatus: vi.fn(),
  emptyRecycleBin: vi.fn(),
}));
vi.mock('../../shared/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/ipc')>()),
  ...ipc,
}));

const temp: CleaningRuleInfo = {
  id: 'user-temp',
  title: 'Temporary files',
  description: 'Files programs left in your temporary folder.',
  category: 'temporaryFiles',
  categoryLabel: 'Temporary files',
  risk: 'safe',
  needsAdministrator: false,
  minAgeDays: 7,
  pickItems: false,
  needsFolder: false,
  chosenFolder: null,
  follows: [],
};

beforeEach(() => {
  Object.values(ipc).forEach((mock) => {
    mock.mockReset();
  });
  ipc.getRecycleBinStatus.mockResolvedValue({ itemCount: 0, sizeBytes: 0 });
  setLocale('vi');
});

it('lists a rule and its group in Vietnamese, filed under the rule id and the category', async () => {
  const user = userEvent.setup();
  ipc.listCleaningRules.mockResolvedValue([temp]);
  render(
    <ToastProvider>
      <CleanScreen />
    </ToastProvider>,
  );
  const group = await screen.findByRole('region', { name: 'Tệp tạm' });
  // The group heading and the rule carry the same name.
  expect(within(group).getAllByText('Tệp tạm')).toHaveLength(2);
  await user.click(within(group).getByText('Đây là gì?'));
  expect(within(group).getByText(/Các tệp mà chương trình và trình cài đặt để lại/)).toBeTruthy();
  expect(within(group).getByText('Chỉ lấy các mục không thay đổi trong 7 ngày.')).toBeTruthy();
  expect(screen.getByText('Thùng rác đang trống.')).toBeTruthy();
});

it('puts the result of a clean together from its totals', () => {
  const result: CleanRunResult = {
    message: 'Moved 3 files to the Recycle Bin (3.0 MB). No disk space is freed until emptied.',
    rules: [],
    skipped: [],
    movedFiles: 3,
    movedFolders: 1,
    movedBytes: 3 * 1024 ** 2,
    skippedTotal: 2,
    historyIncomplete: false,
  };
  expect(resultMessage(result)).toBe(
    'Đã chuyển 3 tệp và 1 thư mục trống vào Thùng rác (3,0 MB). 2 mục được để nguyên. Dung lượng ổ đĩa chỉ được giải phóng sau khi bạn dọn Thùng rác.',
  );
  setLocale('en');
  expect(resultMessage(result)).toBe(result.message);
});
