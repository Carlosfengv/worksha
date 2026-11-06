export function getHunkSideStartBoundary(start: number, count: number): number {
    return start - (count === 0 ? 0 : 1);
}
export function getHunkSideEndBoundary(start: number, count: number): number {
    return getHunkSideStartBoundary(start, count) + count;
}
