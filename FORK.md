# Fork notes

This fork tracks [rmk-rs/rmk](https://github.com/rmk-rs/rmk) and adds two features. Each lives in its own files where possible, and touches upstream files only with small hooks, so rebasing onto upstream stays easy.

## Home row mods that resolve on press

`hold_on_other_press` now honours `unilateral_tap`, like QMK's [Chordal Hold](https://docs.qmk.fm/tap_hold#chordal-hold):

- A key press on the other hand, or on a bilateral (`*`) key, resolves the held key as **hold** at once.
- A key press on the same hand resolves it as **tap** at once.
- A tap-hold key on the same hand leaves both undecided, so modifiers on one hand can stack.

Hands come from the `L`, `R` and `*` markers in `layout.map`.

```toml
[behavior.morse]
enable_flow_tap = true
prior_idle_time = "120ms"

[behavior.morse.profiles]
HRM = { unilateral_tap = true, hold_on_other_press = true, hold_timeout = "250ms" }
# Thumb layer keys share a hand with that layer's modifiers in Miryoku, so no unilateral_tap.
THUMB = { unilateral_tap = false, hold_on_other_press = true, enable_flow_tap = false, hold_timeout = "200ms" }
```

| File | Change |
| --- | --- |
| `rmk/src/keyboard.rs` | Same-hand check in the `HoldOnOtherPress` arm of `make_decisions_for_keys` |
| `rmk/tests/scenarios/morse_hrm_hold_on_other_press.toml` | Scenario tests (new file) |

## nice!view display (Sharp LS011B7DH03)

Enable the `ls011b7dh03` feature of `rmk`, then configure each half in `keyboard.toml`. Use `peripheral` in place of `central` for the other half:

```toml
[split.central.display]
driver = "nice_view"   # or "ls011b7dh03"
size = "160x68"
rotation = 90          # 68x160 portrait
renderer = "OledRenderer"
power_pin = "P0_13"    # nice!nano VCC switch, cut while sleeping

[split.central.display.protocol.spi]   # nRF52 only; miso is not needed
instance = "SPI2"
sck = "P0_20"
mosi = "P0_17"
cs = "P0_06"
```

`power_pin` is the nice!nano's switched VCC output, which powers the display. The display stays dark without it. While the keyboard sleeps, the driver switches the panel off; on wake it powers the panel up, clears it and redraws. Anything else on VCC, such as RGB, goes off too. Leave `power_pin` out to keep the panel powered. Don't also list the pin under `[[output]]`.

`render_interval` defaults to 1000 ms for this panel, because it needs its VCOM toggled about once a second, and every flush toggles it. `BinaryColor::On` is a dark pixel on the light background.

| File | Change |
| --- | --- |
| `rmk/src/display/drivers/sharp_memory.rs` | Driver and its unit tests (new file) |
| `rmk-macro/src/codegen/sharp_memory_display.rs` | SPI and driver codegen, with tests (new file) |
| `rmk-macro/src/codegen/display.rs` | Hooks into the new module for SPI and the new driver |
| `rmk/src/display/mod.rs` | `DisplayDriver::set_sleeping` (default no-op), called by `DisplayProcessor` on sleep and wake |
| `rmk-config/src/lib.rs` | `Ls011b7dh03` driver variant; `power_pin` display field; `miso` is optional |
| `rmk/Cargo.toml`, `rmk/src/display/drivers/mod.rs` | `sharp_memory` and `ls011b7dh03` features |
| `rmk-config/tests/keyboard_toml_validation.rs` | Config test |

## Updating from upstream

`main` mirrors upstream; the fork's commits sit on top in their own branch.

```console
git fetch upstream
git checkout main && git merge --ff-only upstream/main && git push origin main
git checkout <fork-branch> && git rebase main
```

Conflicts, if any, land in the files listed above. After rebasing, run from `rmk/`:

```console
cargo nextest run --no-default-features --test integration morse_
cargo nextest run --no-default-features --features=ls011b7dh03 --lib sharp_memory
```

and from `rmk-macro/` and `rmk-config/`: `cargo nextest run`.
