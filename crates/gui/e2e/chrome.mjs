// What any Free Agent app's chrome must pass, for an app's screen check
// to import: the server, `check` and `section`, a console watcher, the
// shapes of controls and their focus rings, hover and focus help, a
// vertical tablist the arrows move through, and a stand-in for the
// desktop app's backend with the app's own answers merged in. The
// Settings window's checks hold for any app: its sidebar and General
// page, its Models page against the stand-in's providers, its controls,
// grouped lists and help, its narrow layout, and typing going on while
// each change is stored.
//
// It imports nothing but Node's own modules, so it runs with the
// importing app's Playwright.

import { join } from 'node:path';

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

/** What the stand-in's Models page starts with, for an app to put in its
 * settings: two providers added, one out of credits and one none of whose
 * models play, and a model on the local server. */
export const standInModels = {
  providers: [
    { name: 'Broke', base_url: 'https://api.broke.example/v1', api_key_env: 'BROKE_API_KEY' },
    { name: 'Mute', base_url: 'https://api.mute.example/v1', api_key_env: 'MUTE_API_KEY' },
  ],
  models: { qwen: { base_url: 'http://localhost:1234/v1', id: 'qwen2.5-7b-instruct-1m' } },
};

/** The stand-in for the desktop app's backend, for a context's
 * `addInitScript`: it holds the settings `snapshot`, tells the main
 * window of each change to them, answers what the Models page asks as
 * `providers` says, and answers each other command as `answers` says.
 * `answers` is a function, run on the page, of the stand-in, which sends
 * events with `emit`, and the snapshot; it returns each command's
 * answer, or a function of the command's arguments that gives it. */
export function backend(snapshot, answers = () => ({})) {
  return { content: `(${standIn})(${JSON.stringify(snapshot)}, ${answers}, ${providers});` };
}

/** What the Models page asks the stand-in, run on the page: the providers
 * known and found, how far checking them has got, what each lists, and
 * where each key is. OpenAI can't list its models, Anthropic offers two
 * that play and one that doesn't, Broke's account is out of credits, none
 * of Mute's models play, one local server offers two and another has no
 * model loaded. */
function providers() {
  return {
    providers: [
      { name: 'OpenAI', base_url: 'https://api.openai.com/v1', api_key_env: 'OPENAI_API_KEY' },
      { name: 'Anthropic', base_url: 'https://api.anthropic.com/v1', api_key_env: 'ANTHROPIC_API_KEY' },
      { name: 'Local (localhost:1234)', base_url: 'http://localhost:1234/v1', api_key_env: null },
      { name: 'Local (localhost:11434)', base_url: 'http://localhost:11434/v1', api_key_env: null },
    ],
    checking: [
      {
        base_url: 'https://api.broke.example/v1',
        checked: 1,
        total: 1,
        unchecked: { trouble: 'NoCredits', said: 'You have no credits remaining. Add credits at https://billing.broke.example/credits.' },
      },
    ],
    listing: ({ baseUrl }) => {
      const model = (id, context_window) => ({ id, context_window, hidden: false });
      const listings = {
        'https://api.openai.com/v1': { models: [], error: "Couldn't list models: 401, check the API key", checks: {} },
        'https://api.anthropic.com/v1': {
          models: [model('claude-opus-5-5', 200000), model('claude-sonnet-5-5', 200000), model('claude-mute-1', 200000)],
          error: null,
          checks: { 'claude-opus-5-5': 'Plays', 'claude-sonnet-5-5': 'PlaysUnforced', 'claude-mute-1': { DidNotPlay: 'no tools' } },
        },
        'https://api.broke.example/v1': { models: [model('broke-1', 8192)], error: null, checks: {} },
        'https://api.mute.example/v1': {
          models: [model('mute-1', 8192), model('mute-2', 8192)],
          error: null,
          checks: {
            'mute-1': { DidNotPlay: 'mute-1 did not play Werewolf: did not answer a vote: no tools' },
            'mute-2': { DidNotPlay: 'mute-2 did not play Avalon: did not answer as a voter: no tools' },
          },
        },
        'http://localhost:1234/v1': {
          models: [model('qwen2.5-7b-instruct-1m', 1010000), model('llama-3.2-3b-instruct', 131072)],
          error: null,
          checks: { 'qwen2.5-7b-instruct-1m': 'Plays', 'llama-3.2-3b-instruct': 'Plays' },
        },
      };
      return listings[baseUrl] ?? { models: [], error: null, checks: {} };
    },
    key_status: ({ variable }) => (variable === 'OPENAI_API_KEY' ? 'InEnvironment' : 'InKeychain'),
  };
}

