import type { ReactNode } from 'react';

export interface PageHeaderProps {
  /** Screen title, rendered as the page `h1`. */
  title: string;
  /** One supporting sentence. */
  description?: ReactNode;
  /** Optional trailing element, such as a badge. */
  actions?: ReactNode;
}

/** Title block at the top of a screen. */
export function PageHeader({ title, description, actions }: PageHeaderProps) {
  return (
    <header className="flex items-start justify-between gap-3 pb-4">
      <div className="min-w-0">
        <h1 className="text-title font-semibold">{title}</h1>
        {description ? <p className="mt-1 text-body text-muted">{description}</p> : null}
      </div>
      {actions ? <div className="shrink-0 pt-1.5">{actions}</div> : null}
    </header>
  );
}
