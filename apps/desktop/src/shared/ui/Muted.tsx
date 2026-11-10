import type { ReactNode } from 'react';

/** De-emphasised text. Placeholder until the design system lands (TASK-010). */
export function Muted({ children }: { children: ReactNode }) {
  return <p className="text-sm text-neutral-400">{children}</p>;
}