/** The stand-in, set on the page before it loads; see `backend`. */
function standIn(snapshot, answers, providers) {
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
    ...providers(),
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

/** Checks the Settings window `settings`, as `label`, opened on its
 * General page: its sidebar is a vertical tablist of `tabs`, saved to
 * `settings-general.png` in `shots`, General holds the look alone, and
 * its help shows on hover and on focus, saved to `settings-tip.png`. */
export async function settingsWindow(settings, label, { tabs, shots }) {
  await settings.waitForSelector('.fa-sidebar [role="tab"]');
  await tablist(settings, label, '.fa-sidebar', tabs);
  await settings.screenshot({ path: join(shots, 'settings-general.png'), fullPage: true });
  const headings = await settings.$$eval('.fa-settings h2', (elements) => elements.map((element) => element.textContent.trim()));
  check(JSON.stringify(headings) === JSON.stringify(['Appearance']), `${label}: General has the look alone (${headings.join(', ')})`);
  // Help shows under what the pointer is over, and under what has the
  // keyboard's focus.
  await tips(settings, label, {
    hover: { selector: '.fa-segments button:has-text("Light")', text: 'Light, whatever' },
    focus: { selector: '.fa-segments button:has-text("Dark")', text: 'Dark, whatever' },
    press: '#tab-general',
    shot: join(shots, 'settings-tip.png'),
  });
}

/** Checks the Models page of the Settings window `settings`, as `label`,
 * against the stand-in's providers and `standInModels`, saving it to
 * `settings-models.png`, `settings-models-dark.png` and
 * `settings-models-added.png` in `shots`. The models listed are those that
 * play what `plays` names. It leaves the window on the Models page, at
 * 760 by 560. */
export async function models(settings, label, { plays, shots }) {
  await settings.click('#tab-models');
  await settings.waitForSelector('.fa-offered');
  const providers = await settings.$$eval('.fa-provider h2', (names) => names.map((name) => name.textContent.trim()));
  check(
    JSON.stringify(providers) === JSON.stringify(['OpenAI', 'Anthropic', 'Local (localhost:1234)', 'Local (localhost:11434)', 'Broke', 'Mute']),
    `${label}: each provider is shown by its name (${providers.join(', ')})`,
  );
  const card = (name) => `.fa-provider[aria-label="${name}"]`;
  const text = (selector) => settings.$eval(selector, (element) => element.textContent.trim());
  const ids = (selector) => settings.$$eval(selector, (found) => found.map((id) => id.textContent.trim()));
  check(
    (await text(`${card('Local (localhost:11434)')} .fa-warning`)) === 'The server is running, but no model is loaded.',
    `${label}: a local server with no model loaded says so`,
  );
  check((await text(`${card('OpenAI')} .fa-warning`)).includes('401'), `${label}: a provider that cannot list its models says why`);
  const tip = (selector) => settings.$eval(selector, (element) => element.dataset.tip);
  check(
    (await text(`${card('Broke')} .fa-warning`)) ===
      "Broke's account has no credits left. Add credits at billing.broke.example, then reopen Settings." &&
      (await tip(`${card('Broke')} .fa-warning`)).startsWith('You have no credits remaining.') &&
      (await settings.$$(`${card('Broke')} .fa-note`)).length === 0 &&
      (await text('.fa-status')) === "Some models couldn't be checked. Their providers say why.",
    `${label}: a provider whose models couldn't be checked says why, and the provider's words are its help`,
  );
  check(
    (await text(`${card('Mute')} .fa-note`)) === `None of its models plays ${plays}. Most often: no tools.` &&
      (await tip(`${card('Mute')} .fa-note`)) === 'mute-1: no tools\nmute-2: no tools',
    `${label}: a provider none of whose models play gives the most common reason, and each model's in its help`,
  );
  check(
    (await text(`${card('OpenAI')} .fa-chip`)) === 'OPENAI_API_KEY from the environment' &&
      (await text(`${card('Anthropic')} .fa-chip`)) === 'ANTHROPIC_API_KEY in the keychain' &&
      (await text(`${card('Local (localhost:1234)')} .fa-chip`)) === 'No key needed',
    `${label}: each provider says where its key comes from`,
  );
  const offered = await ids(`${card('Anthropic')} .fa-offered .fa-model-id`);
  check(
    JSON.stringify(offered) === JSON.stringify(['claude-opus-5-5', 'claude-sonnet-5-5']),
    `${label}: only models that play are available, in the listing's order (${offered.join(', ')})`,
  );
  const local = await ids(`${card('Local (localhost:1234)')} .fa-model-id`);
  check(
    JSON.stringify(local) === JSON.stringify(['qwen2.5-7b-instruct-1m', 'llama-3.2-3b-instruct']),
    `${label}: a model added is under its provider and not available again (${local.join(', ')})`,
  );
  const untipped = await settings.$$eval('.fa-settings button, .fa-settings input', (all) =>
    all.filter((element) => !element.closest('[data-tip]')).map((element) => element.outerHTML.slice(0, 80)),
  );
  check(untipped.length === 0, `${label}: everything on the Models tab has help (${untipped.join(' ')})`);
  const [chip] = await shapes(settings, `${card('OpenAI')} .fa-chip`);
  check(
    chip.height === 26 && chip.radius === '13px' && chip.background === 'rgb(240, 240, 240)' && chip.weight === '600',
    `${label}: where a key comes from is a gray chip (${JSON.stringify(chip)})`,
  );
  const [add] = await shapes(settings, `${card('Anthropic')} .fa-offered button`);
  check(add.height === 32 && add.radius === '8px', `${label}: a model's Add is a small secondary button (${JSON.stringify(add)})`);
  const flags = await settings.$$eval('.fa-provider .fa-warning', (warnings) =>
    warnings.map((warning) => getComputedStyle(warning, '::before').backgroundColor),
  );
  check(
    flags.length >= 3 && flags.every((flag) => flag === 'rgb(242, 179, 0)'),
    `${label}: every problem on the Models tab is a marigold warning (${flags.join(', ')})`,
  );
  // The pictures are at the size of the design's.
  await settings.setViewportSize({ width: 1040, height: 1100 });
  await settings.keyboard.press('Escape');
  await settings.screenshot({ path: join(shots, 'settings-models.png') });
  await settings.emulateMedia({ colorScheme: 'dark' });
  await settings.screenshot({ path: join(shots, 'settings-models-dark.png') });
  await settings.emulateMedia({ colorScheme: 'light' });
  await settings.setViewportSize({ width: 760, height: 560 });
  // Adding a model asks for its name, filled in with its id.
  await settings.click(`${card('Anthropic')} .fa-offered button`);
  const name = `${card('Anthropic')} .fa-adding input`;
  await settings.waitForSelector(name);
  check((await settings.$eval(name, (input) => input.value)) === 'claude-opus-5-5', `${label}: a model added is first named for its id`);
  await settings.fill(name, 'qwen');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} .fa-field-error`);
  check(
    (await text(`${card('Anthropic')} .fa-field-error`)) === 'Another model already has that name.',
    `${label}: a name another model has is refused`,
  );
  await settings.fill(name, 'opus');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} input.fa-model-name`);
  check(
    (await settings.$eval(`${card('Anthropic')} input.fa-model-name`, (input) => input.value)) === 'opus',
    `${label}: the model is added under the name given`,
  );
  // A name emptied is refused, and the old one is put back.
  const qwen = `${card('Local (localhost:1234)')} input.fa-model-name`;
  await settings.fill(qwen, '');
  await settings.press(qwen, 'Tab');
  check((await settings.$eval(qwen, (input) => input.value)) === 'qwen', `${label}: an empty name is refused`);
  // The context window is the provider's until a number is typed, and
  // Reset puts it back.
  const window = `${card('Local (localhost:1234)')} .fa-context-window input`;
  const shown = () => settings.$eval(window, (input) => [input.value, input.placeholder]);
  check(JSON.stringify(await shown()) === JSON.stringify(['', '1,010,000']), `${label}: an empty context window shows the provider's figure`);
  await settings.fill(window, '32768');
  await settings.press(window, 'Tab');
  await settings.waitForSelector(`${card('Local (localhost:1234)')} .fa-context-window button`);
  check((await shown())[0] === '32,768', `${label}: a context window typed overrides it`);
  await settings.screenshot({ path: join(shots, 'settings-models-added.png'), fullPage: true });
  await settings.click(`${card('Local (localhost:1234)')} .fa-context-window button`);
  check((await shown())[0] === '', `${label}: Reset goes back to the provider's figure`);
}

