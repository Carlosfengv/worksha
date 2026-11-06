export function appendItems<T>(target: T[], items: readonly T[]): void {
    for (const item of items) {
        target.push(item);
    }
}
