import type { ReactNode } from 'react';

export interface ProgressRingProps {
  /** 0 to 100. Values outside the range are clamped. */
  value: number;
  /** Accessible name, for example "Health score". */
  label: string;
  /** Outer size in px. Defaults to 168. */
  size?: number;
  /** Ring thickness in px. Defaults to 12. */
  strokeWidth?: number;
  /** Content centred inside the ring, such as the score. */
  children?: ReactNode;
}

/** Circular progress indicator exposed to assistive tech as a progressbar. */
export function ProgressRing({
  value,
  label,
  size = 168,
  strokeWidth = 12,
  children,
}: ProgressRingProps) {
  const clamped = Math.min(100, Math.max(0, Math.round(value)));
  const radius = (size - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamped}
      className="relative shrink-0"
      style={{ width: size, height: size }}
    >
      <svg
        width={size}
        height={size}
        viewBox={`0 0 ${size} ${size}`}
        aria-hidden
        className="-rotate-90"
      >
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          strokeWidth={strokeWidth}
          className="stroke-line"
        />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          strokeWidth={strokeWidth}
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - clamped / 100)}
          className="stroke-accent transition-[stroke-dashoffset] duration-(--duration-base) ease-standard"
        />
      </svg>
      <div className="absolute inset-0 grid place-items-center text-center">{children}</div>
    </div>
  );
}
