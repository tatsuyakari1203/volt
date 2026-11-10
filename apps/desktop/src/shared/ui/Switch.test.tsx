import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { Switch } from './Switch';

function Harness({ onChange }: { onChange?: (value: boolean) => void }) {
  const [on, setOn] = useState(false);
  return (
    <Switch
      aria-label="Show file extensions"
      checked={on}
      onCheckedChange={(value) => {
        setOn(value);
        onChange?.(value);
      }}
    />
  );
}

it('is reachable with Tab and toggles with Space and Enter', async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  render(<Harness onChange={onChange} />);
  const toggle = screen.getByRole('switch', { name: 'Show file extensions' });
  expect(toggle.getAttribute('aria-checked')).toBe('false');

  await user.tab();
  expect(document.activeElement).toBe(toggle);

  await user.keyboard(' ');
  expect(toggle.getAttribute('aria-checked')).toBe('true');
  await user.keyboard('{Enter}');
  expect(toggle.getAttribute('aria-checked')).toBe('false');
  expect(onChange.mock.calls).toEqual([[true], [false]]);
});

it('does not toggle while disabled', async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  render(<Switch aria-label="Locked" checked={false} disabled onCheckedChange={onChange} />);
  await user.click(screen.getByRole('switch', { name: 'Locked' }));
  expect(onChange).not.toHaveBeenCalled();
});
