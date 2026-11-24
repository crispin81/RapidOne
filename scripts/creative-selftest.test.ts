/**
 * The Creative edit's own logic, checked without a browser or any new package: node runs
 * TypeScript directly (type stripping) with its own test runner.
 *   node --test scripts/creative-selftest.test.ts   (npm run test:creative)
 *
 * Covers what the UI relies on and can't see: the defaults (Bokeh off, so a photo that never
 * touched Creative is not an edit), a change being one, the lens line, the engine's object
 * shape, copy/paste of the Creative (Bokeh) group, and an old .rpd without `creative` loading.
 */

import test from "node:test";
import assert from "node:assert/strict";
import { addLight, FIRST_LIGHT_AMBIENT, moonAmbient, sunAmbient, sunKelvin, isCelestial, intensityForWatts, LIGHT_TYPES, lightWatts, TYPE_WATTS, wattsText, aimAngle, aimFromAngle, aimFromSun, aimThrow, sunFromAim, typeOf, withLightType, creativeHold, creativePanelForKey, DEFAULT_BOKEH, DEFAULT_CREATIVE, DEFAULT_LIGHT, DEFAULT_MIST, DEFAULT_RELIGHT, FLASH_PRESETS, hasCreative, lensLine, normalizeCreative, removeLight, withBokeh, withFlashPreset, withLight, withMist, withRelight } from "../src/creative.ts";
import { pasteSnapshot } from "../src/copySettings.ts";
import { editFromSnapshot, parseSidecar, snapshotFromEdit, sidecarText, editHasContent } from "../src/sidecar.ts";

test("bokeh starts off, and an untouched photo is not an edit", () => {
  assert.equal(DEFAULT_BOKEH.enabled, false);
  assert.equal(hasCreative(undefined), false);
  assert.equal(hasCreative(DEFAULT_CREATIVE), false);
  // Turning it on is a change; so is a lens character left on a switched-off Bokeh.
  assert.equal(hasCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true } }), true);
  assert.equal(hasCreative({ bokeh: { ...DEFAULT_BOKEH, aperture: 1.4 } }), true);
});

test("bokeh and mist are on exactly while they have strength", () => {
  // Moving the strength turns it on; putting it back (0: the shot's own) turns it off.
  const open = withBokeh(undefined, { aperture: 1.4 });
  assert.equal(open.bokeh.enabled, true);
  assert.equal(withBokeh(open, { aperture: 0 }).bokeh.enabled, false);
  // A setting without strength doesn't.
  assert.equal(withBokeh(undefined, { focusRange: 20, boost: 50 }).bokeh.enabled, false);
  // Manual has its own strength, and Open's is still there on the way back.
  const manual = withBokeh(open, { mode: "manual" });
  assert.equal(manual.bokeh.enabled, false);
  const back = withBokeh(manual, { mode: "open" });
  assert.equal(back.bokeh.enabled, true);
  assert.equal(back.bokeh.aperture, 1.4);
  assert.equal(withMist(undefined, { glow: 40, height: 20 }).mist.enabled, false);
  assert.equal(withMist(undefined, { density: 30 }).mist.enabled, true);
});

test("simulate becomes lens based, and protect goes", () => {
  const c = normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true, mode: "simulate", aperture: 1.4, focal: 135 }, mist: { ...DEFAULT_MIST, enabled: true, density: 30, protectSubject: 80 } });
  assert.equal(c.bokeh.mode, "open");
  assert.equal(c.bokeh.aperture, 1.4);
  assert.equal(c.bokeh.enabled, true);
  assert.equal(c.mist.protectSubject, 0);
  assert.equal(normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true, mode: "simulate", focal: 135 } }).bokeh.enabled, false);
});

