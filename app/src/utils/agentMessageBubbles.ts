import debugFactory from 'debug';

const debug = debugFactory('agent-message-bubbles');

/**
 * Split an agent message into render-time bubble segments.
 *
 * Normalize excessive vertical whitespace first, then split only on double
 * newlines. Fenced code blocks stay intact as a single segment so
 * Markdown/code rendering does not fragment unexpectedly.
 * Markdown tables also stay grouped so they can render as dedicated table UI.
 *
 * Finally, regroup so a section heading and the body that follows it always
 * share one bubble (issue #3807): structured output such as the morning
 * briefing writes `## Heading\n\n- body…`, and splitting on the blank line
 * would otherwise orphan every heading into its own content-less bubble.
 */
export function splitAgentMessageIntoBubbles(content: string): string[] {
  const normalized = content
    .replace(/\r\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .replace(/\n?\s*<hr\s*\/?>\s*\n?/gi, '\n\n');
  const trimmedContent = normalized.trim();
  if (trimmedContent.length === 0) return [];
  if (!normalized.includes('\n')) {
    return isVisualSeparatorOnly(trimmedContent) ? [] : [trimmedContent];
  }

  const lines = normalized.split('\n');
  const segments: string[] = [];
  let currentLines: string[] = [];
  let inFence = false;

  const flushCurrent = () => {
    const segment = currentLines.join('\n').trim();
    if (segment.length > 0) {
      segments.push(segment);
    }
    currentLines = [];
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const trimmedLine = line.trim();

    if (!inFence && isMarkdownTableStart(lines, index)) {
      if (currentLines.length > 0) {
        flushCurrent();
      }
      const tableLines = [line, lines[index + 1]];
      index += 2;
      while (index < lines.length && looksLikeMarkdownTableRow(lines[index])) {
        tableLines.push(lines[index]);
        index += 1;
      }
      index -= 1;
      segments.push(tableLines.join('\n').trim());
      continue;
    }

    if (trimmedLine.startsWith('```')) {
      if (!inFence && currentLines.length > 0) {
        flushCurrent();
      }
      currentLines.push(line);
      inFence = !inFence;
      if (!inFence) {
        flushCurrent();
      }
      continue;
    }

    if (inFence) {
      currentLines.push(line);
      continue;
    }

    if (trimmedLine.length === 0) {
      if (currentLines.length > 0) {
        flushCurrent();
      }
      continue;
    }

    currentLines.push(line);
  }

  flushCurrent();
  const cleaned = segments.filter(segment => !isVisualSeparatorOnly(segment));
  const grouped = groupSectionsUnderHeadings(cleaned);
  debug(
    '[bubbles] %d char message -> %d raw segment(s) -> %d bubble(s) after heading grouping',
    content.length,
    cleaned.length,
    grouped.length
  );
  return grouped;
}

/**
 * Merge each section heading with the body segments that follow it (up to the
 * next heading) into a single bubble. A heading is never left in a bubble of
 * its own, and the body below it is never orphaned from its heading. Segments
 * before the first heading (e.g. a greeting) are left untouched.
 */
function groupSectionsUnderHeadings(segments: string[]): string[] {
  const grouped: string[] = [];
  let index = 0;

  while (index < segments.length) {
    if (!startsWithHeading(segments[index])) {
      grouped.push(segments[index]);
      index += 1;
      continue;
    }

    const section = [segments[index]];
    index += 1;
    while (index < segments.length && !startsWithHeading(segments[index])) {
      section.push(segments[index]);
      index += 1;
    }
    grouped.push(section.join('\n\n'));
  }

  return grouped;
}

/** A segment "starts a section" when its first non-empty line is a heading. */
function startsWithHeading(segment: string): boolean {
  const firstLine = segment.split('\n').find(line => line.trim().length > 0) ?? '';
  return isHeadingLine(firstLine);
}

/**
 * Recognize the heading shapes structured agent output uses to label a
 * section: Markdown ATX headings (`#`..`######`) and a single fully-bold line
 * (`**Calendar**`), optionally with a trailing colon.
 */
function isHeadingLine(line: string): boolean {
  const trimmed = line.trim();
  if (/^#{1,6}\s+\S/.test(trimmed)) return true;
  return /^\*\*[^*]+\*\*:?$/.test(trimmed);
}

export interface ParsedMarkdownTable {
  headers: string[];
  rows: string[][];
}

export function parseMarkdownTable(content: string): ParsedMarkdownTable | null {
  const normalized = content.replace(/\r\n/g, '\n').trim();
  const lines = normalized
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0);

  if (lines.length < 2) return null;
  if (!isMarkdownTableStart(lines, 0)) return null;

  const headers = splitMarkdownTableCells(lines[0]);
  const rows = lines.slice(2).map(splitMarkdownTableCells);

  if (headers.length === 0 || rows.some(row => row.length !== headers.length)) {
    return null;
  }

  return { headers, rows };
}

function isMarkdownTableStart(lines: string[], index: number): boolean {
  const header = lines[index];
  const separator = lines[index + 1];
  if (!header || !separator) return false;
  return looksLikeMarkdownTableRow(header) && looksLikeMarkdownTableSeparator(separator);
}

function looksLikeMarkdownTableRow(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed.includes('|')) return false;
  const cells = splitMarkdownTableCells(trimmed);
  return cells.length >= 2;
}

function looksLikeMarkdownTableSeparator(line: string): boolean {
  const cells = splitMarkdownTableCells(line);
  if (cells.length < 2) return false;
  return cells.every(cell => /^:?-{3,}:?$/.test(cell));
}

function splitMarkdownTableCells(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map(cell => cell.trim());
}

function isVisualSeparatorOnly(segment: string): boolean {
  const trimmed = segment.trim();
  if (trimmed.length === 0) return true;
  if (/^<hr\s*\/?>$/i.test(trimmed)) return true;
  return /^(?:-{3,}|\*{3,}|_{3,})$/.test(trimmed.replace(/\s+/g, ''));
}
