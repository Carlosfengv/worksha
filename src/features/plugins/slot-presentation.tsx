import { Component, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

const segments =
  typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : null;
export function truncateSlotTitle(title: string) {
  const characters = segments
    ? Array.from(segments.segment(title), (s) => s.segment)
    : Array.from(title);
  return characters.length > 16
    ? characters.slice(0, 15).join("") + "…"
    : title;
}
export class SlotBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <div role="alert" className="space-y-2">
        <p>此扩展显示失败，现有详情仍可使用。</p>
        <Button
          variant="tertiary"
          onClick={() => this.setState({ failed: false })}
        >
          重新显示扩展
        </Button>
      </div>
    ) : (
      this.props.children
    );
  }
}
