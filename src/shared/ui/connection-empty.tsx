import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyActions,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { useIcon, type IconName } from "@/lib/icon-context";

export function ConnectionEmpty({
  title,
  description,
  icon = "inbox",
  action,
  onAction,
  compact = false,
}: {
  title: string;
  description?: string;
  icon?: IconName;
  action?: string;
  onAction?: () => void;
  compact?: boolean;
}) {
  const Icon = useIcon(icon);
  return (
    <Empty reason="first-use" scope={compact ? "inline" : "section"}>
      <EmptyMedia variant="icon">
        <Icon size={28} />
      </EmptyMedia>
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        {description && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
      {action && onAction && (
        <EmptyActions>
          <Button variant="neutral" onClick={onAction}>
            {action}
          </Button>
        </EmptyActions>
      )}
    </Empty>
  );
}
