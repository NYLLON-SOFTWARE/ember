import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import { preferenceKey, readMode } from "../src/appearance.js";
import { install } from "../src/lifecycle.js";

class Events {
  listeners = new Map();
  addEventListener(name, fn) {
    const listeners = this.listeners.get(name) || [];
    listeners.push(fn);
    this.listeners.set(name, listeners);
  }
  emit(name, event = {}) {
    for (const fn of this.listeners.get(name) || []) fn(event);
  }
}

function fixture({ mode = null, dark = false, storageUnavailable = false } = {}) {
  const media = Object.assign(new Events(), { matches: dark });
  const document = new Events();
  const classes = new Set();
  document.documentElement = {
    classList: { toggle: (name, enabled) => enabled ? classes.add(name) : classes.delete(name), contains: name => classes.has(name) },
    dataset: {}, style: {},
  };
  document.body = { dataset: { styleProfile: "basecoat" } };
  document.readyState = "complete";
  const select = { value: null, matches: () => true };
  const control = { hidden: true };
  let components = [];
  document.querySelectorAll = selector => ({
    "[data-appearance-select]": [select], "[data-appearance-control]": [control], "[data-basecoat-component]": components,
  })[selector] || [];
  document.querySelector = selector => document.querySelectorAll(selector)[0] || null;
  const storage = new Map(mode ? [[preferenceKey, mode]] : []);
  const calls = [];
  const window = Object.assign(new Events(), {
    matchMedia: () => media,
    location: { assign: url => calls.push(["navigate", url]) },
    localStorage: {
      getItem: key => { if (storageUnavailable) throw new Error("denied"); return storage.get(key) || null; },
      setItem: (key, value) => { if (storageUnavailable) throw new Error("denied"); storage.set(key, value); },
    },
    basecoat: {
      start: () => calls.push(["start", document.body]),
      stop: () => calls.push(["stop"]),
      initAll: options => calls.push(["init", options]),
    },
  });
  return { window, document, media, select, control, storage, calls, setComponents: value => { components = value; } };
}

test("appearance keeps legacy preferences and gives the Matchbox preference priority", () => {
  const { window, storage } = fixture();
  storage.set("campfire:appearance", "dark");
  assert.equal(readMode(window), "dark");
  storage.set(preferenceKey, "light");
  assert.equal(readMode(window), "light");
});

test("the shipped prepaint script honors explicit preferences, system and inaccessible storage", async () => {
  const script = await readFile(new URL("../theme-init.js", import.meta.url), "utf8");
  for (const [mode, systemDark, denied, expectedDark] of [
    ["light", true, false, false], ["dark", false, false, true], ["system", true, false, true],
    ["invalid", true, false, true], ["dark", false, true, false],
  ]) {
    const { window, document } = fixture({ mode, dark: systemDark, storageUnavailable: denied });
    vm.runInNewContext(script, { window, document });
    assert.equal(document.documentElement.classList.contains("dark"), expectedDark);
    assert.equal(document.documentElement.style.colorScheme, expectedDark ? "dark" : "light");
  }
});

test("appearance selection persists, follows system changes only in System mode, and stays usable without storage", () => {
  for (const denied of [false, true]) {
    const f = fixture({ storageUnavailable: denied });
    install(f.window, f.document);
    assert.equal(f.control.hidden, false);
    assert.equal(f.select.value, "system");
    f.media.matches = true;
    f.media.emit("change");
    assert.equal(f.document.documentElement.classList.contains("dark"), true);
    f.select.value = "light";
    f.document.emit("change", { target: f.select });
    f.media.emit("change");
    assert.equal(f.document.documentElement.classList.contains("dark"), false);
    if (!denied) assert.equal(f.storage.get(preferenceKey), "light");
  }
});

test("Turbo snapshots clean component listeners and restart observation on the replacement body", () => {
  const f = fixture();
  install(f.window, f.document);
  let destroyed = 0;
  const component = { dataset: { basecoatComponent: "dialog" }, _destroy: () => destroyed++, removeAttribute() {} };
  f.setComponents([component]);
  f.document.emit("turbo:before-cache");
  assert.equal(destroyed, 1);
  assert.equal(component.dataset.basecoatComponent, undefined);
  f.setComponents([]);
  f.document.body = { dataset: { styleProfile: "basecoat" } };
  f.document.emit("turbo:render");
  const starts = f.calls.filter(([kind]) => kind === "start");
  assert.equal(starts.length, 2);
  assert.equal(starts[1][1], f.document.body);
  f.document.emit("turbo:load");
  assert.equal(f.calls.filter(([kind]) => kind === "start").length, 2);
});

test("restored initialized markup is force-initialized, and legacy pages do not inherit appearance controls", () => {
  const f = fixture();
  f.setComponents([{ dataset: { basecoatComponent: "dialog" } }]);
  install(f.window, f.document);
  assert.deepEqual(f.calls.find(([kind]) => kind === "init"), ["init", { force: true }]);
  f.setComponents([]);
  f.document.body = { dataset: { styleProfile: "legacy" } };
  f.document.emit("turbo:render");
  const count = f.calls.filter(([kind]) => kind === "start").length;
  f.document.emit("turbo:load");
  assert.equal(f.calls.filter(([kind]) => kind === "start").length, count);
});

test("cross-profile frames navigate to the final URL; same-profile and non-HTML responses stay within Turbo", () => {
  const f = fixture();
  install(f.window, f.document);
  const options = { headers: {} };
  f.document.emit("turbo:before-fetch-request", { detail: { fetchOptions: options } });
  assert.equal(options.headers["X-Matchbox-Style-Profile"], "basecoat");
  let prevented = 0;
  for (const [profile, type, ok] of [["legacy", "text/html", true], ["basecoat", "text/html", true], ["legacy", "application/json", true], ["legacy", "text/html", false]]) {
    f.document.emit("turbo:before-fetch-response", {
      target: { closest: () => ({}) },
      preventDefault: () => prevented++,
      detail: { fetchResponse: { response: {
        ok, url: "http://localhost/rooms/1", headers: new Headers({ "X-Matchbox-Style-Profile": profile, "Content-Type": type }),
      } } },
    });
  }
  assert.equal(prevented, 1);
  assert.deepEqual(f.calls.filter(([kind]) => kind === "navigate"), [["navigate", "http://localhost/rooms/1"]]);
});
