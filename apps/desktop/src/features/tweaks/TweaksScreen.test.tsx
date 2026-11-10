import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { ToastProvider } from '../../shared/ui';
import { TweaksScreen } from './TweaksScreen';

function renderScreen() {
  return render(
    <ToastProvider>
      <TweaksScreen />
    </ToastProvider>,
  );
}

it('filters the list as the user types and says when nothing matches', async () => {
  const user = userEvent.setup();
  renderScreen();
  const list = screen.getByRole('list', { name: 'Tweaks' });
  expect(within(list).getAllByRole('listitem').length).toBeGreaterThan(5);

  await user.type(screen.getByRole('searchbox', { name: 'Search tweaks' }), 'hibernation');
  expect(within(list).getAllByRole('listitem')).toHaveLength(1);

  await user.clear(screen.getByRole('searchbox', { name: 'Search tweaks' }));
  await user.type(screen.getByRole('searchbox', { name: 'Search tweaks' }), 'zzzz');
  expect(screen.queryByRole('list', { name: 'Tweaks' })).toBeNull();
  expect(screen.getByText(/No tweaks match/)).toBeTruthy();
});

it('toggling a Safe tweak shows a toast whose Undo flips it back', async () => {
  const user = userEvent.setup();
  renderScreen();
  const toggle = screen.getByRole('switch', { name: 'Turn off tips and suggestions' });
  expect(toggle.getAttribute('aria-checked')).toBe('false');

  await user.click(toggle);
  expect(toggle.getAttribute('aria-checked')).toBe('true');
  await user.click(screen.getByRole('button', { name: 'Undo' }));
  expect(toggle.getAttribute('aria-checked')).toBe('false');
});

it('an Advanced tweak asks first, and Cancel leaves it off and returns focus to the switch', async () => {
  const user = userEvent.setup();
  renderScreen();
  const toggle = screen.getByRole('switch', { name: 'Stop the SysMain service' });
  toggle.focus();
  await user.keyboard(' ');

  expect(screen.getByRole('dialog', { name: 'Turn on an Advanced tweak?' })).toBeTruthy();
  expect(toggle.getAttribute('aria-checked')).toBe('false');
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(toggle.getAttribute('aria-checked')).toBe('false');
  expect(document.activeElement).toBe(toggle);

  await user.keyboard(' ');
  await user.click(screen.getByRole('button', { name: 'Turn on' }));
  expect(toggle.getAttribute('aria-checked')).toBe('true');
});
