import { NavItem, NavItemTrigger } from "@/components/ui/nav-item";
import {
  sessionAgentLabel,
  date,
  number,
  projectLabel,
  sessionStorage,
} from "@/features/sessions/display";
import type { NativeSession } from "@/features/sessions/types";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";

export function SessionList({
  items,
  selected,
  onSelect,
}: {
  items: NativeSession[];
  selected?: string;
  onSelect: (session: NativeSession) => void;
}) {
  const { agentNames } = useAgentDirectory();
  return (
    <div className="space-y-1 p-2">
      {items.map((session) => (
        <NavItem
          key={session.id}
          value={session.id}
          active={session.id === selected}
        >
          <NavItemTrigger
            render={
              <button type="button" aria-pressed={session.id === selected} />
            }
            className="min-w-0 whitespace-normal py-3 text-left"
            style={{ height: "auto", minHeight: "var(--control-height-md)" }}
            onClick={() => onSelect(session)}
          >
            <span className="block w-full min-w-0 space-y-2 py-1">
              <span
                className="line-clamp-2 break-words text-body font-medium"
                title={session.title}
              >
                {session.title}
              </span>
              <span className="flex flex-wrap gap-x-2 gap-y-1 text-label text-fg-muted">
                <span>{sessionAgentLabel(session, agentNames)}</span>
                <span>{projectLabel(session)}</span>
                {session.modelErrorCount > 0 && (
                  <span>异常 {session.modelErrorCount} 条</span>
                )}
              </span>
              <span className="flex flex-wrap gap-x-2 gap-y-1 text-label text-fg-subtle">
                <span>{date(session.updatedAt)}</span>
                <span>{number(session.usage.totalTokens)} Token</span>
                <span>{sessionStorage(session)}</span>
              </span>
            </span>
          </NavItemTrigger>
        </NavItem>
      ))}
    </div>
  );
}
