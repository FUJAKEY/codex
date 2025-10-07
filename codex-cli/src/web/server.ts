import type { ApprovalPolicy } from "../approvals";
import type { AppConfig } from "../utils/config";
import type { Request, Response } from "express";
import type { ResponseItem } from "openai/resources/responses/responses";

import { AgentLoop } from "../utils/agent/agent-loop";
import { ReviewDecision } from "../utils/agent/review.js";
import { formatResponseItemForDisplay } from "../utils/format-response.js";
import { createInputItem } from "../utils/input-utils.js";
import chalk from "chalk";
import express from "express";
import { randomUUID } from "node:crypto";
import open from "open";

type SessionEvent = {
  type: string;
  data?: unknown;
};

type Session = {
  id: string;
  events: Array<SessionEvent>;
  listeners: Set<Response>;
  completed: boolean;
};

type WebServerOptions = {
  config: AppConfig;
  model: string;
  provider: string;
  approvalPolicy: ApprovalPolicy;
  additionalWritableRoots: ReadonlyArray<string>;
  port: number;
  host: string;
  autoOpen: boolean;
};

const INDEX_HTML = String.raw`
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>OpenAI Codex · Web Workspace</title>
    <style>
      * {
        box-sizing: border-box;
      }

      body {
        margin: 0;
        font-family: "Inter", "Segoe UI", sans-serif;
        background: radial-gradient(circle at 20% 20%, #20264d, #0d0f1f 60%);
        color: #f8f9ff;
        min-height: 100vh;
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 32px;
        padding: 48px 24px 64px;
      }

      h1 {
        margin: 0;
        font-weight: 700;
        font-size: clamp(2rem, 5vw, 3rem);
        letter-spacing: -0.03em;
        text-align: center;
      }

      p.subtitle {
        margin: 0;
        opacity: 0.75;
        font-size: 1rem;
        text-align: center;
      }

      .panel {
        width: min(960px, 100%);
        background: rgba(16, 18, 35, 0.85);
        border: 1px solid rgba(110, 124, 255, 0.2);
        border-radius: 18px;
        box-shadow: 0 40px 80px rgba(4, 6, 24, 0.6);
        padding: 28px;
        display: flex;
        flex-direction: column;
        gap: 24px;
      }

      form {
        display: flex;
        flex-direction: column;
        gap: 16px;
      }

      textarea {
        width: 100%;
        min-height: 120px;
        border-radius: 14px;
        border: 1px solid rgba(110, 124, 255, 0.35);
        padding: 16px 18px;
        font-size: 1rem;
        line-height: 1.6;
        background: rgba(12, 14, 28, 0.75);
        color: #f8f9ff;
        resize: vertical;
        transition: border-color 160ms ease, box-shadow 160ms ease;
      }

      textarea:focus {
        outline: none;
        border-color: rgba(160, 180, 255, 0.85);
        box-shadow: 0 0 0 3px rgba(94, 104, 255, 0.25);
      }

      button.primary {
        align-self: flex-start;
        border: none;
        border-radius: 14px;
        padding: 14px 28px;
        font-weight: 600;
        letter-spacing: 0.02em;
        background: linear-gradient(135deg, #7f5dff, #6050ff 60%, #4a5bff);
        color: #fff;
        cursor: pointer;
        transition: transform 160ms ease, box-shadow 160ms ease, opacity 160ms ease;
      }

      button.primary:hover {
        transform: translateY(-1px);
        box-shadow: 0 18px 40px rgba(74, 91, 255, 0.35);
      }

      button.primary:disabled {
        opacity: 0.6;
        cursor: not-allowed;
        transform: none;
        box-shadow: none;
      }

      .console {
        background: rgba(4, 6, 24, 0.8);
        border-radius: 14px;
        border: 1px solid rgba(110, 124, 255, 0.2);
        padding: 18px;
        font-family: "Fira Code", "SFMono-Regular", Consolas, monospace;
        font-size: 0.95rem;
        line-height: 1.55;
        max-height: 420px;
        overflow-y: auto;
        position: relative;
      }

      .console pre {
        margin: 0;
        white-space: pre-wrap;
        word-break: break-word;
      }

      .console.empty::before {
        content: "Awaiting instructions…";
        position: absolute;
        inset: 0;
        display: grid;
        place-items: center;
        color: rgba(200, 206, 255, 0.45);
        letter-spacing: 0.08em;
        text-transform: uppercase;
        font-size: 0.85rem;
      }

      .status-pill {
        display: inline-flex;
        align-items: center;
        gap: 8px;
        border-radius: 999px;
        background: rgba(110, 124, 255, 0.15);
        padding: 6px 14px;
        font-size: 0.85rem;
        letter-spacing: 0.06em;
        text-transform: uppercase;
        color: rgba(226, 229, 255, 0.85);
      }

      .status-pill.loading::before {
        content: "";
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 2px solid rgba(226, 229, 255, 0.6);
        border-top-color: transparent;
        animation: spin 900ms linear infinite;
      }

      @keyframes spin {
        to {
          transform: rotate(360deg);
        }
      }

      footer {
        margin-top: auto;
        opacity: 0.55;
        font-size: 0.85rem;
        text-align: center;
      }

      a {
        color: rgba(170, 186, 255, 0.95);
      }
    </style>
  </head>
  <body>
    <div class="panel">
      <div>
        <h1>Codex Web Workspace</h1>
        <p class="subtitle">Launch full-auto agent runs, review output, and iterate from your browser.</p>
      </div>
      <form id="prompt-form">
        <textarea
          id="prompt-input"
          name="prompt"
          placeholder="Describe the feature you want Codex to implement, or paste a bug report…"
          required
        ></textarea>
        <button type="submit" class="primary" id="submit-button">Run Codex</button>
      </form>
      <div class="status-pill" id="status-pill" hidden>Idle</div>
      <div class="console empty" id="console"><pre id="log"></pre></div>
    </div>
    <footer>Powered by <a href="https://github.com/openai/codex" target="_blank" rel="noreferrer">OpenAI Codex CLI</a></footer>
    <script type="module">
      const form = document.getElementById("prompt-form");
      const textarea = document.getElementById("prompt-input");
      const button = document.getElementById("submit-button");
      const log = document.getElementById("log");
      const consoleBox = document.getElementById("console");
      const statusPill = document.getElementById("status-pill");

      let eventSource = null;

      function setLoading(loading) {
        if (loading) {
          button.disabled = true;
          statusPill.hidden = false;
          statusPill.textContent = "Running";
          statusPill.classList.add("loading");
        } else {
          button.disabled = false;
          statusPill.classList.remove("loading");
          statusPill.textContent = "Idle";
        }
      }

      function appendLine(text) {
        if (consoleBox.classList.contains("empty")) {
          consoleBox.classList.remove("empty");
          log.textContent = "";
        }
        log.textContent += text + "\n";
        consoleBox.scrollTop = consoleBox.scrollHeight;
      }

      function resetConsole() {
        consoleBox.classList.add("empty");
        log.textContent = "";
      }

      form.addEventListener("submit", async (event) => {
        event.preventDefault();
        const prompt = textarea.value.trim();
        if (!prompt) {
          return;
        }

        if (eventSource) {
          eventSource.close();
        }

        resetConsole();
        setLoading(true);

        try {
          const response = await fetch("/api/run", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ prompt }),
          });
          if (!response.ok) {
            const payload = await response.json().catch(() => ({ message: "Unable to start Codex" }));
            throw new Error(payload.error || payload.message || "Unable to start Codex");
          }
          const { sessionId } = await response.json();
          eventSource = new EventSource("/api/run/" + sessionId + "/stream");

          eventSource.addEventListener("item", (evt) => {
            const data = JSON.parse(evt.data);
            appendLine(data.text);
          });

          eventSource.addEventListener("status", (evt) => {
            const data = JSON.parse(evt.data);
            setLoading(Boolean(data.loading));
          });

          eventSource.addEventListener("error", (evt) => {
            const data = JSON.parse(evt.data);
            appendLine("⚠️  " + data.message);
            setLoading(false);
          });

          eventSource.addEventListener("complete", () => {
            appendLine("✅  Codex completed the run.");
            setLoading(false);
            eventSource?.close();
          });

          eventSource.onerror = () => {
            setLoading(false);
          };
        } catch (err) {
          const message = err && err.message ? err.message : "Failed to contact Codex";
          appendLine("⚠️  " + message);
          setLoading(false);
        }
      });
    </script>
  </body>
</html>
`;

