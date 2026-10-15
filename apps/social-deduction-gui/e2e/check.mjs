// End-to-end check of the built viewer in headless Chromium.
//
// Serves a `dist` that build-page.sh built and opens each game in it: the
// random Avalon and Werewolf games, and the recorded prompted ones. Every
// game at rest has evil bubbles on the left, good on the right and the
// narrator in the center, each bubble labeled `Player N · Role` in its
// role's color, and the winner in the header. The random Avalon board
// agrees with the narrated quests and has a mark or blank per seat in
// every vote row, with no `data-theme`. The random Werewolf board's dead
// are those the narration kills, and `data-theme` matches the last phase.
// The prompted Werewolf game, played at ten times the pace, shows a
// thinking player's dots until it speaks, turns dark by night and light by
// day, and shows late speech faded. The prompted Avalon board names the
// Assassin's target. At 420 px wide the board stacks under the chat and
// nothing scrolls sideways. With a stand-in for the desktop app's backend,
// the main window opens on its start state with Play, #settings opens the
// Settings window's four tabs, and typing goes on in a field while each
// change is stored. The console stays clean throughout.
// Screenshots land in ./screenshots for a human to look at.
//
//   npm test                 # against ../dist, from this directory
//   node check.mjs <dist>    # against another build

import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { extname, join, normalize, resolve } from 'node:path';
import { chromium } from 'playwright';

const dist = resolve(process.argv[2] ?? '../dist');
const shots = resolve('screenshots');
await mkdir(shots, { recursive: true });