test("creative letters: each panel's are its own, and only sliders showing answer", () => {
  assert.equal(creativePanelForKey("KeyB"), "Bokeh");
  assert.equal(creativePanelForKey("KeyT"), undefined);
  // Each slider's letter is its name's first: Blur, Sharp, Sides (I), Edge, Vintage, Glow.
  assert.equal(creativeHold("Bokeh", "KeyB", DEFAULT_BOKEH), "creative:aperture");
  assert.equal(creativeHold("Bokeh", "KeyB", { ...DEFAULT_BOKEH, mode: "manual" }), "creative:amount");
  assert.equal(creativeHold("Bokeh", "KeyS", DEFAULT_BOKEH), "creative:focusRange");
  assert.equal(creativeHold("Bokeh", "KeyI", DEFAULT_BOKEH), undefined);
  assert.equal(creativeHold("Bokeh", "KeyI", { ...DEFAULT_BOKEH, shape: "blades" }), "creative:blades");
  assert.equal(creativeHold("Bokeh", "KeyV", DEFAULT_BOKEH), "creative:catsEye");
  // Mist: Amount, Far, Near, Soft, Warm, Ground, Glow (L), Patchy.
  assert.equal(creativeHold("Mist", "KeyF", DEFAULT_BOKEH), "creative:far");
  assert.equal(creativeHold("Mist", "KeyN", DEFAULT_BOKEH), "creative:start");
  assert.equal(creativeHold("Mist", "KeyT", DEFAULT_BOKEH), undefined, "Tint is set for you now");
  // Relight: Shift+R selects the panel; Ambient and Relief always answer, a light's sliders only
  // with one selected, and Depth/Cone/Azimuth only where they show.
  assert.equal(creativePanelForKey("KeyR"), "Relight");
  assert.equal(creativeHold("Relight", "KeyA", DEFAULT_BOKEH, DEFAULT_RELIGHT), "creative:relight:ambient");
  assert.equal(creativeHold("Relight", "KeyR", DEFAULT_BOKEH, DEFAULT_RELIGHT), "creative:relight:relief");
  assert.equal(creativeHold("Relight", "KeyI", DEFAULT_BOKEH, DEFAULT_RELIGHT, null), undefined);
  const spot = withLight(addLight(undefined, "spot"), 1, {});
  assert.equal(creativeHold("Relight", "KeyI", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:intensity");
  assert.equal(creativeHold("Relight", "KeyC", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:cone");
  assert.equal(creativeHold("Relight", "KeyX", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:u");
  assert.equal(creativeHold("Relight", "KeyY", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:v");
  assert.equal(creativeHold("Relight", "KeyJ", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:aim");
  // P is Power now (the Depth slider went: depth is set in the 3D space).
  assert.equal(creativeHold("Relight", "KeyP", DEFAULT_BOKEH, spot.relight, 1), "creative:light:1:intensity");
  assert.equal(creativeHold("Depth", "KeyA", DEFAULT_BOKEH), undefined);
});

test("an edit left switched off before stays invisible", () => {
  const old = normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, aperture: 1.8, focal: 85, amount: 50 }, mist: { ...DEFAULT_MIST, density: 40 } });
  assert.equal(old.bokeh.aperture, 0);
  assert.equal(old.mist.density, 0);
  assert.equal(withBokeh(old, { boost: 20 }).bokeh.enabled, false);
  // One that was on keeps its settings.
  const on = normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true, aperture: 1.8 }, mist: { ...DEFAULT_MIST, enabled: true, density: 40 } });
  assert.equal(on.bokeh.aperture, 1.8);
  assert.equal(on.mist.density, 40);
});

test("mist starts off and is normalised from an older edit", () => {
  assert.equal(DEFAULT_MIST.enabled, false);
  assert.equal(hasCreative({ mist: { ...DEFAULT_MIST, enabled: true } }), true);
  assert.equal(hasCreative({ mist: { ...DEFAULT_MIST, density: 80 } }), true);
  // A phase 2 creative object (Bokeh only) gets Mist's defaults.
  const old = normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true } });
  assert.equal(old.mist.enabled, false);
  assert.equal(old.mist.start, DEFAULT_MIST.start);
  const m = withMist({ ...old, bokeh: { ...old.bokeh, aperture: 1.4 } }, { start: 30, height: 50 });
  assert.equal(m.mist.start, 30);
  assert.equal(m.mist.height, 50);
  assert.equal(m.bokeh.enabled, true);
});

test("a panel change is a copy, leaving the default object alone", () => {
  const next = withBokeh(undefined, { focusAuto: false, focusU: 0.2, focusV: 0.8 });
  assert.equal(next.bokeh.focusAuto, false);
  assert.equal(next.bokeh.focusU, 0.2);
  // The defaults are untouched (they're shared by every photo).
  assert.equal(DEFAULT_BOKEH.focusU, 0.5);
  const again = withBokeh(next, { mode: "manual" });
  assert.equal(again.bokeh.focusU, 0.2);
  assert.equal(again.bokeh.mode, "manual");
});

test("relight starts off, with no lights, and is normalised from an older edit", () => {
  assert.equal(DEFAULT_RELIGHT.enabled, false);
  assert.equal(DEFAULT_RELIGHT.lights.length, 0);
  assert.equal(hasCreative({ relight: DEFAULT_RELIGHT }), false);
  // A phase 3 edit (Bokeh and Mist only) gets Relight's defaults.
  const old = normalizeCreative({ bokeh: { ...DEFAULT_BOKEH, enabled: true }, mist: { ...DEFAULT_MIST, enabled: true, density: 30 } });
  assert.deepEqual(old.relight, DEFAULT_RELIGHT);
  // A light is one; switching the last one off and putting Ambient back returns to none.
  const added = addLight(undefined, "point");
  assert.equal(added.relight.lights.length, 1);
  assert.equal(added.relight.enabled, true);
  assert.equal(hasCreative(added), true);
  // (The first light also brought the ambient a stop down: put back, it is as before.)
  const off = withRelight(withLight(added, added.relight.lights[0].id, { on: false }), { ambient: 0 });
  assert.equal(off.relight.enabled, false);
  assert.equal(hasCreative(off), true, "a light kept off is still a change to keep");
  // Ambient alone is a change; Ambient back to 0 with no lights is none.
  assert.equal(withRelight(undefined, { ambient: -30 }).relight.enabled, true);
  assert.equal(hasCreative(withRelight(undefined, { ambient: 0 })), false);
});

test("relight's lights are edited by id and removed cleanly", () => {
  const two = addLight(addLight(undefined, "point"), "sun");
  const [a, b] = two.relight.lights;
  assert.notEqual(a.id, b.id);
  assert.equal(two.relight.lights[0].kind, "point");
  assert.equal(two.relight.lights[1].kind, "sun");
  const moved = withLight(two, b.id, { azimuth: 120, intensity: 70 });
  assert.equal(moved.relight.lights[1].azimuth, 120);
  assert.equal(moved.relight.lights[0].intensity, DEFAULT_LIGHT.intensity);
  const one = removeLight(moved, a.id);
  assert.equal(one.relight.lights.length, 1);
  assert.equal(one.relight.lights[0].id, b.id);
  // A light from a .rpd with fields missing gets the rest's defaults.
  const saved = normalizeCreative({ relight: { ...DEFAULT_RELIGHT, enabled: true, lights: [{ id: 4, kind: "spot" } as never] } });
  assert.equal(saved.relight.lights[0].kind, "spot");
  assert.equal(saved.relight.lights[0].kelvin, DEFAULT_LIGHT.kelvin);
  assert.equal(saved.relight.lights[0].feather, DEFAULT_LIGHT.feather);
});

test("a sun's arrow and its Around/Height sliders are one state", () => {
  // The arrow's place comes from the sliders, and dragging it writes them back: round-tripping
  // leaves the same arrow where it was.
  for (const [az, el] of [[0, 0], [45, 30], [90, 60], [180, 12], [270, 85], [340, 40]] as [number, number][]) {
    const p = aimFromSun(az, el);
    const back = sunFromAim(p.x, p.y);
    assert.ok(Math.abs(back.azimuth - az) <= 2 || Math.abs(back.azimuth - az) >= 358, `${az} -> ${back.azimuth}`);
    assert.ok(Math.abs(back.elevation - el) <= 2, `${el} -> ${back.elevation}`);
  }
  // Up is 90°, right is 0°, out from the centre raises the sun.
  assert.equal(sunFromAim(0.5, 0.1).azimuth, 90);
  assert.equal(sunFromAim(0.9, 0.5).azimuth, 0);
  assert.ok(sunFromAim(0.75, 0.5).elevation > sunFromAim(0.6, 0.5).elevation);
  // A light's aim dot is stored on the light, so the panel's readout and the dot agree.
  const spot = withLight(addLight(undefined, "spot"), 1, { aimU: 0.25, aimV: 0.75 });
  assert.equal(spot.relight.lights[0].aimU, 0.25);
  assert.equal(spot.relight.lights[0].aimV, 0.75);
});

test("a light's type sets its character and never its place", () => {
  const lamp = { ...DEFAULT_LIGHT, id: 3, u: 0.21, v: 0.66, depth: 73 };
  const soft = withLightType(lamp, "softbox");
  const grid = withLightType(lamp, "grid");
  // Character changes…
  assert.equal(soft.kind, "point");
  assert.equal(grid.kind, "spot");
  assert.ok(soft.size > grid.size);
  assert.ok(grid.cone < soft.cone);
  // …and the place never moves.
  for (const l of [soft, grid]) assert.deepEqual([l.u, l.v, l.depth], [lamp.u, lamp.v, lamp.depth]);
  // The dropdown shows the type back.
  assert.equal(typeOf(soft), "softbox");
  assert.equal(typeOf(grid), "grid");
  assert.equal(typeOf(withLightType(lamp, "bare")), "bare");
});

test("a lamp's Aim and Throw sliders and its dot are one state", () => {
  const lamp = { ...DEFAULT_LIGHT, u: 0.3, v: 0.3 };
  // Aimed up and to the right: 45°, a throw out to the frame's middle.
  const up = aimFromAngle(lamp, 45, 50);
  assert.ok(up.aimU > lamp.u && up.aimV < lamp.v, "45° is right and up");
  assert.ok(Math.abs(aimAngle({ ...lamp, ...up }) - 45) <= 1);
  assert.ok(Math.abs(aimThrow({ ...lamp, ...up }) - 50) <= 1);
  // Right, left, down.
  assert.equal(aimAngle({ ...lamp, ...aimFromAngle(lamp, 0, 40) }), 0);
  assert.equal(aimAngle({ ...lamp, ...aimFromAngle(lamp, 180, 40) }), 180);
  assert.ok(aimFromAngle(lamp, 270, 40).aimV > lamp.v);
  // A throw of 0 aims at the lamp itself, which is safe (the engine falls back to the camera axis).
  assert.deepEqual(aimFromAngle(lamp, 90, 0).aimU, lamp.u);
});

test("the flash presets are complete, sane looks, and applying one copies", () => {
  const ids = new Set<string>();
  for (const p of FLASH_PRESETS) {
    assert.equal(ids.has(p.id), false, `duplicate preset ${p.id}`);
    ids.add(p.id);
    assert.ok(p.lights.length >= 1, `${p.id} has no light`);
    assert.ok(p.ambient < 0, `${p.id} should dim the ambient`);
    for (const l of p.lights) {
      assert.ok(["point", "sun", "spot"].includes(l.kind), `${p.id}: ${l.kind}`);
      assert.ok(l.intensity > 0 && l.intensity <= 100);
      assert.ok(l.kelvin >= 2000 && l.kelvin <= 12000);
      assert.ok(l.size >= 0 && l.size <= 100 && l.softness >= 0 && l.softness <= 100);
      assert.ok(l.cone >= 5 && l.cone <= 85);
    }
  }
  // Applying one is a whole Relight, with number ids; it makes the photo an edit.
  const applied = withFlashPreset(undefined, FLASH_PRESETS[0]);
  assert.equal(applied.relight.enabled, true);
  assert.equal(applied.relight.ambient, FLASH_PRESETS[0].ambient);
  assert.equal(applied.relight.lights[0].id, 1);
  assert.equal(hasCreative(applied), true);
  // And it never touches the shared defaults.
  assert.equal(DEFAULT_RELIGHT.lights.length, 0);
  // A bare flash is harder than a softbox: smaller Size and Soft.
  const bare = FLASH_PRESETS.find((p) => p.id === "bare")!;
  const soft = FLASH_PRESETS.find((p) => p.id === "softbox")!;
  assert.ok(bare.lights[0].size < soft.lights[0].size && bare.lights[0].softness < soft.lights[0].softness);
});

test("the lens line says what the shot was and where the focus came from", () => {
  assert.equal(lensLine(null), "No lens in this photo's EXIF");
  const exif = lensLine({ lens: "85 mm Prime", focalMm: 85, fNumber: 1.8, focal35mm: 85, sensorWidthMm: 36, focusDistanceM: 2.4, focusSource: "exif", dNear: 1, dFar: 10 });
  assert.match(exif, /85 mm · shot at f\/1\.8/);
  assert.match(exif, /focus 2\.4 m/);
  const click = lensLine({ lens: null, focalMm: 102, fNumber: 3.2, focal35mm: 102, sensorWidthMm: 36, focusDistanceM: null, focusSource: "click", dNear: 1, dFar: 10 });
  assert.match(click, /102 mm · shot at f\/3\.2/);
  assert.match(click, /focus: from your click/);
});

test("copy and paste carries the Creative (Bokeh, Mist, Relight) group only when ticked", () => {
  const source = { creative: { bokeh: { ...DEFAULT_BOKEH, enabled: true, mode: "open" as const, aperture: 1.4 }, mist: { ...DEFAULT_MIST, enabled: true, density: 70 }, relight: addLight(undefined, "sun").relight } };
  const target = { edits: { exposure: 0.5 } };
  // Ticked: the source's Creative is pasted.
  const on = pasteSnapshot(source, target, ["creative"]);
  assert.deepEqual(on.creative, source.creative);
  assert.deepEqual(on.edits, target.edits);
  // Not ticked: the target keeps its own (none).
  const off = pasteSnapshot(source, target, ["exposure"]);
  assert.equal(off.creative, undefined);
  // A source with none clears a target's, as the other groups do.
  const cleared = pasteSnapshot({}, { creative: source.creative }, ["creative"]);
  assert.equal(cleared.creative, undefined);
});

test("an old .rpd without creative loads, and a new one round-trips", () => {
  const old = '{"format":"rapidone-sidecar","version":1,"origin":"rapidone","edit":{"sliders":{"exposure":0.4}}}';
  const parsedOld = parseSidecar(old);
  assert.ok(parsedOld);
  const snap = snapshotFromEdit(parsedOld.edit);
  assert.equal(snap.creative, undefined);
  assert.equal(snap.edits?.exposure, 0.4);
  // A Creative edit is written only when it changes something, and reads back unchanged.
  const creative = { bokeh: { ...DEFAULT_BOKEH, enabled: true, shape: "mirror" as const, catsEye: 40 } };
  const text = sidecarText({ origin: "rapidone", edit: editFromSnapshot({ creative }) });
  const parsed = parseSidecar(text);
  assert.deepEqual(parsed?.edit.creative, creative);
  assert.deepEqual(snapshotFromEdit(parsed!.edit).creative, normalizeCreative(creative));
  assert.equal(editHasContent(editFromSnapshot({ creative })), true);
  // A default Creative is not written and doesn't make an empty edit an edit.
  assert.equal(editFromSnapshot({ creative: DEFAULT_CREATIVE }).creative, undefined);
  assert.equal(editHasContent(editFromSnapshot({ edits: {}, creative: DEFAULT_CREATIVE })), false);
});

test("each light type starts at its typical flash power, and Power round-trips through watt-seconds", () => {
  for (const t of LIGHT_TYPES.filter((x) => x.kind !== "sun" && x.kind !== "moon")) {
    const l = withLightType({ ...DEFAULT_LIGHT, id: 1 }, t.id);
    assert.ok(Math.abs(lightWatts(l) - TYPE_WATTS[t.id]) < 1e-6, `${t.id} should start at ${TYPE_WATTS[t.id]} Ws`);
    assert.ok(Math.abs(lightWatts(l, l.intensity + 25) - 2 * TYPE_WATTS[t.id]) < 1e-6, "a stop up doubles the power");
    assert.ok(Math.abs(intensityForWatts(l, lightWatts(l, 37)) - 37) < 1e-6, "watts back to the same intensity");
  }
  const added = addLight(undefined, "point", "bounce").relight.lights[0];
  assert.equal(typeOf(added), "bounce");
  assert.equal(wattsText(lightWatts(added)), "60");
});

test("the first light brings an untouched ambient a stop down, and never changes it after", () => {
  const one = addLight(undefined, "point", "softbox");
  assert.equal(one.relight.ambient, FIRST_LIGHT_AMBIENT);
  const moved = withRelight(one, { ambient: -10 });
  assert.equal(addLight(moved, "point", "rim").relight.ambient, -10, "a second light leaves the ambient as set");
  const set = withRelight(undefined, { ambient: 20 });
  assert.equal(addLight(set, "point").relight.ambient, 20, "an ambient already set is kept");
});

test("the sun and moon are kept as themselves, a low sun is warm, and a first moon makes it night", () => {
  const sun = addLight(undefined, "point", "sun", { azimuth: 220, elevation: 10 }).relight.lights[0];
  assert.equal(sun.kind, "sun");
  assert.ok(isCelestial(sun));
  assert.equal(normalizeCreative({ relight: { ...DEFAULT_RELIGHT, enabled: true, lights: [sun] } } as never).relight.lights[0].kind, "sun", "a saved sun stays a sun");
  assert.ok(sunKelvin(2) < 3400 && sunKelvin(60) > 5000 && sunKelvin(10) < sunKelvin(25), "orange low, white high");
  assert.equal(addLight(undefined, "point", "moon", { elevation: 35 }).relight.ambient, moonAmbient(35));
  assert.ok(moonAmbient(10) < moonAmbient(45) && moonAmbient(45) <= -55, "a higher moon is a brighter night, still night");
  // A first sun sets the ambient for its height: bright under a high sun, dim at sunset.
  assert.equal(addLight(undefined, "point", "sun", { elevation: 60 }).relight.ambient, sunAmbient(60));
  assert.ok(sunAmbient(60) > sunAmbient(10) && sunAmbient(10) > sunAmbient(0), "the ambient falls as the sun sets");
  assert.ok(sunAmbient(0) >= -70 && sunAmbient(60) <= -15);
});

test("the sky sets the ambient when a sun or moon arrives, for its height", () => {
  // Ambient already moved (-10, from an earlier sun or by hand): a moon still brings it down.
  const lit = withRelight(undefined, { ambient: -10 });
  const night = addLight(lit, "point", "moon", { elevation: 20 });
  assert.equal(night.relight.ambient, moonAmbient(20));
  // A sun after a night: the sun's day; and a sunset sun brings a bright ambient down.
  assert.equal(addLight(withRelight(undefined, { ambient: -80 }), "point", "sun", { elevation: 40 }).relight.ambient, sunAmbient(40));
  assert.equal(addLight(lit, "point", "sun", { elevation: 3 }).relight.ambient, sunAmbient(3));
  // With another sun or moon already on, the ambient stays.
  const withSun = addLight(undefined, "point", "sun", { elevation: 30 });
  assert.equal(addLight(withRelight(withSun, { ambient: -5 }), "point", "moon", { elevation: 20 }).relight.ambient, -5);
  // A room light leaves the ambient as it is.
  assert.equal(addLight(lit, "point", "softbox").relight.ambient, -10);
});
