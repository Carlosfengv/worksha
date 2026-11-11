import type { Accounts } from "@/features/accounts/types";
import { command } from "@/shared/lib/desktop-command";
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
