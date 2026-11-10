import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it } from 'vitest';
import { Button } from './Button';
import { Dialog } from './Dialog';

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button onClick={() => setOpen(true)}>Open dialog</Button>
      <Dialog
        open={open}
        onClose={() => setOpen(false)}
        title="Turn on an Advanced tweak?"
        description="Volt records the old value."
        footer={
          <>
            <Button onClick={() => setOpen(false)}>Cancel</Button>
            <Button variant="primary" onClick={() => setOpen(false)}>
              Confirm
            </Button>
          </>
        }
      />
    </>
  );
}

async function openDialog() {
  const user = userEvent.setup();
  render(<Harness />);
  const opener = screen.getByRole('button', { name: 'Open dialog' });
  opener.focus();
  await user.keyboard('{Enter}');
  return { user, opener };
}

it('exposes a named modal dialog and moves focus into it', async () => {
  await openDialog();
  const dialog = screen.getByRole('dialog', { name: 'Turn on an Advanced tweak?' });
  expect(dialog.getAttribute('aria-modal')).toBe('true');
  expect(dialog.getAttribute('aria-describedby')).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' }));
});

it('traps Tab and Shift+Tab inside the dialog', async () => {
  const { user } = await openDialog();
  const cancel = screen.getByRole('button', { name: 'Cancel' });
  const confirm = screen.getByRole('button', { name: 'Confirm' });

  await user.tab();
  expect(document.activeElement).toBe(confirm);
  await user.tab();
  expect(document.activeElement).toBe(cancel);
  await user.tab({ shift: true });
  expect(document.activeElement).toBe(confirm);
});

it('closes on Escape and returns focus to the opener', async () => {
  const { user, opener } = await openDialog();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(opener);
});

it('returns focus to the opener after a button closes it', async () => {
  const { user, opener } = await openDialog();
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(opener);
});
