import { SearchX } from 'lucide-react';
import { useMemo, useRef, useState } from 'react';
import {
  Badge,
  Button,
  Card,
  Dialog,
  ListGroup,
  ListRow,
  PageHeader,
  SearchField,
  Switch,
  useToast,
} from '../../shared/ui';
import { type SampleTweak, sampleTweaks } from './sampleData';

/**
 * Tweaks screen mock: a searchable list with a switch, risk badge and one-line description.
 * Static sample data held in local state. Nothing is applied to the system. Turning on an
 * Advanced tweak asks for confirmation, to show the Dialog.
 */
export function TweaksScreen() {
  const toast = useToast();
  const [query, setQuery] = useState('');
  const [enabled, setEnabled] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(sampleTweaks.map((tweak) => [tweak.id, tweak.enabled])),
  );
  const [pending, setPending] = useState<SampleTweak | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return sampleTweaks;
    return sampleTweaks.filter((tweak) =>
      `${tweak.title} ${tweak.description} ${tweak.risk}`.toLowerCase().includes(needle),
    );
  }, [query]);

  const set = (tweak: SampleTweak, value: boolean, announce = true) => {
    setEnabled((current) => ({ ...current, [tweak.id]: value }));
    if (announce) {
      toast.show({
        message: `Sample only: "${tweak.title}" turned ${value ? 'on' : 'off'}.`,
        actionLabel: 'Undo',
        onAction: () => set(tweak, !value, false),
      });
    }
  };

  const toggle = (tweak: SampleTweak, value: boolean) => {
    if (value && tweak.risk === 'advanced') {
      setPending(tweak);
      return;
    }
    set(tweak, value);
  };

  return (
    <>
      <PageHeader
        title="Tweaks"
        description="Every change can be undone."
        actions={<Badge>Sample data</Badge>}
      />
      <div className="flex flex-col gap-3">
        <SearchField
          label="Search tweaks"
          placeholder="Search tweaks"
          value={query}
          onValueChange={setQuery}
        />
        {visible.length > 0 ? (
          <ListGroup aria-label="Tweaks">
            {visible.map((tweak) => (
              <ListRow
                key={tweak.id}
                title={tweak.title}
                description={tweak.description}
                badge={<Badge risk={tweak.risk} />}
                trailing={
                  <Switch
                    aria-label={tweak.title}
                    checked={enabled[tweak.id] ?? false}
                    onCheckedChange={(value) => toggle(tweak, value)}
                  />
                }
              />
            ))}
          </ListGroup>
        ) : (
          <Card className="flex flex-col items-center gap-2 py-10 text-center">
            <SearchX aria-hidden size={24} className="text-muted" />
            <p className="font-semibold">No tweaks match "{query.trim()}"</p>
            <p className="text-body text-muted">Try a different word.</p>
          </Card>
        )}
      </div>
      <Dialog
        open={pending !== null}
        onClose={() => setPending(null)}
        title="Turn on an Advanced tweak?"
        description={
          pending
            ? `"${pending.title}" can change how Windows behaves. Volt records the old value so you can undo it.`
            : undefined
        }
        initialFocusRef={cancelRef}
        footer={
          <>
            <Button ref={cancelRef} onClick={() => setPending(null)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                if (pending) set(pending, true);
                setPending(null);
              }}
            >
              Turn on
            </Button>
          </>
        }
      />
    </>
  );
}
