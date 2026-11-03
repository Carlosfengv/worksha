import type { Conversation } from "./types";

export interface ReadingPage {
  cursor?: string;
  anchor?: string;
  scroll: number;
  expanded: string[];
}
export interface ReadingState {
  epoch?: symbol;
  data: Conversation | null;
  expanded: string[];
  scroll: number;
  tab: string;
  originAnchor?: string;
  page?: ReadingPage;
  previous?: ReadingPage[];
}

// These are retained-data estimates, not a WebView heap limit. One active
// backend page may be larger; it remains readable but is not cached on leave.
export const READING_CACHE_BYTES = 8 * 1024 * 1024;
export const READING_CACHE_EVENTS = 400;
export const READING_TRAIL_PAGES = 100;
const SESSION_LIMIT = 10;

export function readingBytes(value: unknown): number {
  if (typeof value === "string") return 16 + value.length * 2;
  if (value === null || typeof value !== "object") return 8;
  let total = 32;
  for (const key of Object.keys(value)) {
    total +=
      16 +
      key.length * 2 +
      readingBytes((value as Record<string, unknown>)[key]);
  }
  return total;
}

/** Lightweight positions survive body eviction, but never an authority epoch. */
export class ReadingCache {
  private positions = new Map<string, Omit<ReadingState, "data">>();
  private bodies = new Map<string, { data: Conversation; bytes: number }>();
  private sizes = new WeakMap<Conversation, number>();

  get size() {
    return this.positions.size;
  }
  get(id: string): ReadingState | undefined {
    const position = this.positions.get(id);
    return position
      ? { ...position, data: this.bodies.get(id)?.data ?? null }
      : undefined;
  }
  set(id: string, view: ReadingState) {
    const { data, ...position } = view;
    this.positions.delete(id);
    this.positions.set(id, position);
    this.bodies.delete(id);
    if (data) {
      const bytes = this.sizes.get(data) ?? readingBytes(data);
      this.sizes.set(data, bytes);
      if (
        bytes <= READING_CACHE_BYTES &&
        data.items.length <= READING_CACHE_EVENTS
      )
        this.bodies.set(id, { data, bytes });
    }
    while (this.positions.size > SESSION_LIMIT)
      this.delete(this.positions.keys().next().value!);
    while (
      this.usage().bytes > READING_CACHE_BYTES ||
      this.usage().events > READING_CACHE_EVENTS
    )
      this.bodies.delete(this.bodies.keys().next().value!);
  }
  scroll(id: string, scroll: number) {
    const position = this.positions.get(id);
    if (position) position.scroll = scroll;
  }
  delete(id: string) {
    this.positions.delete(id);
    this.bodies.delete(id);
  }
  *values() {
    for (const id of this.positions.keys()) yield this.get(id)!;
  }
  *[Symbol.iterator](): IterableIterator<[string, ReadingState]> {
    for (const id of this.positions.keys()) yield [id, this.get(id)!];
  }
  usage() {
    let bytes = 0,
      events = 0;
    for (const body of this.bodies.values()) {
      bytes += body.bytes;
      events += body.data.items.length;
    }
    return { bytes, events, bodies: this.bodies.size };
  }
}
