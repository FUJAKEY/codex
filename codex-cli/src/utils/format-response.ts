import type { ResponseItem } from "openai/resources/responses/responses";

import { parseToolCall } from "./parsers.js";
import { PRETTY_PRINT } from "../utils/config.js";

type ExecOutputMetadata = {
  exit_code?: number;
  duration_seconds?: number;
};

export function formatResponseItemForDisplay(item: ResponseItem): string {
  if (!PRETTY_PRINT) {
    return JSON.stringify(item);
  }

  switch (item.type) {
    case "message": {
      const role = item.role === "assistant" ? "assistant" : item.role;
      const txt = item.content
        .map((c) => {
          if (c.type === "output_text" || c.type === "input_text") {
            return c.text;
          }
          if (c.type === "input_image") {
            return "<Image>";
          }
          if (c.type === "input_file") {
            return c.filename;
          }
          if (c.type === "refusal") {
            return c.refusal;
          }
          return "?";
        })
        .join(" ");

      return `${role}: ${txt}`;
    }
    case "function_call": {
      const details = parseToolCall(item);
      return `$ ${details?.cmdReadableText ?? item.name}`;
    }
    case "function_call_output": {
      const meta = item.metadata as ExecOutputMetadata | undefined;
      const parts: Array<string> = [];
      if (typeof meta?.exit_code === "number") {
        parts.push(`code: ${meta.exit_code}`);
      }
      if (typeof meta?.duration_seconds === "number") {
        parts.push(`duration: ${meta.duration_seconds}s`);
      }
      const header = parts.length > 0 ? ` (${parts.join(", ")})` : "";
      return `command.stdout${header}\n${item.output}`;
    }
    default: {
      return JSON.stringify(item);
    }
  }
}
