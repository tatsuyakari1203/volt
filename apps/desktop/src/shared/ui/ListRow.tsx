import type { HTMLAttributes, ReactNode } from 'react';
import { cx } from './cx';

/** A card-like list: rows separated by hairlines. Pass `ListRow` children. */
export function ListGroup({ className, ...rest }: HTMLAttributes<HTMLUListElement>) {
  return (
    <ul
      {...rest}
      className={cx(
        'divide-y divide-line overflow-hidden rounded-md border border-line bg-surface shadow-card',
        className,
      )}
    />
  );
}

export interface ListRowProps {
  /** Leading visual, usually an icon. Decorative. */
  icon?: ReactNode;
  /** Primary line. */
  title: ReactNode;
  /** Optional badge shown next to the title, such as a risk level. */
  badge?: ReactNode;
  /** One-line supporting text. */
  description?: ReactNode;
  /** Trailing control: a switch, button or value. */
  trailing?: ReactNode;
}

/** One row inside a `ListGroup`: icon, title with badge, description and a trailing control. */
export function ListRow({ icon, title, badge, description, trailing }: ListRowProps) {
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      {icon ? (
        <span
          aria-hidden
          className="grid size-9 shrink-0 place-items-center rounded-md bg-accent-soft text-accent-text"
        >
          {icon}
        </span>
      ) : null}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-semibold">{title}</span>
          {badge}
        </div>
        {description ? <div className="mt-0.5 text-caption text-muted">{description}</div> : null}
      </div>
      {trailing ? <div className="shrink-0">{trailing}</div> : null}
    </li>
  );
}
