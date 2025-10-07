#!/usr/bin/env node
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';

const huskyBin = path.join('node_modules', '.bin', process.platform === 'win32' ? 'husky.cmd' : 'husky');

if (!existsSync(huskyBin)) {
  console.warn('[prepare] Husky не установлен, пропускаю настройку git-хуков.');
  process.exit(0);
}

const result = spawnSync(huskyBin, { stdio: 'inherit', shell: process.platform === 'win32' });

if (result.error) {
  console.error('[prepare] Не удалось запустить Husky:', result.error.message);
  process.exit(result.status ?? 1);
}

process.exit(result.status ?? 0);
