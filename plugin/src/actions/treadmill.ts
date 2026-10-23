import streamDeck, {
	action,
	type DialAction,
	type DialDownEvent,
	type DialRotateEvent,
	type DidReceiveSettingsEvent,
	type KeyAction,
	type KeyDownEvent,
	SingletonAction,
	type WillAppearEvent,
} from "@elgato/streamdeck";

import { describe } from "../display";
import type { Helper, HelperState, Request } from "../helper";

type Settings = {
	/** Maximum speed position (1 to 8, i.e. km/h). */
	maxSpeed?: number;
};

const DEFAULT_MAX = 8;

@action({ UUID: "org.eeckhaut.deskfit.treadmill" })
export class Treadmill extends SingletonAction<Settings> {
	private resumeSpeed: number | null = null;

	constructor(private readonly helper: Helper) {
		super();
		helper.on("state", (state) => this.renderAll(state));
	}

	override async onWillAppear(ev: WillAppearEvent<Settings>): Promise<void> {
		this.configure(ev.payload.settings);
		if (ev.action.isDial() || ev.action.isKey()) await this.render(ev.action, this.helper.state);
	}

	override onDidReceiveSettings(ev: DidReceiveSettingsEvent<Settings>): void {
		this.configure(ev.payload.settings);
	}

	override async onDialRotate(ev: DialRotateEvent<Settings>): Promise<void> {
		// One tick is one whole position, like the remote.
		await this.send(ev.action, { cmd: "adjust", delta: ev.payload.ticks });
	}

	override async onDialDown(ev: DialDownEvent<Settings>): Promise<void> {
		await this.send(ev.action, { cmd: "toggle" });
	}

	override async onKeyDown(ev: KeyDownEvent<Settings>): Promise<void> {
		await this.send(ev.action, { cmd: "toggle" });
	}

	private configure(settings: Settings): void {
		const max = Math.round((settings.maxSpeed ?? DEFAULT_MAX) * 10);
		this.helper.request({ cmd: "configure", max_speed: max }).catch((e) => streamDeck.logger.warn(`configure: ${e.message}`));
	}

	private async send(target: DialAction<Settings> | KeyAction<Settings>, request: Request): Promise<void> {
		try {
			await this.helper.request(request);
		} catch (e) {
			streamDeck.logger.warn(`${request.cmd}: ${(e as Error).message}`);
			await target.showAlert();
		}
	}

	private renderAll(state: HelperState): void {
		if (state.status?.state === "running") this.resumeSpeed = state.status.speed;
		for (const a of this.actions) {
			if (a.isDial() || a.isKey()) void this.render(a, state);
		}
	}

	private async render(target: DialAction<Settings> | KeyAction<Settings>, state: HelperState): Promise<void> {
		const d = describe(state, this.resumeSpeed);
		if (target.isDial()) {
			await target.setFeedback({
				state: { value: d.label, color: d.color },
				speed: d.speed,
				bar: { value: d.bar, range: { min: 0, max: state.max_speed }, bar_fill_c: d.color },
			});
		} else {
			await target.setTitle(`${d.label}\n${d.speed}`);
		}
	}
}
