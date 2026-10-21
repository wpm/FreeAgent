// What any Free Agent app's chrome must pass, for an app's screen check
// to import: the server, `check` and `section`, a console watcher, the
// shapes of controls and their focus rings, hover and focus help, a
// vertical tablist the arrows move through, and a stand-in for the
// desktop app's backend with the app's own answers merged in.
//
// It imports nothing but Node's own modules, so it runs with the
// importing app's Playwright.

export { serve } from './serve.mjs';

let failures = 0;

/** Errors the pages watched logged to the console or threw. */
const errors = [];

/** Logs `label` as passed or failed, on `condition`. */
export function check(condition, label) {
  console.log(`${condition ? 'ok  ' : 'FAIL'} ${label}`);
  if (!condition) failures += 1;
}

/** Runs the checks `checks` of `name`, failing them all on a throw, such as a wait that times out. */
export async function section(name, checks) {
  try {
    await checks();
  } catch (error) {
    check(false, `${name}: ${error.message.split('\n')[0]}`);
  }
}

/** `page`, its console errors and page errors recorded. */
export function watch(page) {
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  return page;
}

/** Checks that the pages watched kept the console clean, says how the
 * checks went, and exits with a failure if any failed. */
export function finish() {
  check(errors.length === 0, `the console stays clean${errors.length ? `: ${errors.join('; ')}` : ''}`);
  if (failures > 0) {
    console.log(`${failures} failed`);
    process.exit(1);
  }
  console.log('all passed');
}

/** The shape of each element `selector` matches on `page`: its box and
 * the computed styles the controls are drawn with. */
export function shapes(page, selector) {
  return page.$$eval(selector, (elements) =>
    elements.map((element) => {
      const style = getComputedStyle(element);
      const { height, width } = element.getBoundingClientRect();
      return {
        text: element.textContent.trim(),
        height,
        width,
        radius: style.borderTopLeftRadius,
        border: `${style.borderTopWidth} ${style.borderTopColor}`,
        background: style.backgroundColor,
        color: style.color,
        shadow: style.boxShadow,
        weight: style.fontWeight,
        font: style.fontFamily,
        appearance: style.appearance,
        icon: element.querySelector('svg') !== null || element.parentElement.querySelector(':scope > svg') !== null,
      };
    }),
  );
}

/** What has the focus on `page` after each of `count` presses of Tab,
 * and whether it shows the ink focus ring. */
export async function ringed(page, count) {
  const unringed = [];
  for (let press = 0; press < count; press += 1) {
    await page.keyboard.press('Tab');
    const focused = await page.evaluate(() => {
      const element = document.activeElement;
      const style = getComputedStyle(element);
      return {
        html: element.outerHTML.slice(0, 60),
        ring: style.outlineStyle === 'solid' && style.outlineWidth === '2px' && style.outlineOffset === '3px',
      };
    });
    if (!focused.ring) unringed.push(focused.html);
  }
  return unringed;
}

/** Checks the help on `page`, as `label`: hovering over `hover.selector`
 * shows help starting `hover.text`, focusing `focus.selector` shows help
 * starting `focus.text`, then saved to the screenshot `shot`, Escape hides
 * it, pressing `press` hides it, and help on a part of an element shows
 * under the part. */
