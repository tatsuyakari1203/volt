import type { HTMLAttributes } from 'react';
import { cx } from './cx';

export interface CardProps extends HTMLAttributes<HTMLElement> {
  /** Element to render. Use `section` with an accessible name for landmark-like groups. */
  as?: 'div' | 'section' | 'article';
  /** `none` is for cards that hold a list and manage their own padding. Defaults to `md`. */
  padding?: 'none' | 'md';
}

/** Raised surface with a subtle border. Elevation comes from the border, not a heavy shadow. */
export function Card({ as: Tag = 'div', padding = 'md', className, ...rest }: CardProps) {
  return (
    <Tag
      {...rest}
      className={cx(
        'rounded-md border border-line bg-surface shadow-card',
        padding === 'md' && 'p-4',
        className,
      )}
    />
  );
}
