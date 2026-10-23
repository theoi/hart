import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export interface Expectation {
  /** Every string must appear in the output. */
  contains?: string[];
  /** The output must equal this string exactly (LF line endings). */
  equals?: string;
  /** The output must equal this file's contents exactly; path relative to the script file. */
  equalsFile?: string;
}

export interface ScriptCommand {
  input: string;
  expect: Expectation;
}

export interface CommandScript {
  /** The file this script was read from. */
  source: string;
  prompt: string;
  commands: ScriptCommand[];
}

const here = dirname(fileURLToPath(import.meta.url));

/**
 * Loads the shared command script used by both the native and browser boot tests,
 * falling back to the local stub script until the shared one exists. HART_SCRIPT
 * overrides the path.
 */
export function loadScript(): CommandScript {
  const candidates = [
    process.env.HART_SCRIPT,
    resolve(here, '../../script/commands.json'),
    resolve(here, '../script.fallback.json'),
  ].filter((p): p is string => !!p);
  const source = candidates.find((p) => existsSync(p));
  if (!source) throw new Error(`no command script found in ${candidates.join(', ')}`);
  const raw = JSON.parse(readFileSync(source, 'utf8')) as Omit<CommandScript, 'source'>;
  return { source, prompt: raw.prompt ?? '# ', commands: raw.commands };
}

export function expectedText(script: CommandScript, expect: Expectation): string | undefined {
  if (expect.equals !== undefined) return expect.equals;
  if (expect.equalsFile !== undefined) return readFileSync(join(dirname(script.source), expect.equalsFile), 'utf8');
  return undefined;
}