export async function tips(page, label, { hover, focus, press, shot }) {
  const tip = () =>
    page.$eval('.fa-tip', (element) => ({ text: element.textContent, shown: !element.hidden, top: element.getBoundingClientRect().top }));
  await page.hover(hover.selector);
  const hovered = await tip();
  check(hovered.shown && hovered.text.startsWith(hover.text), `${label}: hovering shows help (${JSON.stringify(hovered)})`);
  await page.mouse.move(0, 0);
  await page.focus(focus.selector);
  const focused = await tip();
  check(focused.shown && focused.text.startsWith(focus.text), `${label}: focus shows help (${JSON.stringify(focused)})`);
  await page.screenshot({ path: shot });
  await page.keyboard.press('Escape');
  check(!(await tip()).shown, `${label}: Escape hides help`);
  await page.hover(press);
  await page.mouse.down();
  const pressed = await tip();
  await page.mouse.up();
  check(!pressed.shown, `${label}: pressing hides help (${JSON.stringify(pressed)})`);
  // An element whose help is placed by a part of it, as a handle's is by
  // its grip, shows its help under the part.
  await page.evaluate(() => {
    const whole = document.createElement('div');
    whole.id = 'whole';
    whole.dataset.tip = 'The part help';
    whole.dataset.tipAt = 'span';
    whole.style.cssText = 'position: fixed; left: 0; top: 0; width: 40px; height: 400px';
    whole.innerHTML = '<span style="position: absolute; top: 200px; width: 40px; height: 10px"></span>';
    document.body.append(whole);
  });
  await page.hover('#whole', { position: { x: 20, y: 20 } });
  const onPart = await tip();
  await page.mouse.move(600, 600);
  await page.evaluate(() => document.getElementById('whole').remove());
  check(
    onPart.shown && onPart.text === 'The part help' && Math.abs(onPart.top - 218) <= 1,
    `${label}: help on a part of an element shows under the part (${JSON.stringify(onPart)})`,
  );
}

/** Checks the vertical tablist `selector` on `page`, as `label`: its tabs
 * are named `names`, and from the first, the arrows move to the second
 * and back, the focus going with them. */
export async function tablist(page, label, selector, names) {
  const tabs = await page.$$eval(`${selector}[role="tablist"][aria-orientation="vertical"] [role="tab"]`, (buttons) =>
    buttons.map((button) => button.textContent.trim()),
  );
  check(JSON.stringify(tabs) === JSON.stringify(names), `${label}: the pages are a vertical tablist (${tabs.join(', ')})`);
  const current = () =>
    page.evaluate(() => ({
      selected: document.querySelector('[role="tab"][aria-selected="true"]').textContent,
      focused: document.activeElement.textContent,
    }));
  await page.focus(`${selector} [role="tab"]`);
  await page.keyboard.press('ArrowDown');
  const down = await current();
  await page.keyboard.press('ArrowUp');
  const up = await current();
  check(
    down.selected === names[1] && down.focused === names[1] && up.selected === names[0] && up.focused === names[0],
    `${label}: the arrow keys move between the pages (${JSON.stringify([down, up])})`,
  );
}

/** The stand-in for the desktop app's backend, for a context's
 * `addInitScript`: it holds the settings `snapshot`, tells the main
 * window of each change to them, and answers each other command as
 * `answers` says. `answers` is a function, run on the page, of the
 * stand-in, which sends events with `emit`, and the snapshot; it returns
 * each command's answer, or a function of the command's arguments that
 * gives it. */
export function backend(snapshot, answers = () => ({})) {
  return { content: `(${standIn})(${JSON.stringify(snapshot)}, ${answers});` };
}

/** The stand-in, set on the page before it loads; see `backend`. */
function standIn(snapshot, answers) {
  // This window's label, as the desktop app names its windows.
  const label = location.hash.startsWith('#settings') ? 'settings' : 'main';
  // As in Tauri, a listener for any window hears every event, and one for
  // this window hears those sent to it or to every window.
  const listeners = [];
  const emit = (event, payload, to) =>
    listeners
      .filter((listener) => listener.event === event && (listener.target === null || to === null || listener.target === to))
      .forEach((listener) => listener.handler({ event, payload }));
  const listen = (target) => async (event, handler) => {
    listeners.push({ event, handler, target });
    return () => {};
  };
  const stood = { emit };
  window.standIn = stood;
  const answered = {
    settings_get: snapshot,
    // The backend tells the main window of each change.
    settings_set: ({ settings }) => {
      emit('settings', { ...snapshot, settings: structuredClone(settings) }, 'main');
      return [];
    },
    ...answers(stood, snapshot),
  };
  window.__TAURI__ = {
    core: {
      invoke: async (command, args) => {
        const answer = answered[command];
        return typeof answer === 'function' ? answer(args) : (answer ?? null);
      },
      Channel: class {
        set onmessage(handler) {
          this.handler = handler;
        }
      },
    },
    event: { listen: listen(null) },
    webviewWindow: { getCurrentWebviewWindow: () => ({ label, listen: listen(label) }) },
  };
}
