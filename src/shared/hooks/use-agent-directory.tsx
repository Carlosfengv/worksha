import { createContext, useContext, useMemo, type ReactNode } from "react";
import {
  agentProducts,
  agentFamily,
  configAgent,
  configAgentName,
  createAgentNames,
  createFamilyNames,
  isSharedSource,
  sourceName,
  type AgentProduct,
} from "@/shared/types/agent";

function createDirectory(products: readonly AgentProduct[]) {
  return {
    agentProducts: products,
    agentNames: createAgentNames(products),
    familyNames: createFamilyNames(products),
    agentFamily: (id: string) => agentFamily(id, products),
    configAgent: (id: string) => configAgent(id, products),
    configAgentName: (id: string) => configAgentName(id, products),
    isSharedSource: (family: string) => isSharedSource(family, products),
    sourceName: (family: string) => sourceName(family, products),
  };
}
const Directory = createContext(createDirectory(agentProducts));

export function AgentDirectoryProvider({
  products,
  children,
}: {
  products?: readonly AgentProduct[];
  children: ReactNode;
}) {
  const directory = useMemo(
    () => createDirectory(products ?? agentProducts),
    [products],
  );
  return <Directory.Provider value={directory}>{children}</Directory.Provider>;
}

export function useAgentDirectory() {
  return useContext(Directory);
}
