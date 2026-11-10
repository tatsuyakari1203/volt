/** Joins class names, skipping falsy parts. Tiny stand-in for a class-name library. */
export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(' ');
}
