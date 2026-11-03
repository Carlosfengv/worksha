import { useCallback } from "react";

/** Preserve the active slot control when its bounded reader changes layout. */
export function useSlotFocusVisibility() {
  return useCallback((root: HTMLElement | null) => {
    const view = root?.ownerDocument.defaultView;
    if (!root || !view) return;
    const reveal = () => {
      const active = root.ownerDocument.activeElement;
      if (active instanceof HTMLElement && root.contains(active))
        active.scrollIntoView({ block: "nearest", inline: "nearest" });
    };
    let frame: number | null = null;
    const afterLayout = () => {
      if (frame !== null) view.cancelAnimationFrame(frame);
      frame = view.requestAnimationFrame(() => {
        frame = null;
        reveal();
      });
    };
    let height = root.getBoundingClientRect().height;
    const observer = new ResizeObserver(([entry]) => {
      const next = entry.contentRect.height;
      if (next > height) afterLayout();
      height = next;
    });
    observer.observe(root);
    // Window height can shrink without changing the slot's own dimensions.
    view.addEventListener("resize", afterLayout);
    root.addEventListener("focusin", reveal);
    return () => {
      observer.disconnect();
      view.removeEventListener("resize", afterLayout);
      root.removeEventListener("focusin", reveal);
      if (frame !== null) view.cancelAnimationFrame(frame);
    };
  }, []);
}
