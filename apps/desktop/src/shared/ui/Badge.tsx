import { Flame, ShieldCheck, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { type MessageKey, t } from '../i18n';
import { cx } from './cx';

/** Risk level of a tweak. Mirrors `Risk` in volt-core. */
export type RiskLevel = 'safe' | 'caution' | 'advanced';

const risks: Record<RiskLevel, { label: MessageKey; icon: ReactNode; className: string }> = {
  safe: {
    label: 'risk.safe',
    icon: <ShieldCheck aria-hidden size={12} />,
    className: 'bg-safe text-safe-fg',
  },
  caution: {
    label: 'risk.caution',
    icon: <TriangleAlert aria-hidden size={12} />,
    className: 'bg-caution text-caution-fg',
  },
  advanced: {
    label: 'risk.advanced',
    icon: <Flame aria-hidden size={12} />,
    className: 'bg-advanced text-advanced-fg',
  },
};

export interface BadgeProps {
  /** Risk level badge: coloured, with an icon so colour is never the only signal. */
  risk?: RiskLevel;
  /** Label for a neutral badge. For risk badges it overrides the default label. */
  children?: ReactNode;
}

/** Small status pill. Use `risk` for Safe, Caution and Advanced; plain children for neutral. */
export function Badge({ risk, children }: BadgeProps) {
  const spec = risk ? risks[risk] : null;
  return (
    <span
      className={cx(
        'inline-flex items-center gap-1 rounded-sm px-1.5 py-px text-caption font-semibold',
        spec ? spec.className : 'border border-line bg-surface-sunken text-muted',
      )}
    >
      {spec?.icon}
      {children ?? (spec ? t(spec.label) : null)}
    </span>
  );
}
