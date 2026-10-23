import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** One check from tests/script/commands.json (see tests/script/README.md). */
export type Check =
  | { type: 'exact'; text: string }
  | { type: 'exact_file'; path: string }
  | { type: 'contains'; text: string }
  | { type: 'regex'; pattern: string; flags?: string };

export interface ScriptCommand {
  id: string;
  input: string;
  checks: Check[];
}

export interface CommandScript {
  /** The file this script was read from; exact_file paths are relative to its directory. */
  source: string;
  prompt: { text: string; regex: string };
  /** Expected on the line before the first prompt; null when checking is disabled. */
  banner: string | null;
  enter: string;
  terminal: { cols: number; rows: number };
  bootTimeoutMs: number;
  commandTimeoutMs: number;
  commands: ScriptCommand[];
}

const here = dirname(fileURLToPath(import.meta.url));

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Loads the shared command script (tests/script/commands.json, or HART_SCRIPT).
 *
 * HART_PROMPT overrides the prompt for guest images that predate the spec, such as the
 * dev pilot image, which prints "# " and no banner; the banner check is then skipped.
 */
export function loadScript(): CommandScript {
  const source = resolve(process.env.HART_SCRIPT ?? join(here, '../../script/commands.json'));
  if (!existsSync(source)) throw new Error(`command script not found: ${source}`);
  const raw = JSON.parse(readFileSync(source, 'utf8'));
  if (raw.schema_version !== 1) throw new Error(`${source}: unsupported schema_version ${raw.schema_version}`);
  const override = process.env.HART_PROMPT;
  const prompt = override ? { text: override, regex: `(^|\\n)${escapeRegex(override)}$` } : raw.prompt;
  return {
    source,
    prompt,
    banner: override ? null : (raw.banner ?? null),
    enter: raw.enter,
    terminal: raw.terminal,
    bootTimeoutMs: Number(process.env.HART_BOOT_TIMEOUT_MS ?? raw.boot_timeout_seconds * 1000),
    commandTimeoutMs: raw.command_timeout_seconds * 1000,
    commands: raw.commands,
  };
}

/** Applies a check to a command's normalized output. Returns null on success, or why it failed. */
export function applyCheck(script: CommandScript, check: Check, output: string): string | null {
  switch (check.type) {
    case 'exact':
      return output === check.text ? null : `expected exactly ${JSON.stringify(check.text)}`;
    case 'exact_file': {
      let want = readFileSync(join(dirname(script.source), check.path), 'utf8');
      if (want.endsWith('\n')) want = want.slice(0, -1);
      return output === want ? null : `expected the contents of ${check.path}`;
    }
    case 'contains':
      return output.includes(check.text) ? null : `expected to contain ${JSON.stringify(check.text)}`;
    case 'regex': {
      const flags = check.flags?.includes('m') ? 'm' : '';
      return new RegExp(check.pattern, flags).test(output) ? null : `expected to match /${check.pattern}/${flags}`;
    }
    default:
      return `unknown check type ${JSON.stringify((check as { type: string }).type)}`;
  }
}
