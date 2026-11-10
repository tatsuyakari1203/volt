import { Search } from 'lucide-react';
import type { InputHTMLAttributes } from 'react';
import { cx } from './cx';

export interface SearchFieldProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type' | 'onChange' | 'value'> {
  /** Accessible name. Not shown visually; the placeholder carries the hint. */
  label: string;
  value: string;
  onValueChange: (value: string) => void;
}

/** Search input with a leading icon. */
export function SearchField({ label, value, onValueChange, className, ...rest }: SearchFieldProps) {
  return (
    <label className={cx('relative block', className)}>
      <span className="sr-only">{label}</span>
      <Search
        aria-hidden
        size={16}
        className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted"
      />
      <input
        {...rest}
        type="search"
        value={value}
        onChange={(event) => onValueChange(event.target.value)}
        className="h-10 w-full rounded-md border border-line-strong bg-surface pr-3 pl-9 text-fg transition-colors duration-(--duration-fast) placeholder:text-muted hover:bg-surface-hover focus:border-accent"
      />
    </label>
  );
}
