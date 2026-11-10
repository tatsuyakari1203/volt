import type { ButtonHTMLAttributes } from 'react';
import { cx } from './cx';

export interface SwitchProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onChange' | 'role' | 'children'> {
  /** Current state. The component is controlled. */
  checked: boolean;
  /** Called with the next state when the user toggles with mouse, Space or Enter. */
  onCheckedChange: (checked: boolean) => void;
}

/**
 * On/off switch. Renders a native button with `role="switch"`, so Space and Enter toggle it and
 * it sits in the normal tab order. Give it an accessible name with `aria-label` or
 * `aria-labelledby`.
 */
export function Switch({ checked, onCheckedChange, className, onClick, ...rest }: SwitchProps) {
  return (
    <button
      {...rest}
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) onCheckedChange(!checked);
      }}
      className={cx(
        'relative h-6 w-11 shrink-0 rounded-full border transition-colors duration-(--duration-fast) ease-standard',
        'disabled:pointer-events-none disabled:opacity-50',
        checked ? 'border-accent bg-accent' : 'border-line-strong bg-transparent hover:bg-hover',
        className,
      )}
    >
      <span
        aria-hidden
        className={cx(
          'absolute top-1/2 left-[3px] size-4 -translate-y-1/2 rounded-full transition-transform duration-(--duration-fast) ease-standard',
          checked ? 'translate-x-5 bg-accent-fg' : 'translate-x-0 bg-line-strong',
        )}
      />
    </button>
  );
}
