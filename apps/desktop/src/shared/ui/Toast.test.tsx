import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ToastProvider, useToast } from './Toast';

function Trigger({ onUndo }: { onUndo: () => void }) {
  const toast = useToast();
  return (
    <button
      type="button"
      onClick={() =>
        toast.show({ message: 'Tweak turned on.', actionLabel: 'Undo', onAction: onUndo })
      }
    >
      Show
    </button>
  );
}

afterEach(() => {
  vi.useRealTimers();
});

it('calls the undo callback and dismisses the toast', async () => {
  const user = userEvent.setup();
  const onUndo = vi.fn();
  render(
    <ToastProvider>
      <Trigger onUndo={onUndo} />
    </ToastProvider>,
  );
  await user.click(screen.getByRole('button', { name: 'Show' }));
  expect(screen.getByText('Tweak turned on.')).toBeTruthy();

  await user.click(screen.getByRole('button', { name: 'Undo' }));
  expect(onUndo).toHaveBeenCalledTimes(1);
  expect(screen.queryByText('Tweak turned on.')).toBeNull();
});

it('is announced through a polite live region and can be dismissed', async () => {
  const user = userEvent.setup();
  render(
    <ToastProvider>
      <Trigger onUndo={() => {}} />
    </ToastProvider>,
  );
  await user.click(screen.getByRole('button', { name: 'Show' }));
  const region = screen.getByRole('region', { name: 'Notifications' });
  expect(region.getAttribute('aria-live')).toBe('polite');
  expect(region.textContent).toContain('Tweak turned on.');

  await user.click(screen.getByRole('button', { name: 'Dismiss notification' }));
  expect(screen.queryByText('Tweak turned on.')).toBeNull();
});

it('goes away by itself after its delay without calling the action', () => {
  vi.useFakeTimers();
  const onUndo = vi.fn();
  render(
    <ToastProvider>
      <Trigger onUndo={onUndo} />
    </ToastProvider>,
  );
  act(() => screen.getByRole('button', { name: 'Show' }).click());
  expect(screen.getByText('Tweak turned on.')).toBeTruthy();
  act(() => vi.advanceTimersByTime(10_000));
  expect(screen.queryByText('Tweak turned on.')).toBeNull();
  expect(onUndo).not.toHaveBeenCalled();
});