/** Checks the controls on the page the Settings window `settings` shows,
 * as `label`, one of an app's own with a strategy and a prompt part: a
 * select is a field with a chevron, a text field and a prompt are sunk
 * into the page, a prompt is monospace, a remove button is a square
 * drawn with an icon, and each of the first `rings` controls shows a
 * focus ring. */
export async function settingsControls(settings, label, { rings }) {
  const [select] = await shapes(settings, '.fa-select');
  check(
    select.appearance === 'none' && select.icon && select.height === 36 && select.radius === '8px',
    `${label}: a select is a field with a chevron drawn beside it (${JSON.stringify(select)})`,
  );
  const [field] = await shapes(settings, '.fa-adder input');
  check(field.height === 36 && field.radius === '8px' && field.shadow.includes('inset'), `${label}: a text field is sunk into the page (${JSON.stringify(field)})`);
  const [prompt] = await shapes(settings, '.fa-strategy textarea');
  check(prompt.font.includes('monospace'), `${label}: a prompt is monospace (${prompt.font})`);
  const [remove] = await shapes(settings, '.fa-remove');
  check(
    remove.width === 32 && remove.height === 32 && remove.radius === '8px' && remove.icon && remove.shadow === 'none',
    `${label}: a remove button is a square drawn with an icon (${JSON.stringify(remove)})`,
  );
  const unringed = await ringed(settings, rings);
  check(unringed.length === 0, `${label}: every control on the page shows a focus ring (${unringed.join(' ')})`);
}

