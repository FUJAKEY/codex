import type { AppConfig } from "../utils/config.js";
import type { ResponseItem } from "openai/resources/responses/responses";

import { AgentLoop } from "../utils/agent/agent-loop.js";
import { ReviewDecision } from "../utils/agent/review.js";
import { checkLandlockSupport } from "../utils/agent/sandbox/landlock.js";
import { AutoApprovalMode } from "../utils/auto-approval-mode.js";
import { createInputItem } from "../utils/input-utils.js";
import { CLI_VERSION } from "../version.js";
import express from "express";
import open from "open";

type CodexWebServerOptions = {
  config: AppConfig;
  port: number;
  additionalWritableRoots: ReadonlyArray<string>;
  openBrowser: boolean;
};

type ServerContext = {
  cwd: string;
  model: string;
  provider: string;
  disableResponseStorage: boolean;
  version: string;
  sandboxStatus: "available" | "unavailable" | "not-applicable";
  sandboxMessage?: string;
};

const DEFAULT_PORT = 3210;

export async function runCodexWeb({
  config,
  port,
  additionalWritableRoots,
  openBrowser,
}: CodexWebServerOptions): Promise<never> {
  const app = express();
  app.disable("x-powered-by");
  app.use(express.json({ limit: "8mb" }));

  const sandboxStatus = await determineSandboxStatus();
  const context: ServerContext = {
    cwd: process.cwd(),
    model: config.model,
    provider: config.provider ?? "openai",
    disableResponseStorage: Boolean(config.disableResponseStorage),
    version: CLI_VERSION,
    sandboxStatus: sandboxStatus.status,
    sandboxMessage: sandboxStatus.message,
  };

  app.get("/health", (_req, res) => {
    res.json({ status: "ok", version: CLI_VERSION });
  });

  app.get("/", (_req, res) => {
    res.type("html").send(renderHtml(context));
  });

  let activeRun: Promise<void> | null = null;

  app.post("/api/run", async (req, res) => {
    if (activeRun) {
      res.status(409).json({ error: "Another run is already in progress." });
      return;
    }

    const { prompt, images, fullStdout } = req.body ?? {};
    const promptText = typeof prompt === "string" ? prompt.trim() : "";

    if (!promptText) {
      res.status(400).json({ error: "Prompt is required." });
      return;
    }

    const imagePaths: Array<string> = Array.isArray(images)
      ? images.filter((value: unknown): value is string => typeof value === "string")
      : [];

    const results: Array<ResponseItem> = [];
    const loadingStates: Array<{ type: string; data: unknown }> = [];
    let lastResponseId: string | undefined;

    const agent = new AgentLoop({
      model: config.model,
      provider: config.provider ?? "openai",
      instructions: config.instructions,
      approvalPolicy: AutoApprovalMode.FULL_AUTO,
      disableResponseStorage: config.disableResponseStorage,
      config,
      additionalWritableRoots,
      onItem: (item) => {
        results.push(item);
      },
      onLoading: (payload) => {
        loadingStates.push({ type: "loading", data: payload });
      },
      getCommandConfirmation: async () => ({
        review: ReviewDecision.YES,
      }),
      onLastResponseId: (id) => {
        lastResponseId = id;
      },
    });

    const runPromise = (async () => {
      const inputItem = await createInputItem(promptText, imagePaths);
      await agent.run([inputItem]);
    })();

    activeRun = runPromise.finally(() => {
      activeRun = null;
    });

    const startedAt = Date.now();
    try {
      await runPromise;
      res.json({
        items: results,
        loading: loadingStates,
        lastResponseId,
        durationMs: Date.now() - startedAt,
        sandbox: sandboxStatus,
        fullStdout: Boolean(fullStdout),
      });
    } catch (error) {
      res.status(500).json({
        error: error instanceof Error ? error.message : String(error),
        items: results,
        durationMs: Date.now() - startedAt,
        sandbox: sandboxStatus,
      });
    }
  });

  const serverPort = Number.isFinite(port) && port > 0 ? port : DEFAULT_PORT;
  const server = app.listen(serverPort, "127.0.0.1");

  await new Promise<void>((resolve, reject) => {
    server.on("listening", async () => {
      // eslint-disable-next-line no-console
      console.log(
        `🚀 Codex web UI available at http://localhost:${serverPort} (v${CLI_VERSION})`,
      );
      if (sandboxStatus.status === "unavailable" && sandboxStatus.message) {
        // eslint-disable-next-line no-console
        console.warn(`⚠️  Sandbox unavailable: ${sandboxStatus.message}`);
      }

      if (openBrowser) {
        try {
          await open(`http://localhost:${serverPort}`);
        } catch (error) {
          // eslint-disable-next-line no-console
          console.warn(
            `Unable to automatically open the browser: ${
              error instanceof Error ? error.message : error
            }`,
          );
        }
      }
    });
    server.on("error", (err) => {
      reject(err);
    });
  });

  const shutdown = () => {
    server.close(() => {
      process.exit(0);
    });
  };

  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);

  await new Promise<never>(() => {
    /* keep process alive */
  });
}

