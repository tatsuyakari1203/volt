import { Badge, Card, PageHeader } from '../shared/ui';
import type { Section } from './sections';

/** Plain-language description of what each not-yet-built section will hold. */
const plans: Record<string, string> = {
  health: 'Check disk, battery and system health, and see what changed since the last check.',
};

/** Stand-in for a section that is not built yet. It says what will live there. */
export function PlaceholderScreen({ section }: { section: Section }) {
  const Icon = section.icon;
  return (
    <>
      <PageHeader title={section.label} />
      <Card className="flex flex-col items-center gap-3 px-6 py-10 text-center">
        <span
          aria-hidden
          className="grid size-12 place-items-center rounded-md bg-accent-soft text-accent-text"
        >
          <Icon size={24} strokeWidth={1.75} />
        </span>
        <Badge>Coming soon</Badge>
        <p className="max-w-xs text-body text-muted">{plans[section.id] ?? ''}</p>
      </Card>
    </>
  );
}
