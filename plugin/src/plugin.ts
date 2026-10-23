import streamDeck from "@elgato/streamdeck";

import { Treadmill } from "./actions/treadmill";
import { Helper } from "./helper";

streamDeck.logger.setLevel("info");

const helper = new Helper();
helper.start();

streamDeck.actions.registerAction(new Treadmill(helper));
streamDeck.connect();
