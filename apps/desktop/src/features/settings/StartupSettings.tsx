import { Power } from 'lucide-react';
import { useEffect, useState } from 'react';
import { getAutostartEnabled, setAutostartEnabled } from '../../shared/ipc';
import { Card, ListGroup, ListRow, Muted, Switch } from '../../shared/ui';

/** `null` while the state is being read from the shell. */
type Known = boolean | null;

/**
 * Settings card for how Volt starts. Today it holds one switch, Start with Windows: off by
 * default, per-user, so turning it on never shows a UAC prompt. The state is read from and
 * written to the Rust shell; the switch shows what the shell reports, not what was asked.
 */
export function StartupSettings() {
  const [enabled, setEnabled] = useState<Known>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    getAutostartEnabled()
      .then((value) => {
        if (alive) setEnabled(value);
      })
      .catch((e: unknown) => {
        if (alive) setError(`Could not read the setting: ${String(e)}`);
      });
    return () => {
      alive = false;
    };
  }, []);

  function change(next: boolean) {
    setBusy(true);
    setError(null);
    setAutostartEnabled(next)
      .then(setEnabled)
      .catch((e: unknown) => setError(`Could not change the setting: ${String(e)}`))
      .finally(() => setBusy(false));
  }

  return (
    <Card as="section" aria-labelledby="settings-startup" padding="none">
      <h2 id="settings-startup" className="px-4 pt-4 pb-3 text-subtitle font-semibold">
        Startup
      </h2>
      <ListGroup className="rounded-none border-x-0 border-b-0 shadow-none">
        <ListRow
          icon={<Power aria-hidden size={18} />}
          title="Start with Windows"
          description="Opens Volt in the tray when you sign in. Needs no administrator approval."
          trailing={
            <Switch
              aria-label="Start with Windows"
              checked={enabled === true}
              disabled={enabled === null || busy}
              onCheckedChange={change}
            />
          }
        />
      </ListGroup>
      {error ? (
        <div role="alert" className="px-4 py-3">
          <Muted>{error}</Muted>
        </div>
      ) : null}
    </Card>
  );
}
