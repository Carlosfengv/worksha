import { command } from "@/shared/lib/desktop-command";
import type {
  Account,
  AccountEdit,
  AccountInput,
  ChangePassword,
  IamStatus,
  Resource,
  ResourceDetail,
  Role,
  SetupPermit,
} from "./types";

type Action =
  | { kind: "prepare"; recovery: boolean }
  | ({ kind: "initialize"; operationId: string } & AccountInput)
  | {
      kind: "recover";
      operationId: string;
      recoveryCode: string;
      password: string;
    }
  | { kind: "login"; username: string; password: string }
  | { kind: "logout" | "accounts" | "resources" }
  | ({ kind: "create"; role: Role } & AccountInput)
  | { kind: "edit"; accountId: string; input: AccountEdit }
  | { kind: "delete"; accountId: string; revision: number }
  | ({ kind: "password" } & ChangePassword)
  | { kind: "grants"; accountId: string }
  | { kind: "grant"; accountId: string; serviceId: string; allow: boolean }
  | { kind: "resource"; serviceId: string };

const perform = <T>(input: Action) => command<T>("perform_iam", { input });
export const iamApi = {
  status: () => command<IamStatus>("get_iam_status"),
  prepare: (recovery: boolean) =>
    perform<SetupPermit>({ kind: "prepare", recovery }),
  initialize: (operationId: string, input: AccountInput) =>
    perform({ kind: "initialize", operationId, ...input }),
  recover: (operationId: string, recoveryCode: string, password: string) =>
    perform({ kind: "recover", operationId, recoveryCode, password }),
  login: (username: string, password: string) =>
    perform({ kind: "login", username, password }),
  logout: () => perform({ kind: "logout" }),
  accounts: () => perform<Account[]>({ kind: "accounts" }),
  create: (input: AccountInput, role: Role) =>
    perform<Account>({ kind: "create", ...input, role }),
  edit: (accountId: string, input: AccountEdit) =>
    perform<Account>({ kind: "edit", accountId, input }),
  remove: (accountId: string, revision: number) =>
    perform({ kind: "delete", accountId, revision }),
  changePassword: (input: ChangePassword) =>
    perform({ kind: "password", ...input }),
  grants: (accountId: string) =>
    perform<string[]>({ kind: "grants", accountId }),
  grant: (accountId: string, serviceId: string, allow: boolean) =>
    perform({ kind: "grant", accountId, serviceId, allow }),
  resources: () => perform<Resource[]>({ kind: "resources" }),
  resource: (serviceId: string) =>
    perform<ResourceDetail>({ kind: "resource", serviceId }),
};
