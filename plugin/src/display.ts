import type { HelperState } from "./helper";

export type Display = {
	/** Short state, e.g. "Running" or "Slowing down". */
	label: string;
	/** Speed line, e.g. "3 km/h" or "3 → 4". */
	speed: string;
	/** Speed for the bar, in tenths of km/h. */
	bar: number;
	color: string;
};

const GREEN = "#4ade80";
const AMBER = "#fbbf24";
const GRAY = "#9ca3af";

/** Whole positions as "3"; other values (e.g. while slowing down) as "2.1". */
const kmh = (tenths: number) => (tenths % 10 === 0 ? String(tenths / 10) : (tenths / 10).toFixed(1));

/**
 * Describe the helper state for the touch strip and keys. `resumeSpeed` is
 * the last running speed: after a pause the treadmill reports 0, but start
 * resumes at the previous speed.
 */
export function describe(state: HelperState, resumeSpeed: number | null): Display {
	if (state.connection !== "connected") {
		const label = state.connection === "connecting" ? "Connecting…" : "Disconnected";
		return { label, speed: "--", bar: 0, color: GRAY };
	}
	const status = state.status;
	if (!status) return { label: "Unknown", speed: "--", bar: 0, color: GRAY };

	const s = status.state;
	if (s === "running") {
		const target = state.target;
		const speed = target != null && target !== status.speed ? `${kmh(status.speed)} → ${kmh(target)}` : `${kmh(status.speed)} km/h`;
		return { label: "Running", speed, bar: target ?? status.speed, color: GREEN };
	}
	if (s === "paused" && status.speed > 0) {
		return { label: "Slowing down", speed: `${kmh(status.speed)} km/h`, bar: status.speed, color: AMBER };
	}
	if (s === "paused" || s === "stopped") {
		const label = s === "paused" ? "Paused" : "Stopped";
		const speed = s === "paused" && resumeSpeed != null ? `${kmh(resumeSpeed)} km/h` : "--";
		return { label, speed, bar: 0, color: AMBER };
	}
	if (s === "starting") {
		return { label: `Starting ${status.countdown ?? ""}`.trim(), speed: "--", bar: 0, color: AMBER };
	}
	return { label: `State ${s.unknown}`, speed: "--", bar: 0, color: GRAY };
}
