import type { RiskLevel } from '../../shared/ui';

/** SAMPLE DATA for the design mock. These are not real tweak definitions. */
export interface SampleTweak {
  id: string;
  title: string;
  description: string;
  risk: RiskLevel;
  enabled: boolean;
}

export const sampleTweaks: readonly SampleTweak[] = [
  {
    id: 'file-extensions',
    title: 'Show file extensions',
    description: 'Always show .exe, .pdf and other endings in File Explorer.',
    risk: 'safe',
    enabled: true,
  },
  {
    id: 'start-web-search',
    title: 'Turn off web results in Start',
    description: 'Start search looks only at your PC, not the web.',
    risk: 'safe',
    enabled: false,
  },
  {
    id: 'tips',
    title: 'Turn off tips and suggestions',
    description: 'Stops Windows from showing tips and promoted apps.',
    risk: 'safe',
    enabled: false,
  },
  {
    id: 'classic-menu',
    title: 'Classic right-click menu',
    description: 'Skip "Show more options" and open the full menu directly.',
    risk: 'safe',
    enabled: false,
  },
  {
    id: 'game-dvr',
    title: 'Turn off Game DVR',
    description: 'Stops background game recording that can lower frame rates.',
    risk: 'caution',
    enabled: false,
  },
  {
    id: 'hibernation',
    title: 'Turn off hibernation',
    description: 'Frees disk space but removes Fast Startup and Hibernate.',
    risk: 'caution',
    enabled: false,
  },
  {
    id: 'sysmain',
    title: 'Stop the SysMain service',
    description: 'Disables prefetching. Can slow app launches on hard drives.',
    risk: 'advanced',
    enabled: false,
  },
  {
    id: 'telemetry-tasks',
    title: 'Disable diagnostic scheduled tasks',
    description: 'Turns off several Microsoft data collection tasks.',
    risk: 'advanced',
    enabled: false,
  },
];
