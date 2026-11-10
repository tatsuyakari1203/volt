import type { ReactNode } from 'react';

/** De-emphasised text. Uses the muted token, which meets AA contrast on every surface. */
export function Muted({ children }: { children: ReactNode }) {
  return <p className="text-body text-muted">{children}</p>;
}
