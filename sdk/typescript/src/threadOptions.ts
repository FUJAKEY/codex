export type ApprovalMode = "never" | "on-request" | "on-failure" | "untrusted";

export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

export type TheadOptions = {
  model?: string;
  sandboxMode?: SandboxMode;
  workingDirectory?: string;
  skipGitRepoCheck?: boolean;
};