const sessions = new Map<string, Session>();

function writeEvent(res: Response, event: SessionEvent): void {
  res.write(`event: ${event.type}\n`);
  if (event.data !== undefined) {
    res.write(`data: ${JSON.stringify(event.data)}\n`);
  }
  res.write("\n");
}

function emit(session: Session, event: SessionEvent): void {
  session.events.push(event);
  for (const listener of session.listeners) {
    writeEvent(listener, event);
  }
}

function createSession(): Session {
  const session: Session = {
    id: randomUUID(),
    events: [],
    listeners: new Set<Response>(),
    completed: false,
  };
  sessions.set(session.id, session);
  return session;
}

function cleanupSessionIfIdle(session: Session): void {
  if (session.completed && session.listeners.size === 0) {
    sessions.delete(session.id);
  }
}

async function runAgentForSession(
  session: Session,
  prompt: string,
  options: WebServerOptions,
): Promise<void> {
  const agent = new AgentLoop({
    model: options.model,
    provider: options.provider,
    instructions: options.config.instructions,
    config: options.config,
    approvalPolicy: options.approvalPolicy,
    disableResponseStorage: options.config.disableResponseStorage,
    additionalWritableRoots: options.additionalWritableRoots,
    onItem: (item: ResponseItem) => {
      emit(session, {
        type: "item",
        data: { text: formatResponseItemForDisplay(item) },
      });
    },
    onLoading: (loading: boolean) => {
      emit(session, { type: "status", data: { loading } });
    },
    getCommandConfirmation: async () => ({
      review: ReviewDecision.YES,
    }),
    onLastResponseId: () => {
      /* web mode keeps track server-side */
    },
  });

  try {
    const inputItem = await createInputItem(prompt, []);
    await agent.run([inputItem]);
    emit(session, { type: "complete" });
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unexpected error";
    emit(session, { type: "error", data: { message } });
  } finally {
    agent.terminate();
    session.completed = true;
    cleanupSessionIfIdle(session);
  }
}

