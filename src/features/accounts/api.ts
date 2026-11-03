import type { Accounts } from "@/features/accounts/types";
import { command } from "@/shared/lib/desktop-command";
export const accountsApi = {
  accounts(): Promise<Accounts> {
    return command("get_accounts");
  },
  refreshAccount(
    accountRef: string,
    expectedRevision: string,
  ): Promise<Accounts> {
    return command("refresh_account", { accountRef, expectedRevision });
  },
  createClaudeQuotaLauncher(): Promise<{
    launcherPath: string;
    settingsPath: string;
  }> {
    return command("create_claude_quota_launcher");
  },
  openAccountConsole(accountRef: string): Promise<void> {
    return command("open_account_console", { accountRef });
  },
};