// build-page.sh builds for the site's /FreeAgent/viewer/, so the server
// serves `dist` there.
const base = '/FreeAgent/viewer/';
const types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.wasm': 'application/wasm',
  '.css': 'text/css',
  '.json': 'application/json',
};
const server = createServer(async (request, response) => {
  const path = normalize(decodeURIComponent(new URL(request.url, 'http://x').pathname));
  const inside = path.startsWith(base) ? path.slice(base.length) : null;
  const file = join(dist, inside === '' ? 'index.html' : (inside ?? 'missing'));
  try {
    const body = await readFile(file);
    response.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((ready) => server.listen(0, '127.0.0.1', ready));
const origin = `http://127.0.0.1:${server.address().port}${base}`;

let failures = 0;
function check(condition, label) {
  console.log(`${condition ? 'ok  ' : 'FAIL'} ${label}`);
  if (!condition) failures += 1;
}

/** The evil roles, as labels name them; every other role is good. */
const evil = new Set(['Werewolf', 'Assassin', 'Morgana', 'Mordred', 'Oberon', 'Minion']);

/** How long night and day take to fall, with a margin. */
const SETTLE = 800;

/** Opens the game `hash` and waits for it to finish drawing at rest. */
async function open(page, hash) {
  await page.goto(`${origin}#${hash}`);
  await page.waitForSelector('.sd-outcome');
  await page.waitForTimeout(SETTLE);
}

/** Every narrator line in the chat, in order. */
const narrated = (page) =>
  page.$$eval('.rc-message[data-name="Narrator"] .rc-bubble', (bubbles) =>
    bubbles.flatMap((bubble) => bubble.innerText.split('\n').map((line) => line.trim()).filter(Boolean)),
  );

/** Checks what every game shows at rest: sides, labels, colors and the winner. */
async function atRest(page, game) {
  const bubbles = await page.$$eval('.rc-message', (messages) => {
    const pane = document.querySelector('.rc-messages').getBoundingClientRect();
    // A role's color, resolved the way the browser resolves a background.
    const probe = document.createElement('div');
    document.body.append(probe);
    const color = (role) => {
      probe.style.background = `var(--role-${role}-bg)`;
      return getComputedStyle(probe).backgroundColor;
    };
    const drawn = messages
      .filter((message) => message.querySelector(':scope > .rc-bubble'))
      .map((message) => {
        const element = message.querySelector(':scope > .rc-bubble');
        const bubble = element.getBoundingClientRect();
        const name = message.dataset.name;
        const role = name.split(' · ')[1] ?? '';
        return {
          name,
          role,
          sender: message.querySelector('.rc-sender')?.textContent,
          left: bubble.left - pane.left,
          right: pane.right - bubble.right,
          background: getComputedStyle(element).backgroundColor,
          expected: role ? color(role.toLowerCase()) : null,
        };
      });
    probe.remove();
    return drawn;
  });
  const players = bubbles.filter((bubble) => bubble.name !== 'Narrator');
  const narrator = bubbles.filter((bubble) => bubble.name === 'Narrator');
  // Random players never speak, and prompted ones do.
  check(narrator.length > 0, `${game}: the narrator speaks`);
  if (game.endsWith('-prompted')) check(players.length > 0, `${game}: players speak`);
  const evils = players.filter((bubble) => evil.has(bubble.role));
  const goods = players.filter((bubble) => !evil.has(bubble.role));
  check(evils.every((bubble) => bubble.left < bubble.right), `${game}: evil bubbles sit on the left`);
  check(goods.every((bubble) => bubble.right < bubble.left), `${game}: good bubbles sit on the right`);
  check(
    narrator.every((bubble) => Math.abs(bubble.left - bubble.right) <= 2),
    `${game}: the narrator sits in the center`,
  );
  check(
    players.every((bubble) => /^Player \d+ · [A-Z][a-z]+$/.test(bubble.name) && bubble.sender === bubble.name),
    `${game}: every bubble is labeled Player N · Role`,
  );
  const miscolored = players.filter(
    (bubble) => !bubble.expected || bubble.expected === 'rgba(0, 0, 0, 0)' || bubble.background !== bubble.expected,
  );
  check(
    miscolored.length === 0,
    `${game}: every bubble is in its role's color${miscolored.map((bubble) => `; ${bubble.name} is ${bubble.background}, not ${bubble.expected}`).join('')}`,
  );
  const outcome = await page.$eval('.sd-outcome', (element) => ({
    text: element.textContent,
    side: element.classList.contains('sd-good') ? 'good' : element.classList.contains('sd-evil') ? 'evil' : null,
  }));
  const named = /werewolves win|Evil wins/.test(outcome.text)
    ? 'evil'
    : /villagers win|Good wins/.test(outcome.text)
      ? 'good'
      : null;
  check(named !== null && named === outcome.side, `${game}: the header names the winner (${outcome.text})`);
}

/** Screenshots the game as it stands, light and dark, and goes back to light. */
async function screenshots(page, hash) {
  await page.screenshot({ path: join(shots, `${hash}-light.png`), fullPage: true });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForTimeout(SETTLE);
  await page.screenshot({ path: join(shots, `${hash}-dark.png`), fullPage: true });
  await page.emulateMedia({ colorScheme: 'light' });
}

const browser = await chromium.launch();
const context = await browser.newContext({ colorScheme: 'light', viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
const errors = [];
page.on('console', (message) => {
  if (message.type() === 'error') errors.push(message.text());
});
page.on('pageerror', (error) => errors.push(error.message));

/** Runs the checks `checks` of `name`, failing them all on a throw, such as a wait that times out. */
async function section(name, checks) {
  try {
    await checks();
  } catch (error) {
    check(false, `${name}: ${error.message.split('\n')[0]}`);
  }
}

await section('avalon', async () => {
  await open(page, 'avalon');
  await atRest(page, 'avalon');
  const lines = await narrated(page);
  const quests = [];
  for (const line of lines) {
    const result = line.match(/^The quest (succeeds|fails)(?:, with (\d+) sabotages?)?/);
    if (result) quests.push(result[1] === 'succeeds' ? '✓' : result[2]);
  }
  const tokens = await page.$$eval('.sd-quests .sd-token', (tokens) =>
    tokens.map((token) =>
      token.classList.contains('sd-succeeded') || token.classList.contains('sd-failed')
        ? token.querySelector('span').textContent
        : null,
    ),
  );
  const played = tokens.filter((token) => token !== null);
  check(
    // Five rejected proposals can end the game before any quest.
    JSON.stringify(played) === JSON.stringify(quests) && tokens.slice(0, played.length).every((token) => token !== null),
    `avalon: the quest tokens agree with the narrated quests (${quests.join(' ')})`,
  );
  const rows = await page.$$eval('.sd-votes', ([table]) => {
    const seats = table.querySelectorAll('thead th').length - 2;
    return [...table.querySelectorAll('tbody tr')].map((row) => {
      const cells = [...row.querySelectorAll('td')].slice(0, -1);
      return { seats, marks: cells.map((cell) => cell.textContent) };
    });
  });
  check(
    rows.length > 0 && rows.every((row) => row.marks.length === row.seats && row.marks.every((mark) => ['✓', '✗', ''].includes(mark))),
    `avalon: every vote row has one mark or blank per seat (${rows.length} rows)`,
  );
  check(
    !(await page.evaluate(() => document.documentElement.hasAttribute('data-theme'))),
    'avalon: data-theme is unset',
  );
  await screenshots(page, 'avalon');
});

await section('werewolf', async () => {
  await open(page, 'werewolf');
  await atRest(page, 'werewolf');
  const lines = await narrated(page);
  const killed = lines.flatMap((line) => line.match(/^(Player \d+) dies\.$/)?.slice(1) ?? []).sort();
  const dead = (await page.$$eval('.sd-players li.sd-dead .sd-name', (names) => names.map((name) => name.textContent))).sort();
  check(
    killed.length > 0 && JSON.stringify(dead) === JSON.stringify(killed),
    `werewolf: the dead in the panel are those the narration kills (${killed.join(', ')})`,
  );
  const phase = await page.$$eval('.sd-heading', (headings) => headings.at(-1).textContent);
  const theme = await page.evaluate(() => document.documentElement.getAttribute('data-theme'));
  check(
    theme === (phase.startsWith('Night') ? 'dark' : 'light'),
    `werewolf: data-theme is ${theme} after ${phase}`,
  );
  await screenshots(page, 'werewolf');
});

await section('avalon-prompted', async () => {
  await open(page, 'avalon-prompted');
  await atRest(page, 'avalon-prompted');
  const lines = await narrated(page);
  const target = lines.map((line) => line.match(/^The Assassin names (Player \d+)/)?.[1]).find(Boolean);
  const band = await page.$eval('.sd-assassination', (element) => element.textContent).catch(() => '');
  check(
    target !== undefined && band.includes(`names ${target}`),
    `avalon-prompted: the assassination band names the Assassin's target (${band})`,
  );
  await screenshots(page, 'avalon-prompted');
});

// Prompted Werewolf, at rest, then played at ten times the pace while a
// mutation observer keeps a timeline of what the screen shows.
await section('werewolf-prompted', async () => {
  await open(page, 'werewolf-prompted');
  await atRest(page, 'werewolf-prompted');
  await screenshots(page, 'werewolf-prompted');
  await open(page, 'werewolf-prompted-fast');
  await page.evaluate(() => {
    window.timeline = [];
    const snapshot = () => {
      const phase = document.querySelector('.sd-phase');
      const clock = phase?.querySelector('.sd-clock')?.textContent ?? '';
      const messages = [...document.querySelectorAll('.rc-message')];
      window.timeline.push({
        at: performance.now(),
        theme: document.documentElement.getAttribute('data-theme'),
        phase: (phase?.textContent ?? '').replace(clock, '').trim(),
        over: document.querySelector('.sd-outcome') !== null,
        thinking: messages
          .filter((message) => message.dataset.messageId.startsWith('think-') && message.querySelector('.rc-thinking'))
          .map((message) => message.dataset.name),
        spoken: messages
          .filter((message) => !message.dataset.messageId.startsWith('think-') && message.dataset.name !== 'Narrator')
          .map((message) => message.dataset.name),
        late: document.querySelectorAll('.rc-bubble.sd-late').length,
      });
    };
    new MutationObserver(snapshot).observe(document, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: ['data-theme'],
    });
  });
  await page.click('.sd-header button');
  await page.waitForSelector('.rc-thinking');
  await page.screenshot({ path: join(shots, 'werewolf-prompted-thinking.png'), fullPage: true });
  await page.waitForSelector('.sd-outcome', { timeout: 60_000 });
  await page.waitForTimeout(SETTLE);
  const timeline = await page.evaluate(() => window.timeline);
  // From the cleared screen Play starts on.
  const playing = timeline.slice(timeline.findIndex((shot) => shot.spoken.length === 0));
  const count = (shot, name) => shot.spoken.filter((spoken) => spoken === name).length;
  // Each time a player whose dots showed since it last spoke speaks, and
  // whether its dots were gone within half a second.
  const spokeAfterDots = [];
  const lastSpoke = new Map();
  for (let i = 1; i < playing.length; i += 1) {
    for (const name of new Set(playing[i].spoken)) {
      if (count(playing[i], name) <= count(playing[i - 1], name)) continue;
      const since = playing.slice(lastSpoke.get(name) ?? 0, i);
      lastSpoke.set(name, i);
      if (!since.some((shot) => shot.thinking.includes(name))) continue;
      let gone = false;
      for (let j = i; j < playing.length && playing[j].at - playing[i].at < 500 && !gone; j += 1) {
        gone = !playing[j].thinking.includes(name);
      }
      spokeAfterDots.push({ name, gone });
    }
  }
  check(
    spokeAfterDots.length > 0 && spokeAfterDots.every((spoke) => spoke.gone),
    `werewolf-prompted: thinking dots show for a player and are gone after it speaks (${spokeAfterDots.length} times)`,
  );
  // The theme as each phase ends, before the next heading or the outcome.
  const ends = [];
  for (let i = 0; i < playing.length; i += 1) {
    const shot = playing[i];
    const next = playing[i + 1];
    if (!shot.over && /^(Night|Day) \d+\.$/.test(shot.phase) && (!next || next.phase !== shot.phase || next.over)) {
      ends.push(shot);
    }
  }
  check(
    ends.some((shot) => shot.phase.startsWith('Night')) &&
      ends.some((shot) => shot.phase.startsWith('Day')) &&
      ends.every((shot) => shot.theme === (shot.phase.startsWith('Night') ? 'dark' : 'light')),
    `werewolf-prompted: data-theme is dark by night and light by day (${ends.map((shot) => `${shot.phase} ${shot.theme}`).join(', ')})`,
  );
  check(
    timeline.at(-1).late > 0,
    `werewolf-prompted: late speech shows as a .sd-late bubble (${timeline.at(-1).late})`,
  );
});

// A wide window sets the board beside the chat, and a narrow one stacks
// it under.
await section('narrow', async () => {
  const layout = () =>
    page.evaluate(() => {
      const chat = document.querySelector('.sd-chat').getBoundingClientRect();
      const board = document.querySelector('.sd-board').getBoundingClientRect();
      return {
        beside: board.left >= chat.right - 1,
        under: board.top >= chat.bottom - 1,
        wide: document.documentElement.scrollWidth,
        window: document.documentElement.clientWidth,
      };
    });
  await open(page, 'avalon');
  check((await layout()).beside, 'wide: the board sits beside the chat');
  await page.setViewportSize({ width: 420, height: 900 });
  await page.waitForTimeout(SETTLE);
  const narrow = await layout();
  check(narrow.under, 'narrow: the board stacks under the chat');
  check(narrow.wide <= narrow.window, `narrow: nothing scrolls sideways (${narrow.wide} in ${narrow.window})`);
  await page.screenshot({ path: join(shots, 'avalon-narrow.png'), fullPage: true });
});

// The desktop app, its backend stood in for by an init script that
// answers each command with what the app would start with.
const desktop = {
  settings: {
    general: { appearance: 'system', game: 'Werewolf' },
    models: { qwen: { base_url: 'http://localhost:1234/v1', id: 'qwen2.5-7b-instruct-1m' } },
    werewolf: {
      strategy: 'everyone',
      roles: [
        { key: 'werewolf', count: 2 },
        { key: 'villager', count: 3 },
        { key: 'doctor', count: 1 },
        { key: 'seer', count: 1 },
      ],
      phases: [
        { key: 'night_limit', limit: '1m' },
        { key: 'day_limit', limit: '1m' },
        { key: 'ratify_limit', limit: '20s' },
      ],
      strategies: { everyone: { policy: 'prompted', model: 'qwen', system: 'You are {{ name }}, a {{ role }}.' } },
      text: { rules: 'Play well.' },
    },
    avalon: {
      strategy: 'random',
      roles: [
        { key: 'merlin', count: 1 },
        { key: 'servant', count: 2 },
        { key: 'assassin', count: 1 },
        { key: 'minion', count: 1 },
      ],
      phases: [{ key: 'vote_limit', limit: '1m' }],
      strategies: {},
      text: {},
    },
  },
  errors: [],
  logs: '/tmp/games',
};

/** The stand-in backend, set on the page before it loads. */
function backend(snapshot) {
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
  const answers = {
    settings_get: snapshot,
    // The backend tells the main window of each change.
    settings_set: ({ settings }) => {
      emit('settings', { ...snapshot, settings: structuredClone(settings) }, 'main');
      return [];
    },
    providers: [{ name: 'OpenAI', base_url: 'https://api.openai.com/v1', api_key_env: 'OPENAI_API_KEY' }],
    listing: { models: [], error: null, checks: {} },
    key_status: 'Missing',
  };
  window.__TAURI__ = {
    core: {
      invoke: async (command, args) => {
        const answer = answers[command];
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

await section('desktop', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend, desktop);
  const watch = (opened) => {
    opened.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text());
    });
    opened.on('pageerror', (error) => errors.push(error.message));
    return opened;
  };
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.sd-start .sd-play');
  const play = await main.$eval('.sd-start .sd-play', (button) => ({ text: button.textContent.trim(), disabled: button.disabled }));
  check(play.text === 'Play' && !play.disabled, `desktop: the start state shows Play (${JSON.stringify(play)})`);
  const summary = await main.$eval('.sd-summary', (element) => element.textContent);
  check(summary === '7 players · 1 strategy · qwen2.5-7b-instruct-1m', `desktop: the start state sums up the game (${summary})`);
  await main.screenshot({ path: join(shots, 'desktop-start.png'), fullPage: true });
  const settings = watch(await app.newPage());
  await settings.setViewportSize({ width: 760, height: 560 });
  await settings.goto(`${origin}#settings`);
  await settings.waitForSelector('.sd-tabs [role="tab"]');
  const tabs = await settings.$$eval('.sd-tabs [role="tab"]', (buttons) => buttons.map((button) => button.textContent.trim()));
  check(
    JSON.stringify(tabs) === JSON.stringify(['General', 'Models', 'Werewolf', 'Avalon']),
    `desktop: #settings renders the four tabs (${tabs.join(', ')})`,
  );
  await settings.screenshot({ path: join(shots, 'settings-general.png'), fullPage: true });
  for (const [index, name] of [[1, 'models'], [2, 'werewolf']]) {
    await settings.click(`.sd-tabs [role="tab"]:nth-child(${index + 1})`);
    await settings.waitForSelector('.sd-tab-head h2');
    await settings.screenshot({ path: join(shots, `settings-${name}.png`), fullPage: true });
  }
  const players = await settings.$$eval('.sd-stepper input', (inputs) => inputs.map((input) => input.value));
  check(JSON.stringify(players) === JSON.stringify(['2', '3', '1', '1']), `desktop: the Werewolf tab shows each role's count (${players.join(', ')})`);
  // Typing goes on into a new prompt part and a phase limit, each change
  // stored as it is made.
  await settings.fill('.sd-adder-part input', 'tone');
  await settings.click('.sd-adder-part button');
  const part = '[id="field-werewolf.text.tone"]';
  await settings.click(part);
  await settings.keyboard.type('Speak briefly.', { delay: 30 });
  const typed = await settings.$eval(part, (area) => ({ value: area.value, focused: document.activeElement === area }));
  check(typed.value === 'Speak briefly.' && typed.focused, `desktop: typing goes on in a new prompt part (${JSON.stringify(typed)})`);
  const limit = '[id="field-werewolf.phases.night_limit"]';
  await settings.fill(limit, '');
  await settings.click(limit);
  await settings.keyboard.type('45s', { delay: 30 });
  const limited = await settings.$eval(limit, (input) => ({ value: input.value, focused: document.activeElement === input }));
  check(limited.value === '45s' && limited.focused, `desktop: typing goes on in a phase limit (${JSON.stringify(limited)})`);
  await app.close();
});

check(errors.length === 0, `the console stays clean${errors.length ? `: ${errors.join('; ')}` : ''}`);

await browser.close();
server.close();
if (failures > 0) {
  console.log(`${failures} failed`);
  process.exit(1);
}
console.log('all passed');
