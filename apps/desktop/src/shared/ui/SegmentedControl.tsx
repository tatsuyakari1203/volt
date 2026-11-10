import { type ReactNode, useId } from 'react';

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  icon?: ReactNode;
}

export interface SegmentedControlProps<T extends string> {
  /** Accessible name of the group. */
  label: string;
  options: ReadonlyArray<SegmentedOption<T>>;
  value: T;
  onValueChange: (value: T) => void;
}

/**
 * Mutually exclusive choice shown as a row of segments. Built on native radio inputs, so arrow
 * keys, grouping and screen-reader semantics come from the browser.
 */
export function SegmentedControl<T extends string>({
  label,
  options,
  value,
  onValueChange,
}: SegmentedControlProps<T>) {
  const name = useId();
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="inline-flex rounded-md border border-line bg-surface-sunken p-0.5"
    >
      {options.map((option) => (
        <label
          key={option.value}
          className="relative flex h-8 cursor-default items-center gap-1.5 rounded-sm px-3 text-body font-medium text-muted transition-colors duration-(--duration-fast) hover:text-fg has-checked:bg-surface has-checked:text-fg has-checked:shadow-card has-focus-visible:outline-2 has-focus-visible:outline-offset-1 has-focus-visible:outline-focus"
        >
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={option.value === value}
            onChange={() => onValueChange(option.value)}
            className="absolute inset-0 size-full cursor-default opacity-0 outline-none"
          />
          {option.icon}
          {option.label}
        </label>
      ))}
    </div>
  );
}
