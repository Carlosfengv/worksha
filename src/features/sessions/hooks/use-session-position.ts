import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { NativeSession } from "../types";
import type { useNativePage } from "./use-native-page";

type Pages = ReturnType<typeof useNativePage<NativeSession>>;

/** One viewport owner for the desktop list and its mobile drawer. */
export function useSessionPosition({
  scope,
  epoch,
  pages,
  drawer,
  selected,
  summaryId,
}: {
  scope: string;
  epoch: symbol;
  pages: Pages;
  drawer: boolean;
  selected?: string;
  summaryId?: string;
}) {
  const desktopList = useRef<HTMLDivElement>(null);
  const mobileList = useRef<HTMLDivElement>(null);
  const [expandedProjects, setExpandedProjects] = useState<string[]>([]);
  const positions = useRef(
    new Map<string | undefined, { scroll: number; expanded: string[] }>(),
  );
  const owner = useRef({ scope, epoch });
  const focusPage = useRef(false);
  const hasPage = pages.data !== null && !pages.scopeInvalid;

  // Restore before paint, then follow only this page's changing group heights.
  useLayoutEffect(() => {
    if (owner.current.scope !== scope || owner.current.epoch !== epoch) {
      positions.current.clear();
      owner.current = { scope, epoch };
      focusPage.current = false;
    }
    if (pages.scopeInvalid) {
      positions.current.clear();
      setExpandedProjects([]);
      focusPage.current = false;
    }
    if (!hasPage) return;
    const allowed = new Set([...pages.previousCursors, pages.cursor]);
    for (const cursor of positions.current.keys())
      if (!allowed.has(cursor)) positions.current.delete(cursor);
    const saved = positions.current.get(pages.cursor) ?? {
      scroll: 0,
      expanded: [],
    };
    setExpandedProjects(saved.expanded);
    const node = (drawer ? mobileList : desktopList).current;
    if (!node) return;
    let frame = 0;
    const restore = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        node.scrollTop = saved.scroll;
      });
    };
    const observer = new ResizeObserver(restore);
    for (const child of node.children) observer.observe(child);
    restore();
    if (focusPage.current) {
      node.focus({ preventScroll: true });
      focusPage.current = false;
    }
    const stop = () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
    const events = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const event of events)
      node.addEventListener(event, stop, { passive: true, once: true });
    return () => {
      stop();
      for (const event of events) node.removeEventListener(event, stop);
    };
  }, [
    scope,
    epoch,
    hasPage,
    pages.cursor,
    pages.previousCursors,
    pages.scopeInvalid,
    drawer,
  ]);
  useEffect(() => {
    if (pages.error) focusPage.current = false;
  }, [pages.error]);
  useEffect(() => {
    const node = (drawer ? mobileList : desktopList).current;
    if (!node || node.clientHeight === 0) return;
    const button = node.querySelector<HTMLButtonElement>(
      'button[aria-pressed="true"]',
    );
    if (!button) return;
    const target = button.getBoundingClientRect(),
      area = node.getBoundingClientRect();
    if (target.top < area.top) node.scrollTop += target.top - area.top;
    else if (target.bottom > area.bottom)
      node.scrollTop += target.bottom - area.bottom;
  }, [selected, summaryId, drawer]);

  return {
    desktopList,
    mobileList,
    expandedProjects,
    rememberScroll(scroll: number) {
      if (hasPage)
        positions.current.set(pages.cursor, {
          scroll,
          expanded:
            positions.current.get(pages.cursor)?.expanded ?? expandedProjects,
        });
    },
    expandProjects(expanded: string[]) {
      setExpandedProjects(expanded);
      positions.current.set(pages.cursor, {
        scroll: (drawer ? mobileList : desktopList).current?.scrollTop ?? 0,
        expanded,
      });
    },
    turn(direction: "next" | "previous" | "first") {
      if (pages.loading) return;
      if (hasPage)
        positions.current.set(pages.cursor, {
          scroll: (drawer ? mobileList : desktopList).current?.scrollTop ?? 0,
          expanded: expandedProjects,
        });
      focusPage.current = true;
      void (direction === "first"
        ? pages.reload()
        : direction === "previous"
          ? pages.loadPrevious()
          : pages.loadMore());
    },
  };
}
