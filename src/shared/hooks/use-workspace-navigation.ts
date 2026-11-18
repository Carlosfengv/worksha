import { createContext, useContext } from "react";

export type WorkspaceNavigation = {
  open: (href: string, options?: { pin?: boolean; title?: string }) => void;
  close: (goHome?: boolean) => void;
  dismiss: () => void;
  setDirty: (dirty: boolean) => void;
  setTitle: (title: string) => void;
};

const fallbackNavigation: WorkspaceNavigation = {
  open: (href) => {
    window.location.hash = href;
  },
  close: () => undefined,
  dismiss: () => undefined,
  setDirty: () => undefined,
  setTitle: () => undefined,
};

export const WorkspaceNavigationContext = createContext(fallbackNavigation);
export const useWorkspaceNavigation = () =>
  useContext(WorkspaceNavigationContext);
