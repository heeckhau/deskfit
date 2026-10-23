import streamDeck from "@elgato/streamdeck";
import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

/** The Rust helper, copied next to the bundled plugin.js by `npm run build:helper`. */
export const helperPath = fileURLToPath(new URL("./deskfit", import.meta.url));

const REQUEST_TIMEOUT_MS = 5_000;
const MAX_RESTART_DELAY_MS = 30_000;

/** Mirrors `protocol::Status` in the helper. */
export type TreadmillStatus = {
	state: "stopped" | "starting" | "running" | "paused" | { unknown: number };
	/** Tenths of km/h. While running this is the target the belt is heading for. */
	speed: number;
	elapsed_seconds: number;
	distance_hundredths_km: number;
	calories: number;
	steps: number;
	countdown: number | null;
};

/** Mirrors the helper's state event. */
export type HelperState = {
	connection: "connecting" | "connected" | "disconnected";
	error: string | null;
	/** Null when the belt state is unknown. */
	status: TreadmillStatus | null;
	/** Requested speed not yet confirmed by a status, in tenths of km/h. */
	target: number | null;
	max_speed: number;
};

export type Request =
	| { cmd: "toggle" | "start" | "pause" | "stop" }
	/** Change speed by `delta` whole positions. */
	| { cmd: "adjust"; delta: number }
	| { cmd: "set_speed"; speed: number }
	| { cmd: "configure"; max_speed: number };

type Pending = { resolve: () => void; reject: (e: Error) => void; timer: NodeJS.Timeout };

/**
 * Runs `deskfit serve` as a child process and talks JSON lines with it. The
 * helper exits by itself when this process ends and its stdin closes.
 */
export class Helper extends EventEmitter<{ state: [HelperState] }> {
	state: HelperState = { connection: "connecting", error: null, status: null, target: null, max_speed: 80 };

	private child?: ChildProcessWithoutNullStreams;
	private nextId = 1;
	private pending = new Map<number, Pending>();
	private restartDelay = 1_000;
	private configure?: Request;

	start(): void {
		// --disclaim: macOS must judge the helper's own Bluetooth permission,
		// not Stream Deck's (see docs/macos-bluetooth.md).
		const child = spawn(helperPath, ["--disclaim", "serve"], { stdio: ["pipe", "pipe", "pipe"] });
		this.child = child;

		createInterface({ input: child.stdout }).on("line", (line) => this.onLine(line));
		createInterface({ input: child.stderr }).on("line", (line) => streamDeck.logger.warn(`helper: ${line}`));

		child.on("spawn", () => {
			streamDeck.logger.info(`helper started (pid ${child.pid})`);
			if (this.configure) void this.request(this.configure).catch(() => undefined);
		});
		child.on("error", (e) => streamDeck.logger.error(`helper: ${e.message}`));
		child.on("exit", (code, signal) => {
			streamDeck.logger.error(`helper exited (code ${code}, signal ${signal}); restarting in ${this.restartDelay} ms`);
			this.child = undefined;
			for (const [id, p] of this.pending) {
				clearTimeout(p.timer);
				p.reject(new Error("helper exited"));
				this.pending.delete(id);
			}
			this.setState({ connection: "disconnected", error: "helper exited", status: null, target: null, max_speed: this.state.max_speed });
			setTimeout(() => this.start(), this.restartDelay);
			this.restartDelay = Math.min(this.restartDelay * 2, MAX_RESTART_DELAY_MS);
		});
	}

	/** Send a request; resolves when the helper accepts it. */
	request(request: Request): Promise<void> {
		if (request.cmd === "configure") this.configure = request;
		const child = this.child;
		if (!child) return Promise.reject(new Error("helper not running"));
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new Error(`no reply to ${request.cmd}`));
			}, REQUEST_TIMEOUT_MS);
			this.pending.set(id, { resolve, reject, timer });
			child.stdin.write(JSON.stringify({ id, ...request }) + "\n");
		});
	}

	private onLine(line: string): void {
		let event: { type: string; id?: number | null; ok?: boolean; error?: string | null };
		try {
			event = JSON.parse(line);
		} catch {
			streamDeck.logger.warn(`helper sent invalid JSON: ${line}`);
			return;
		}
		if (event.type === "state") {
			const { type: _, ...state } = event as unknown as HelperState & { type: string };
			if (state.connection === "connected") this.restartDelay = 1_000;
			this.setState(state);
		} else if (event.type === "reply") {
			if (event.id == null) {
				streamDeck.logger.warn(`helper: ${event.error}`);
				return;
			}
			const p = this.pending.get(event.id);
			if (!p) return;
			clearTimeout(p.timer);
			this.pending.delete(event.id);
			if (event.ok) p.resolve();
			else p.reject(new Error(event.error ?? "request failed"));
		}
	}

	private setState(state: HelperState): void {
		if (state.connection !== this.state.connection || state.error !== this.state.error) {
			streamDeck.logger.info(`treadmill ${state.connection}${state.error ? `: ${state.error}` : ""}`);
		}
		this.state = state;
		this.emit("state", state);
	}
}
