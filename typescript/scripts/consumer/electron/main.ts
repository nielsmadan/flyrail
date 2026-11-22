import { app } from 'electron';
import { join, resolve } from 'node:path';
import { runConsumer } from '../core.mjs';

const scratch = resolve(process.argv.at(-1) ?? '.');
app.setPath('userData', join(scratch, 'electron-user-data'));
app.dock?.hide();
app.on('window-all-closed', () => {});

const main = async (): Promise<void> => {
  try {
    console.log(JSON.stringify(await runConsumer(scratch, 'electron')));
    app.exit(0);
  } catch (error) {
    console.error(error);
    app.exit(1);
  }
};

void app.whenReady().then(main);
