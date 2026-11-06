export function applyLineTextWithNewline(line: string, lineText: string): string {
    if (line.endsWith('\r\n')) {
        return lineText + '\r\n';
    }
    if (line.endsWith('\r')) {
        return lineText + '\r';
    }
    if (line.endsWith('\n')) {
        return lineText + '\n';
    }
    return lineText;
}
