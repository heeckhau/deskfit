# DeskFit Bluetooth protocol

Findings for a Sportstech DeskFit200 (label: model DeskFit200, distributor
InnovaMaxx GmbH, Berlin), tested on macOS in October 2026. The command set
comes from [machinekoder/deskfit] and [leoluk/dft200-go]; what was verified
on hardware is marked below.

## Device

- Advertised name: `V-RUN1-xxxx` (a per-device suffix).
- On macOS, CoreBluetooth exposes a per-host peripheral UUID, not the
  hardware address. `deskfit setup` stores it; on another Mac, run setup
  again.
- The Bluetooth interface is unauthenticated: anyone in range can connect.
  The treadmill accepts one connection at a time.

| Service / characteristic | UUID | Use |
| --- | --- | --- |
| Vendor service | `0000fff0-0000-1000-8000-00805f9b34fb` | |
| Status | `0000fff1-0000-1000-8000-00805f9b34fb` | notify |
| Commands | `0000fff2-0000-1000-8000-00805f9b34fb` | write, write without response |
| Device information | `0000180a-0000-1000-8000-00805f9b34fb` | |

## Commands

    f0 c3 03 COMMAND VALUE 00 CHECKSUM

`03` is the payload length; the checksum is the sum of the preceding six
bytes modulo 256. For example, pause is `f0 c3 03 04 00 00 ba`.

| Command | Byte | Value | Verified |
| --- | --- | --- | --- |
| Fetch status | 00 | 0 | Yes: one status notification per request |
| Start | 01 | 0 | Resume from pause: yes. Start from stop: not yet |
| Stop | 02 | 0 | Not yet |
| Set speed | 03 | speed, 10-80 | Yes, while running (10, 30, 35, 50) |
| Pause | 04 | 0 | Yes |

An acknowledged write does not prove the motor obeyed; check the status that
follows. The treadmill reportedly also has EEPROM read/write commands, and
unknown commands may brick it. This project can only encode the five
commands above.

## Status notifications

Status arrives on fff1 in reply to Fetch status; polling every 500 ms works.
Whether the treadmill ever notifies unprompted is untested. Example:

    f0 d3 10 1c 04 00 1d 00 0c 00 1e 00 01 62 04 00 00 36 00 d7

Validate length (20), header (`f0 d3 10`, where `10` is the payload length)
and checksum (sum of bytes 0-18 modulo 256 equals byte 19) before trusting
any field. Zero-based offsets:

| Bytes | Meaning | Verified |
| --- | --- | --- |
| 3, 4 | elapsed seconds, minutes | Yes |
| 5, 6 | distance: km and hundredths of a km | Yes, including rollover |
| 7, 8 | calories as b7 * 100 + b8 | Increases plausibly |
| 10 | speed in tenths of km/h | Yes, see below |
| 12, 13 | steps as b12 * 100 + b13 | Not usable, see below |
| 14 | state: 1 stopped, 2 starting, 3 running, 4 paused | Yes for running and paused |
| 16 | countdown while starting | Not yet |
| 17 | unknown (0 while running, 0x36 once while paused) | |

## Speed

The reference implementations map speed 10-80 to levels 1-8 with nominal
speeds of 0.7 to 7.1 km/h. On this treadmill the distance counter instead
advanced at exactly speed / 10 km/h: 0.01 km every 12 s at speed 30
(3.0 km/h) and every 7-8 s at 50. So the speed byte is km/h x 10, and the
remote's positions 1-8 are 1-8 km/h. The treadmill display shows only the
position number.

Values between positions (e.g. 35) are accepted and change the distance
counter, but a rider noticed no difference in belt speed between 3.0 and
3.9. The Stream Deck plugin therefore sets whole positions only.

While running, the reported speed jumps to a new setting at once, so it is
the target speed, not a measurement of the belt.

## Observed behavior

- Pause: the state becomes paused immediately. The reported speed then
  falls (3.0, 2.1, ... 0.5 over about 7 s) and reads 0 once stopped.
  Elapsed time freezes.
- Start while paused resumes the same session (time and distance kept) at
  the previous speed, without a starting state or countdown.
- The step counter barely moves while walking (about 10 steps in 20
  minutes) and jumps during speed changes; it is not usable.

Not yet tested: start from stopped and its countdown, stop, set speed while
paused, and the auto-stop after a long pause.

## Reported by dft200-go, not verified here

- The treadmill stops by itself after being paused for a few minutes.
- Pressing start twice on the hardware remote triggers an emergency stop.
  No emergency-stop command is known.

[machinekoder/deskfit]: https://github.com/machinekoder/deskfit
[leoluk/dft200-go]: https://github.com/leoluk/dft200-go
