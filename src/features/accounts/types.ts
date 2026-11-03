export interface AccountSnapshot {
  identity: string | null;
  plan: string | null;
  accountType: "subscription" | "api";
  windows: {
    id: string;
    label: string;
    usedPercent: number | null;
    remainingPercent: number | null;
    resetsAt: number | null;
    durationMins: number | null;
  }[];
  money: {
    label: string;
    amount: string | null;
    currency: string;
    kind: string;
    period: string | null;
    resetsAt: string | null;
  }[];
  resetCredits: {
    availableCount: number | null;
    detailsComplete: boolean;
    expirations: (number | null)[];
    accessMode: string;
    reason: string;
  };
  asOf: string;
  validForSeconds: number;
  source: string;
  notes: string[];
}

export interface BillingAccount {
  id: string;
  name: string;
  accountType: "subscription" | "api";
  scope: string;
  providerId: string | null;
  declaredFor: string[];
  sourceRevision: string;
  readable: boolean;
  officialUrl: string | null;
  state: string;
  error: string | null;
  snapshot: AccountSnapshot | null;
}

export interface Accounts {
  accounts: BillingAccount[];
  asOf: string;
}
