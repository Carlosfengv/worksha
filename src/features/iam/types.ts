export const IAM_PLUGIN_ID = "builtin.iam";

export type Role = "owner" | "member";
export interface Account {
  id: string;
  username: string;
  displayName: string;
  role: Role;
  enabled: boolean;
  revision: number;
}
export interface IamStatus {
  enabled: boolean;
  available: boolean;
  account: Account | null;
}
export interface SetupPermit {
  operationId: string;
  recoveryCode: string;
}
export interface Resource {
  id: string;
  name: string;
}
export interface ResourceDetail extends Resource {
  models: string[];
  creator: string;
  tenantId: string;
}
export interface AccountInput {
  username: string;
  displayName: string;
  password: string;
}
export type AccountEdit = Pick<
  Account,
  "displayName" | "role" | "enabled" | "revision"
>;

export interface ChangePassword {
  accountId: string;
  revision: number;
  password: string;
  currentPassword?: string;
}
