export function guardWebKitScrollDuringRebuild(pre: HTMLElement | undefined, rebuild: () => void): void {
    const height = pre?.offsetHeight ?? 0;
    if (pre == null || height === 0) {
        rebuild();
        return;
    }
    pre.style.minHeight = `${height}px`;
    try {
        rebuild();
        void pre.offsetHeight;
    }
    finally {
        pre.style.minHeight = '';
    }
}