/** Checks the page the Settings window `settings` shows, as `label`: a
 * 220 px sidebar beside at least `lists` grouped lists, rows on
 * hairlines, with no box inside a list but its controls; and a prompt
 * part's name is refused unless it is letters, digits, underscores and
 * spaces. */
export async function groupedLists(settings, label, { lists }) {
  const page = await settings.evaluate(() => {
    const controls = 'button, input, select, textarea, .fa-stepper, .fa-segments, .fa-select-box';
    const boxed = (element) => {
      const style = getComputedStyle(element);
      return ['Top', 'Right', 'Bottom', 'Left'].every((side) => style[`border${side}Width`] !== '0px' && style[`border${side}Style`] !== 'none');
    };
    const found = [...document.querySelectorAll('.fa-list')];
    return {
      sidebar: document.querySelector('.fa-sidebar').getBoundingClientRect().width,
      lists: found.length,
      nested: found.flatMap((list) =>
        [...list.querySelectorAll('*')].filter((inner) => !inner.closest(controls) && boxed(inner)).map((inner) => inner.outerHTML.slice(0, 60)),
      ),
    };
  });
  check(page.sidebar === 220 && page.lists >= lists, `${label}: the page is a sidebar beside grouped lists (${JSON.stringify(page)})`);
  check(page.nested.length === 0, `${label}: no grouped list holds another box (${page.nested.join(' ')})`);
  await settings.fill('.fa-adder-part input', 'the-tone');
  await settings.click('.fa-adder-part button');
  const refused = await settings.$eval('.fa-adder-part', (adder) => adder.textContent);
  check(refused.includes('Use letters, digits, underscores and spaces'), `${label}: a prompt part's name is letters, digits, underscores and spaces (${refused})`);
}

/** Checks, as `label`, that everything on each of the `tabs` of the
 * Settings window `settings`, by their ids' ends, has help. */
export async function helped(settings, label, tabs) {
  const unhelped = [];
  for (const tab of tabs) {
    await settings.click(`#tab-${tab}`);
    await settings.waitForTimeout(100);
    unhelped.push(
      ...(await settings.$$eval('.fa-settings button, .fa-settings input, .fa-settings select, .fa-settings textarea, .fa-settings h2, .fa-settings h3', (elements) =>
        elements.filter((element) => !element.closest('[data-tip]:not([data-tip=""])') && !element.querySelector('[data-tip]:not([data-tip=""])')).map((element) => element.outerHTML.slice(0, 80)),
      )),
    );
  }
  check(unhelped.length === 0, `${label}: everything on ${tabs.join(', ')} has help${unhelped.map((element) => `; ${element}`).join('')}`);
}

/** Checks, as `label`, that the Settings window `settings`, narrowed to
 * 520 px, stacks the sidebar above the page and scrolls only down, saved
 * to `shot`, then widens it back to 760 px. */
export async function narrowSettings(settings, label, shot) {
  await settings.setViewportSize({ width: 520, height: 700 });
  await settings.waitForTimeout(100);
  const narrow = await settings.evaluate(() => {
    const box = (selector) => document.querySelector(selector).getBoundingClientRect();
    const pane = document.scrollingElement;
    return {
      stacked: box('.fa-page').top >= box('.fa-sidebar').bottom,
      wide: Math.max(pane.scrollWidth, document.querySelector('.fa-settings').scrollWidth),
      window: pane.clientWidth,
    };
  });
  check(narrow.stacked, `${label}: a narrow Settings window stacks the sidebar above the page`);
  check(narrow.wide <= narrow.window, `${label}: a narrow Settings window scrolls only down (${narrow.wide} in ${narrow.window})`);
  await settings.screenshot({ path: shot, fullPage: true });
  await settings.setViewportSize({ width: 760, height: 560 });
}

/** Checks, as `label`, that typing `text` goes on in the field `selector`
 * of the Settings window `settings`, emptied first when `empty`, while
 * each change is stored: it keeps the focus and every letter. `what` says
 * what the field is. */
export async function typing(settings, label, { selector, text, what, empty = false }) {
  if (empty) await settings.fill(selector, '');
  await settings.click(selector);
  await settings.keyboard.type(text, { delay: 30 });
  const typed = await settings.$eval(selector, (field) => ({ value: field.value, focused: document.activeElement === field }));
  check(typed.value === text && typed.focused, `${label}: typing goes on in ${what} (${JSON.stringify(typed)})`);
}
