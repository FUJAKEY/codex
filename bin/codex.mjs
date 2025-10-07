#!/usr/bin/env node
// Прокси-исполнитель, который делегирует запуск в собранный CLI Codex.
import { fileURLToPath, pathToFileURL } from "url";
import path from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const cliEntry = pathToFileURL(path.join(__dirname, "../codex-cli/bin/codex.js"));

await import(cliEntry.href);
