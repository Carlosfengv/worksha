import { useCallback, useState } from "react";
import type { OnChangeFn, PaginationState } from "@tanstack/react-table";

// Clamp a local table page when refreshed/filtered rows shrink, without resetting
// the reader to page one on every background snapshot.
export function useTablePagination(rowCount: number, initialPageSize: number) {
  const [saved, setSaved] = useState<PaginationState>({
    pageIndex: 0,
    pageSize: initialPageSize,
  });
  const bounded = useCallback(
    (value: PaginationState): PaginationState => ({
      ...value,
      pageIndex: Math.min(
        value.pageIndex,
        Math.max(0, Math.ceil(rowCount / value.pageSize) - 1),
      ),
    }),
    [rowCount],
  );
  const onChange: OnChangeFn<PaginationState> = useCallback(
    (updater) => {
      setSaved((previous) =>
        typeof updater === "function" ? updater(bounded(previous)) : updater,
      );
    },
    [bounded],
  );
  return { value: bounded(saved), onChange };
}
