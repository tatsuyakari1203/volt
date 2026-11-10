import { Loader2 } from 'lucide-react';
import type { ComponentProps, MouseEvent, ReactNode } from 'react';
import { cx } from './cx';

/** Visual weight of a button. Use one primary per view. */
export type ButtonVariant = 'primary' | 'secondary' | 'ghost';
/** Button height and type size. */
export type ButtonSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends ComponentProps<'button'> {
  /** Defaults to `secondary`. */
  variant?: ButtonVariant;
  /** Defaults to `md`. */
  size?: ButtonSize;
  /** Shows a spinner and ignores clicks while keeping keyboard focus. */
  loading?: boolean;
  /** Optional icon shown before the label. Decorative: the label names the button. */
  icon?: ReactNode;
}

const variants: Record<ButtonVariant, string> = {
  primary: 'bg-accent text-accent-fg hover:bg-accent-hover active:bg-accent-pressed',
  secondary:
    'border border-line bg-surface text-fg hover:bg-surface-hover active:bg-surface-sunken',
  ghost: 'text-fg hover:bg-hover active:bg-pressed',
};

const sizes: Record<ButtonSize, string> = {
  sm: 'h-8 px-3 text-body',
  md: 'h-9 px-4 text-body',
  lg: 'h-12 px-6 text-subtitle',
};

/** Button with primary, secondary and ghost variants. Loading keeps focus and blocks clicks. */
export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  icon,
  className,
  children,
  onClick,
  type = 'button',
  ...rest
}: ButtonProps) {
  const handleClick = (event: MouseEvent<HTMLButtonElement>) => {
    if (loading) {
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };
  return (
    <button
      {...rest}
      type={type}
      aria-busy={loading || undefined}
      aria-disabled={loading || undefined}
      onClick={handleClick}
      className={cx(
        'inline-flex shrink-0 select-none items-center justify-center gap-2 rounded-md font-semibold',
        'transition-colors duration-(--duration-fast) ease-standard',
        'disabled:pointer-events-none disabled:opacity-50 aria-disabled:cursor-progress',
        variants[variant],
        sizes[size],
        className,
      )}
    >
      {loading ? <Loader2 aria-hidden size={18} className="motion-safe:animate-spin" /> : icon}
      {children}
    </button>
  );
}