async function determineSandboxStatus(): Promise<
  | { status: "available" | "not-applicable"; message?: string }
  | { status: "unavailable"; message: string }
> {
  if (process.platform !== "linux") {
    return { status: "not-applicable" };
  }

  const support = await checkLandlockSupport();
  if (support.ok) {
    return { status: "available" };
  }

  return { status: "unavailable", message: support.error.message };
}

function renderHtml(context: ServerContext): string {
  const serializedContext = JSON.stringify(context).replace(/</g, "\\u003c");
  const accent = "#4f46e5";
  return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Codex Web • v${context.version}</title>
    <style>
      :root {
        color-scheme: dark;
        font-family: "Inter", system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        background: radial-gradient(circle at 20% 20%, #1f2937, #0f172a 55%);
        color: #f8fafc;
      }
      body {
        margin: 0;
        min-height: 100vh;
        display: flex;
        justify-content: center;
        padding: 48px 16px;
        box-sizing: border-box;
      }
      .container {
        width: min(960px, 100%);
        display: flex;
        flex-direction: column;
        gap: 32px;
      }
      header {
        backdrop-filter: blur(18px);
        background: rgba(30, 41, 59, 0.55);
        border: 1px solid rgba(148, 163, 184, 0.18);
        border-radius: 28px;
        padding: 32px;
        box-shadow: 0 24px 60px rgba(15, 23, 42, 0.45);
      }
      header h1 {
        margin: 0 0 8px;
        font-size: 2.2rem;
        letter-spacing: -0.02em;
      }
      header p {
        margin: 0;
        color: #cbd5f5;
        max-width: 640px;
        line-height: 1.6;
      }
      form {
        display: grid;
        gap: 24px;
        background: rgba(15, 23, 42, 0.75);
        border-radius: 24px;
        padding: 32px;
        border: 1px solid rgba(79, 70, 229, 0.28);
        box-shadow: 0 16px 50px rgba(59, 130, 246, 0.18);
      }
      label {
        display: grid;
        gap: 12px;
      }
      label span {
        font-weight: 600;
        color: #e2e8f0;
        display: flex;
        align-items: center;
        gap: 8px;
      }
      textarea {
        resize: vertical;
        min-height: 160px;
        padding: 16px;
        border-radius: 18px;
        border: 1px solid rgba(148, 163, 184, 0.32);
        background: rgba(15, 23, 42, 0.75);
        color: inherit;
        font-size: 1rem;
        box-shadow: inset 0 0 0 1px rgba(79, 70, 229, 0.1);
      }
      textarea:focus {
        outline: 2px solid ${accent};
        outline-offset: 2px;
      }
      .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
      }
      button {
        background: linear-gradient(135deg, ${accent}, #7c3aed);
        border: none;
        color: #fff;
        font-weight: 600;
        padding: 14px 28px;
        border-radius: 999px;
        cursor: pointer;
        transition: transform 150ms ease, box-shadow 150ms ease;
        box-shadow: 0 16px 40px rgba(79, 70, 229, 0.35);
      }
      button:disabled {
        opacity: 0.6;
        cursor: progress;
      }
      button:not(:disabled):hover {
        transform: translateY(-2px);
        box-shadow: 0 20px 45px rgba(124, 58, 237, 0.38);
      }
      .badge {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 6px 12px;
        border-radius: 999px;
        background: rgba(59, 130, 246, 0.15);
        color: #bfdbfe;
        font-size: 0.85rem;
      }
      .status-card {
        background: rgba(15, 23, 42, 0.7);
        border: 1px solid rgba(148, 163, 184, 0.22);
        border-radius: 20px;
        padding: 24px;
        display: grid;
        gap: 12px;
      }
      .results {
        display: grid;
        gap: 20px;
      }
      .result-item {
        background: rgba(15, 23, 42, 0.6);
        border-radius: 18px;
        padding: 20px;
        border: 1px solid rgba(79, 70, 229, 0.18);
      }
      .result-item h3 {
        margin: 0 0 12px;
        font-size: 1.05rem;
        color: #c7d2fe;
      }
      pre {
        background: rgba(30, 41, 59, 0.85);
        padding: 16px;
        border-radius: 14px;
        overflow-x: auto;
        border: 1px solid rgba(148, 163, 184, 0.16);
      }
      .error-card {
        border: 1px solid rgba(248, 113, 113, 0.5);
        background: rgba(127, 29, 29, 0.35);
      }
      .muted {
        color: #94a3b8;
      }
      .sandbox-warning {
        border: 1px solid rgba(248, 191, 89, 0.45);
        background: rgba(120, 53, 15, 0.35);
        color: #fef08a;
      }
      footer {
        text-align: center;
        color: #94a3b8;
        font-size: 0.85rem;
      }
      @media (max-width: 720px) {
        body {
          padding: 24px 12px;
        }
        header, form {
          padding: 24px;
        }
      }
    </style>
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
    <link
      href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap"
      rel="stylesheet"
    />
  </head>
  <body>
    <div class="container">
      <header>
        <div class="badge">Codex Web · v${context.version}</div>
        <h1>Ship ideas faster with a browser-native Codex</h1>
        <p>
          Launch full-auto Codex sessions from your browser with streaming command logs,
          rendered markdown, and smart summaries. Commands run in the sandbox whenever
          it's available so you can stay focused on your flow.
        </p>
      </header>
      <main>
        <form id="prompt-form">
          <label>
            <span>Prompt</span>
            <textarea
              id="prompt-input"
              name="prompt"
              placeholder="Describe the task for Codex..."
              required
            ></textarea>
          </label>
          <div class="status-card" id="environment-card"></div>
          <div class="actions">
            <button type="submit" id="submit-button">Run in Codex</button>
            <div class="muted" id="duration-indicator"></div>
          </div>
        </form>
        <section class="results" id="results"></section>
      </main>
      <footer>
        Built for repositories at <code>${context.cwd}</code>
      </footer>
    </div>
    <script>
      const SERVER_CONTEXT = ${serializedContext};
      const form = document.getElementById('prompt-form');
      const promptInput = document.getElementById('prompt-input');
      const submitButton = document.getElementById('submit-button');
      const resultsContainer = document.getElementById('results');
      const durationIndicator = document.getElementById('duration-indicator');
      const environmentCard = document.getElementById('environment-card');

      function renderEnvironment() {
        const rows = [
          '<strong>Model:</strong> <span>' + escapeHtml(SERVER_CONTEXT.model) + '</span>',
          '<strong>Provider:</strong> <span>' + escapeHtml(SERVER_CONTEXT.provider) + '</span>',
          '<strong>Response storage:</strong> <span>' +
            (SERVER_CONTEXT.disableResponseStorage ? "Disabled" : "Enabled") +
            '</span>',
        ];

        if (SERVER_CONTEXT.sandboxStatus === "available") {
          rows.push('<strong>Sandbox:</strong> <span>Linux Landlock active</span>');
        } else if (SERVER_CONTEXT.sandboxStatus === "not-applicable") {
          rows.push('<strong>Sandbox:</strong> <span>Not required on this platform</span>');
        } else {
          const warning = escapeHtml(
            SERVER_CONTEXT.sandboxMessage ||
              "Codex will run commands without sandboxing. Ensure you trust this environment.",
          );
          rows.push(
            '<div class="sandbox-warning"><strong>Sandbox unavailable:</strong><br>' +
              warning +
              '</div>',
          );
        }

        environmentCard.innerHTML = rows
          .map((row) => '<div>' + row + '</div>')
          .join('');
      }

      function escapeHtml(value) {
        return value
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;')
          .replace(/'/g, '&#39;');
      }

      function renderItems(items) {
        if (!Array.isArray(items) || items.length === 0) {
          resultsContainer.innerHTML = '<div class="muted">No output yet.</div>';
          return;
        }

        const fragments = items.map((item) => {
          if (!item || typeof item !== 'object') {
            return (
              '<article class="result-item"><pre>' +
              escapeHtml(JSON.stringify(item, null, 2)) +
              '</pre></article>'
            );
          }

          if (item.type === 'message') {
            const role = escapeHtml(item.role || 'assistant');
            let content = '';
            if (Array.isArray(item.content)) {
              content = item.content
                .map((piece) => {
                  if (piece.type === 'output_text' || piece.type === 'input_text') {
                    return '<p>' + escapeHtml(piece.text || '') + '</p>';
                  }
                  if (piece.type === 'refusal') {
                    return '<p class="error-card">' + escapeHtml(piece.refusal || '') + '</p>';
                  }
                  return '<pre>' + escapeHtml(JSON.stringify(piece, null, 2)) + '</pre>';
                })
                .join('');
            } else {
              content = '<pre>' + escapeHtml(JSON.stringify(item.content, null, 2)) + '</pre>';
            }

            return '<article class="result-item"><h3>' + role + '</h3>' + content + '</article>';
          }

          if (item.type === 'function_call') {
            const name = escapeHtml(item.name || 'function_call');
            const args =
              typeof item.arguments === 'string'
                ? item.arguments
                : JSON.stringify(item.arguments ?? {}, null, 2);
            return (
              '<article class="result-item"><h3>Command</h3><pre>' +
              escapeHtml(args) +
              '</pre><div class="muted">' +
              name +
              '</div></article>'
            );
          }

          if (item.type === 'function_call_output') {
            const output =
              typeof item.output === 'string'
                ? item.output
                : JSON.stringify(item.output ?? {}, null, 2);
            return (
              '<article class="result-item"><h3>Command output</h3><pre>' +
              escapeHtml(output) +
              '</pre></article>'
            );
          }

          return (
            '<article class="result-item"><h3>' +
            escapeHtml(item.type) +
            '</h3><pre>' +
            escapeHtml(JSON.stringify(item, null, 2)) +
            '</pre></article>'
          );
        });

        resultsContainer.innerHTML = fragments.join('');
      }

      async function handleSubmit(event) {
        event.preventDefault();
        const prompt = promptInput.value.trim();
        if (!prompt) {
          return;
        }

        submitButton.disabled = true;
        durationIndicator.textContent = 'Running...';
        resultsContainer.innerHTML = '';

        try {
          const response = await fetch('/api/run', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
            },
            body: JSON.stringify({ prompt }),
          });

          const payload = await response.json();

          if (!response.ok) {
            const message = payload?.error ? String(payload.error) : 'Unexpected error';
            resultsContainer.innerHTML =
              '<article class="result-item error-card"><h3>Request failed</h3><p>' +
              escapeHtml(message) +
              '</p></article>';
          } else {
            renderItems(payload.items || []);
            if (typeof payload.durationMs === 'number') {
              durationIndicator.textContent = 
                'Completed in ' + (payload.durationMs / 1000).toFixed(1) + 's';
            } else {
              durationIndicator.textContent = '';
            }
          }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          resultsContainer.innerHTML =
            '<article class="result-item error-card"><h3>Request failed</h3><p>' +
            escapeHtml(message) +
            '</p></article>';
          durationIndicator.textContent = '';
        } finally {
          submitButton.disabled = false;
        }
      }

      renderEnvironment();
      form.addEventListener('submit', handleSubmit);
    </script>
  </body>
</html>`;
}
