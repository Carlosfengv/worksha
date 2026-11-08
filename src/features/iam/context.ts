import { createContext, useContext } from "react";
import type { IamStatus } from "./types";

export const IamContext = createContext<{
  status: IamStatus | null;
  beginSetup?: () => void;
  logout?: () => Promise<void>;
  refresh?: () => Promise<void>;
}>({ status: null });
export const useIam = () => useContext(IamContext);
