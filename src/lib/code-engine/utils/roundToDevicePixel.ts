export function roundToDevicePixel(value: number): number {
    const dpr = window.devicePixelRatio ?? 1;
    return Math.round(value * dpr) / dpr;
}
