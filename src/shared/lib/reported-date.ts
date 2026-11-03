// Wire dates need a real calendar day and an explicit zone for clock times.
// Date.parse alone normalizes impossible days and accepts locale-dependent input.
export function parseReportedDate(value: string): Date | null {
  const match =
    /^(\d{4}-\d{2}-\d{2})(?:[Tt](\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?([Zz]|[+-]\d{2}:\d{2}))?$/.exec(
      value,
    );
  if (!match) return null;
  const day = new Date(`${match[1]}T00:00:00Z`);
  if (
    !Number.isFinite(day.getTime()) ||
    day.toISOString().slice(0, 10) !== match[1]
  )
    return null;
  if (!match[2]) return day;
  if (+match[2] > 23 || +match[3] > 59 || +match[4] > 59) return null;
  if (
    match[5].length > 1 &&
    (+match[5].slice(1, 3) > 23 || +match[5].slice(4) > 59)
  )
    return null;
  const time = new Date(value);
  return Number.isFinite(time.getTime()) ? time : null;
}
