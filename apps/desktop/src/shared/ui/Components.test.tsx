import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Badge } from './Badge';
import { Button } from './Button';
import { ErrorBoundary } from './ErrorBoundary';
import { ProgressRing } from './ProgressRing';
import { SegmentedControl } from './SegmentedControl';

it('button: loading marks it busy, keeps focus and ignores clicks', async () => {
  const user = userEvent.setup();
  const onClick = vi.fn();
  render(
    <Button loading onClick={onClick}>
      Boost
    </Button>,
  );
  const button = screen.getByRole('button', { name: 'Boost' });
  expect(button.getAttribute('aria-busy')).toBe('true');
  await user.tab();
  expect(document.activeElement).toBe(button);
  await user.keyboard('{Enter}');
  await user.click(button);
  expect(onClick).not.toHaveBeenCalled();
});

it('button: fires onClick from the keyboard when idle', async () => {
  const user = userEvent.setup();
  const onClick = vi.fn();
  render(<Button onClick={onClick}>Review</Button>);
  await user.tab();
  await user.keyboard(' ');
  expect(onClick).toHaveBeenCalledTimes(1);
});

it('badge: risk levels carry a text label, not only a colour', () => {
  render(
    <>
      <Badge risk="safe" />
      <Badge risk="caution" />
      <Badge risk="advanced" />
    </>,
  );
  for (const label of ['Safe', 'Caution', 'Advanced']) {
    expect(screen.getByText(label)).toBeTruthy();
  }
});

it('progress ring: exposes a clamped value as a progressbar', () => {
  render(<ProgressRing value={140} label="Health score" />);
  const ring = screen.getByRole('progressbar', { name: 'Health score' });
  expect(ring.getAttribute('aria-valuenow')).toBe('100');
  expect(ring.getAttribute('aria-valuemin')).toBe('0');
  expect(ring.getAttribute('aria-valuemax')).toBe('100');
});

it('segmented control: arrow keys change the selection', async () => {
  const user = userEvent.setup();
  const onValueChange = vi.fn();
  render(
    <SegmentedControl
      label="Theme"
      value="system"
      onValueChange={onValueChange}
      options={[
        { value: 'system', label: 'System' },
        { value: 'light', label: 'Light' },
        { value: 'dark', label: 'Dark' },
      ]}
    />,
  );
  expect(screen.getByRole('radiogroup', { name: 'Theme' })).toBeTruthy();
  await user.tab();
  await user.keyboard('{ArrowRight}');
  expect(onValueChange).toHaveBeenCalledWith('light');
});

function Boom(): never {
  throw new Error('boom');
}

it('error boundary: shows the fallback when a child throws', () => {
  const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
  render(
    <ErrorBoundary fallback={<p>Fallback</p>}>
      <Boom />
    </ErrorBoundary>,
  );
  spy.mockRestore();
  expect(screen.getByText('Fallback')).toBeTruthy();
});
