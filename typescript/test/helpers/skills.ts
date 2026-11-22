import { Bundle, fileEntry } from 'flyrail';

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);

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
