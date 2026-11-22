import { Bundle, fileEntry } from 'flyrail';
import { encode } from './bytes.js';

export const skillBundle = (version = '1', id = 'team'): Bundle =>
  Bundle.fromMemory(
    encode(
      JSON.stringify({
        schema_version: 1,
        id,
        version,
        skills: [
          { name: 'review', path: 'review' },
          { name: 'deploy', path: 'deploy' },
        ],
      }),
    ),
    [
      fileEntry('review/SKILL.md', encode(`# Review ${version}\n`)),
      fileEntry('deploy/SKILL.md', encode(`# Deploy ${version}\n`)),
    ],
  );
