import type { RiskLevel } from '../../shared/ui';

/** SAMPLE DATA for the design mock. Nothing here is measured from a real machine. */
export const sampleHealth = { score: 82, label: 'Good' } as const;

export interface SampleMetric {
  id: 'memory' | 'junk' | 'startup' | 'disk';
  label: string;
  value: string;
  caption: string;
}

export const sampleMetrics: readonly SampleMetric[] = [
  { id: 'memory', label: 'Memory', value: '62%', caption: '9.9 of 16 GB in use' },
  { id: 'junk', label: 'Junk files', value: '4.2 GB', caption: 'Can be cleaned safely' },
  { id: 'startup', label: 'Startup apps', value: '11', caption: '3 slow down sign-in' },
  { id: 'disk', label: 'Disk space', value: '182 GB', caption: 'Free of 476 GB on C:' },
];

export interface SampleRecommendation {
  id: string;
  title: string;
  description: string;
  risk: RiskLevel;
}

export const sampleRecommendations: readonly SampleRecommendation[] = [
  {
    id: 'clean-temp',
    title: 'Clean temporary files',
    description: 'Frees about 4.2 GB. You review the list first.',
    risk: 'safe',
  },
  {
    id: 'release-memory',
    title: 'Release unused memory',
    description: 'Gives back memory Windows is holding in reserve.',
    risk: 'safe',
  },
  {
    id: 'startup-apps',
    title: 'Pause 3 startup apps',
    description: 'Faster sign-in. You can turn them back on.',
    risk: 'caution',
  },
];
