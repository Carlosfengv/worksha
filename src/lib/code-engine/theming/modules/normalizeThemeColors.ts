import { hoverWouldEraseText, isFullyTransparent } from './color';
import type { ThemeLike } from './types';
const cache = new WeakMap<ThemeLike, ThemeLike>();
export function normalizeThemeColors(theme: ThemeLike): ThemeLike {
    const cached = cache.get(theme);
    if (cached != null)
        return cached;
    const originalColors = theme.colors ?? {};
    const colors: Record<string, string> = { ...originalColors };
    // Surface precedence: the editor falls back to the base theme bg/fg, and the
    // sidebar falls back to the editor. Plain `??` so an explicit value is honored
    // and the chain never invents a color.
    const editorBackground = originalColors['editor.background'] ?? theme.bg;
    const editorForeground = originalColors['editor.foreground'] ?? theme.fg;
    const sidebarBackground = originalColors['sideBar.background'] ?? editorBackground;
    const sidebarForeground = originalColors['sideBar.foreground'] ?? editorForeground;
    // Write each resolved surface back onto its canonical key so consumers read a
    // single key instead of re-walking the chain.
    fill(colors, 'editor.background', editorBackground);
    fill(colors, 'editor.foreground', editorForeground);
    fill(colors, 'sideBar.background', sidebarBackground);
    fill(colors, 'sideBar.foreground', sidebarForeground);
    fill(colors, 'input.background', originalColors['input.background'] ?? sidebarBackground);
    fill(colors, 'sideBarSectionHeader.foreground', originalColors['sideBarSectionHeader.foreground'] ?? sidebarForeground);
    fill(colors, 'list.activeSelectionForeground', originalColors['list.activeSelectionForeground'] ?? sidebarForeground);
    // Git status foreground chains: the dedicated gitDecoration key, then the
    // terminal ANSI color, then the editor gutter background (which catches
    // gutter-only themes like vesper). firstColor skips empty strings so a blank
    // key falls through to the next tier.
    fill(colors, 'gitDecoration.addedResourceForeground', firstColor(originalColors['gitDecoration.addedResourceForeground'], originalColors['terminal.ansiGreen'], originalColors['editorGutter.addedBackground']));
    fill(colors, 'gitDecoration.modifiedResourceForeground', firstColor(originalColors['gitDecoration.modifiedResourceForeground'], originalColors['terminal.ansiBlue'], originalColors['editorGutter.modifiedBackground']));
    fill(colors, 'gitDecoration.deletedResourceForeground', firstColor(originalColors['gitDecoration.deletedResourceForeground'], originalColors['terminal.ansiRed'], originalColors['editorGutter.deletedBackground']));
    // Focus ring: first non-transparent of [list.focusOutline, focusBorder]. A
    // transparent outline is rejected so the resolved key is always a visible
    // color; if neither candidate is visible the key is left absent.
    const focusRing = (isFullyTransparent(originalColors['list.focusOutline'])
        ? undefined
        : originalColors['list.focusOutline']) ??
        (isFullyTransparent(originalColors['focusBorder'])
            ? undefined
            : originalColors['focusBorder']);
    if (focusRing != null) {
        colors['list.focusOutline'] = focusRing;
    }
    else {
        delete colors['list.focusOutline'];
    }
    // Hover repair: a hover background that exactly matches the surface, or that
    // sits closer to the text color than the surface (so it would erase the row
    // text), is unusable for any consumer — drop it and let the consumer apply its
    // own hover default.
    const hover = originalColors['list.hoverBackground'];
    if (hover != null &&
        (matchesSurface(hover, sidebarBackground) ||
            hoverWouldEraseText(hover, sidebarBackground, sidebarForeground))) {
        delete colors['list.hoverBackground'];
    }
    const result = Object.freeze({ ...theme, colors: Object.freeze(colors) });
    cache.set(theme, result);
    return result;
}
// Writes `value` to `key` only when it is a real color, so an absent source key
// stays absent rather than being coerced to undefined/''.
function fill(colors: Record<string, string>, key: string, value: string | undefined): void {
    if (value != null && value !== '')
        colors[key] = value;
}
// Returns the first non-empty color among the candidates, in priority order.
// Skipping '' lets a blank workbench key fall through to the next tier.
function firstColor(...candidates: (string | undefined)[]): string | undefined {
    for (const candidate of candidates) {
        if (candidate != null && candidate !== '')
            return candidate;
    }
    return undefined;
}
// Case-insensitive exact-string surface match (NOT a luminance-based compare).
// Matches the equality the hover repair has always used.
function matchesSurface(color: string, surface: string | undefined): boolean {
    return surface != null && color.toLowerCase() === surface.toLowerCase();
}