export async function runWebServer(options: WebServerOptions): Promise<void> {
  const app = express();
  app.use(express.json({ limit: "1mb" }));

  app.get("/", (_req: Request, res: Response) => {
    res.setHeader("Content-Type", "text/html; charset=utf-8");
    res.send(INDEX_HTML);
  });

  app.post("/api/run", async (req: Request, res: Response) => {
    const prompt = typeof req.body?.prompt === "string" ? req.body.prompt.trim() : "";
    if (!prompt) {
      res.status(400).json({ error: "Prompt is required" });
      return;
    }

    const session = createSession();
    res.json({ sessionId: session.id });

    void runAgentForSession(session, prompt, options);
  });

  app.get("/api/run/:id/stream", (req: Request, res: Response) => {
    const session = sessions.get(req.params.id);
    if (!session) {
      res.sendStatus(404);
      return;
    }

    res.status(200);
    res.setHeader("Content-Type", "text/event-stream");
    res.setHeader("Cache-Control", "no-cache, no-transform");
    res.setHeader("Connection", "keep-alive");
    res.flushHeaders?.();

    session.listeners.add(res);
    for (const event of session.events) {
      writeEvent(res, event);
    }

    const keepAlive = setInterval(() => {
      res.write(": keep-alive\n\n");
    }, 15000);

    req.on("close", () => {
      clearInterval(keepAlive);
      session.listeners.delete(res);
      cleanupSessionIfIdle(session);
    });
  });

  const server = app.listen(options.port, options.host);

  await new Promise<void>((resolve, reject) => {
    server.on("listening", async () => {
      const url = `http://${options.host}:${options.port}`;
      // eslint-disable-next-line no-console
      console.log(
        chalk.bold.cyan("›") +
          chalk.cyanBright(` Codex web interface available at ${url}`),
      );
      if (options.autoOpen) {
        try {
          await open(url, { newInstance: true });
        } catch (error) {
          // eslint-disable-next-line no-console
          console.error(chalk.yellow("Unable to open browser:"), error);
        }
      }
    });

    server.on("error", (error) => {
      reject(error);
    });

    const shutdown = () => {
      server.close(() => resolve());
    };

    process.once("SIGINT", shutdown);
    process.once("SIGTERM", shutdown);
  }).catch((error) => {
    throw error;
  });
}
