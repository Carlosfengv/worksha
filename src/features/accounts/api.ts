import type {
  Accounts,
  ClaudeStatusLinePreview,
  ClaudeStatusLineStatus,
} from "@/features/accounts/types";
import { command } from "@/shared/lib/desktop-command";

type ClaudeStatusLineAction =
  { kind: "status" } | { kind: "apply" | "cancel"; reviewId: string };
type ClaudeStatusLineReview = {
  kind: "preview";
  enabled: boolean;
  expectedRevision: string;
};

function claudeStatusLine(
  input: ClaudeStatusLineAction,
): Promise<ClaudeStatusLineStatus>;
function claudeStatusLine(
  input: ClaudeStatusLineReview,
): Promise<ClaudeStatusLinePreview>;
function claudeStatusLine(
  input: ClaudeStatusLineAction | ClaudeStatusLineReview,
): Promise<ClaudeStatusLineStatus | ClaudeStatusLinePreview> {
  return command("manage_claude_quota_statusline", { input });
}

export const accountsApi = {
  accounts(): Promise<Accounts> {
    return command("get_accounts");
  },
  async refreshAccount(
    accountRef: string,
    expectedRevision: string,
  ): Promise<Accounts> {
    const reply = await command<Accounts>("refresh_account", {
      accountRef,
      expectedRevision,
    });
    window.dispatchEvent(new Event("account-quota-changed"));
    return reply;
  },
  setCodexQuotaConsent(
    expectedRevision: string,
    enabled: boolean,
  ): Promise<{ authorized: boolean }> {
    return command("set_codex_quota_consent", { expectedRevision, enabled });
  },
  createClaudeQuotaLauncher(): Promise<{
    launcherPath: string;
    settingsPath: string;
  }> {
    return command("create_claude_quota_launcher");
  },
  claudeStatusLine,
  openAccountConsole(accountRef: string): Promise<void> {
    return command("open_account_console", { accountRef });
  },
};
