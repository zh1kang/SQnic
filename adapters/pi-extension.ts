// SQNIC_ADAPTER_V1
// This project-local extension uses Pi's documented lifecycle events.
import { spawnSync } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const SQNIC_BIN = __SQNIC_BIN__;
const SQNIC_DB = __SQNIC_DB__;
const SQNIC_REPO = __SQNIC_REPO__;

function send(
  event: Record<string, unknown>,
  ctx: { ui: { notify(message: string, type: "warning"): void }; cwd: string; sessionManager: { getSessionId(): string; getSessionFile(): string | undefined } },
): Record<string, unknown> {
  const result = spawnSync(
    SQNIC_BIN,
    ["hook", "--repo", SQNIC_REPO, "--harness", "pi", "--db", SQNIC_DB],
    { input: JSON.stringify({ ...event, cwd: ctx.cwd, session_id: ctx.sessionManager.getSessionId(), transcript_path: ctx.sessionManager.getSessionFile() }), encoding: "utf8", maxBuffer: 1024 * 1024, timeout: 3000 },
  );
  try {
    if (result.error) throw result.error;
    if (result.status !== 0 || !result.stdout) throw new Error("sqnic hook returned no output");
    const value: unknown = JSON.parse(result.stdout);
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid hook response");
    const output = value as Record<string, unknown>;
    if (output.systemMessage) ctx.ui.notify(String(output.systemMessage), "warning");
    return output;
  } catch (error) {
    ctx.ui.notify(`SQnic context unavailable: ${String(error)}. Run sqnic auto-status.`, "warning");
    return {};
  }
}

export default function (pi: ExtensionAPI) {
  let pending: Record<string, unknown> | undefined;
  pi.on("session_start", async (event, ctx) => {
    pending = send({ ...event, hook_event_name: "SessionStart" }, ctx);
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const observed = send({ ...event, hook_event_name: "UserPromptSubmit" }, ctx);
    const result = observed.hookSpecificOutput ? observed : pending;
    pending = undefined;
    const output = result?.hookSpecificOutput;
    const context =
      output && typeof output === "object" && "additionalContext" in output
        ? (output as { additionalContext?: unknown }).additionalContext
        : undefined;
    if (typeof context !== "string" || context.length === 0) return undefined;
    return {
      message: {
        customType: "sqnic-context",
        content: context,
        display: false,
      },
    };
  });

  pi.on("tool_execution_end", async (event, ctx) => {
    send({ ...event, hook_event_name: "PostToolUse" }, ctx);
  });

  pi.on("session_shutdown", async (event, ctx) => {
    send({ ...event, hook_event_name: "SessionEnd" }, ctx);
  });
}
