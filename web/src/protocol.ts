/** Messages between the page and the emulator worker, and the image manifest format. */

export interface ImageFile {
  role: 'firmware' | 'kernel' | 'dtb';
  /** Original asset name, for display. */
  name: string;
  /** URL relative to the manifest. */
  url: string;
  /** Decoded size in bytes. */
  size: number;
  /** Size of the file as served, in bytes. */
  encodedSize: number;
  encoding: 'gzip' | 'identity';
  sha256: string;
}

export interface Manifest {
  version: 1;
  /** Guest image identity, for example the release tag. */
  guest: string;
  ramMiB: number;
  bootargs?: string;
  files: ImageFile[];
}

export type ToWorker =
  | { type: 'boot'; manifestUrl: string }
  | { type: 'input'; data: Uint8Array }
  | { type: 'restart' };

export type MachineState = 'running' | 'idle' | 'poweroff' | 'reboot';

export type FromWorker =
  | { type: 'manifest'; manifest: Manifest; backend: string }
  | { type: 'progress'; loaded: number; total: number }
  | { type: 'loaded'; downloadMs: number; compileMs: number; encodedBytes: number; decodedBytes: number }
  | { type: 'started' }
  | { type: 'output'; data: Uint8Array }
  | {
      type: 'stats';
      instret: number;
      /** Instructions retired in this window. */
      insns: number;
      /** Milliseconds spent executing in this window. */
      busyMs: number;
      /** Length of this window in milliseconds. */
      wallMs: number;
      state: MachineState;
    }
  | { type: 'stopped'; reason: 'poweroff' | 'reboot' }
  | { type: 'error'; message: string };
