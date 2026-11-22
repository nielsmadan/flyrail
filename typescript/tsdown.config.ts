import { defineConfig } from 'tsdown';

export default defineConfig({
  entry: ['src/index.ts', 'src/errors.ts'],
  unbundle: true,
  format: 'esm',
  platform: 'node',
  dts: true,
  outDir: 'dist',
  fixedExtension: false,
});
