// End-to-end check of the built viewer in headless Chromium.
//
// Serves a `dist` that build-page.sh built and opens each game in it: the
// random Avalon and Werewolf games, and the recorded prompted ones. Every
// game at rest has evil bubbles on the left, good on the right and the
// narrator in the center, each bubble labeled `Player N · Role` and
// unfilled, the line over it holding the seat square on the side away from
// the speaker's edge, the player in their side's text color, light or
// dark, and the role in muted italics, every player the board names marked by a seat square numbered
// from the seat in its side's color, and the winner in the header. The
// random Avalon board's quest squares agree with the narrated quests, and
// it has a mark or blank per seat in every vote row, with no `data-theme`.
// The random Werewolf board's dead are those the narration kills, struck
// through, and `data-theme` matches the last phase. The prompted Werewolf
// game, played at ten times the pace, shows a thinking player's dots until
// it speaks, turns dark by night and light by day, inverts a vote's row
// and tags it New as it lands, and shows a day closing, with speech
// landing while it does, before the night falls. The prompted Avalon board names the Assassin's target.
// At 420 px wide the board stacks under the chat and nothing scrolls
// sideways. The tab shows the mark. With a stand-in for the desktop app's backend,
// the main window opens on its start state: the mark, then one row per game
// with what it is, its seats, its summary and its own Play, which a
// settings error disables, linked to its field. #settings opens the
// Settings window's four pages in a vertical tablist the arrows move
// through, each page a few grouped lists with no box inside them and what
// each role and phase does written under its name, help shows on hover and
// on focus, and typing goes on in a field while each change is stored. Its Models tab
// shows each provider by name with where its key comes from, why it lists
// nothing, or the models that play; adding one asks for its name, and a
// name taken or empty is refused; a context window typed overrides the
// provider's, and Reset puts it back; and everything on it has help. Its
// controls are ink only, each with a focus ring: Play a large primary
// button, the others secondary, quiet or icon buttons, a segmented control
// raised out of a sunken track, a joined stepper, fields and selects sunk
// into the page, and chips in gray. The console stays clean throughout.
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
  '.svg': 'image/svg+xml',
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
    return messages
      .filter((message) => message.querySelector(':scope > .rc-bubble'))
      .map((message) => {
        const element = message.querySelector(':scope > .rc-bubble');
        const bubble = element.getBoundingClientRect();
        const name = message.dataset.name;
        const role = name.split(' · ')[1] ?? '';
        return {
          name,
          role,
          // What a screen reader reads, decoration left out.
          sender: (() => {
            const line = message.querySelector('.rc-sender')?.cloneNode(true);
            line?.querySelectorAll('[aria-hidden="true"]').forEach((mark) => mark.remove());
            return line?.textContent;
          })(),
          left: bubble.left - pane.left,
          right: pane.right - bubble.right,
          background: getComputedStyle(element).backgroundColor,
          border: getComputedStyle(element).borderTopWidth,
        };
      });
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
    players.every((bubble) => /^Player \d+ · [A-Z][a-z]+$/.test(bubble.name) && bubble.sender === bubble.name.replace(' · ', ', ')),
    `${game}: every bubble is labeled Player N · Role and its line reads Player N, Role`,
  );
  await senderLines(page, game);
  const filled = players.filter((bubble) => bubble.background !== 'rgba(0, 0, 0, 0)' || bubble.border !== '1px');
  check(
    filled.length === 0,
    `${game}: every player's bubble is unfilled and outlined${filled.map((bubble) => `; ${bubble.name} is ${bubble.background} with ${bubble.border}`).join('')}`,
  );
  const { sides, squares } = await page.$$eval('.sd-players li', (rows) => {
    // A side's color, resolved the way the browser resolves a background.
    const probe = document.createElement('div');
    document.body.append(probe);
    const color = (side) => {
      probe.style.background = `var(--sd-${side})`;
      return getComputedStyle(probe).backgroundColor;
    };
    const sides = { good: color('good'), evil: color('evil') };
    probe.remove();
    return {
      sides,
      squares: rows.map((row) => {
        const square = row.querySelector('.sd-seat');
        const style = getComputedStyle(square);
        return {
          role: row.querySelector('.sd-detail').textContent.split(',')[0],
          name: row.querySelector('.sd-name').textContent,
          number: square.textContent,
          background: style.backgroundColor,
          size: `${style.width}×${style.height}`,
          round: style.borderRadius,
        };
      }),
    };
  });
  const capitalized = (role) => role.charAt(0).toUpperCase() + role.slice(1);
  const offside = squares.filter(
    (square) =>
      square.background !== sides[evil.has(capitalized(square.role)) ? 'evil' : 'good'] ||
      square.name !== `Player ${square.number}` ||
      square.size !== '22px×22px' ||
      square.round !== '0px',
  );
  check(
    squares.length > 0 && sides.good !== sides.evil && offside.length === 0,
    `${game}: every seat's square is numbered from its seat in its side's color${offside.map((square) => `; ${JSON.stringify(square)}`).join('')}`,
  );
  const unmarked = await page.$$eval('.sd-board .sd-name', (names) =>
    names.filter((name) => !name.previousElementSibling?.matches('.sd-seat')).map((name) => name.textContent),
  );
  check(
    unmarked.length === 0 && (await page.$$('.sd-swatch')).length === 0,
    `${game}: every player the board names has a seat square, and no swatch remains${unmarked.map((name) => `; ${name}`).join('')}`,
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

/** Checks the line over every player's bubble: a decorative seat square
 * in the side's fill, before the player for evil and after the role for
 * good, the player in the side's text color, and the role in muted italics. */
async function senderLines(page, label) {
  const wrong = await page.$$eval(
    '.rc-message:not([data-name="Narrator"]) > .rc-sender',
    (lines, evil) => {
      // A custom property's color, resolved the way the browser resolves one.
      const probe = document.createElement('span');
      document.body.append(probe);
      const color = (name) => {
        probe.style.color = `var(${name})`;
        return getComputedStyle(probe).color;
      };
      const muted = color('--sd-muted');
      const sides = Object.fromEntries(
        ['good', 'evil'].map((side) => [side, { text: color(`--sd-${side}-text`), fill: color(`--sd-${side}`) }]),
      );
      probe.remove();
      return lines.flatMap((line) => {
        const [player, role] = line.parentElement.dataset.name.split(' · ');
        const side = evil.includes(role) ? 'evil' : 'good';
        const parts = [...line.children].filter((child) => !child.matches('.sd-unseen'));
        const square = side === 'evil' ? parts[0] : parts.at(-1);
        const [name, title] = side === 'evil' ? parts.slice(1) : parts;
        const shown = {
          parts: parts.length,
          square: square?.matches('.sd-seat') && square.getAttribute('aria-hidden') === 'true' ? square.textContent : null,
          fill: square && getComputedStyle(square).backgroundColor,
          player: name?.textContent,
          color: name && getComputedStyle(name).color,
          weight: name && getComputedStyle(name).fontWeight,
          role: title?.textContent,
          italic: title && getComputedStyle(title).fontStyle,
          muted: title && getComputedStyle(title).color,
        };
        const right =
          shown.parts === 3 &&
          shown.square === player.replace('Player ', '') &&
          shown.fill === sides[side].fill &&
          shown.player === player &&
          shown.color === sides[side].text &&
          shown.weight === '700' &&
          shown.role === role &&
          shown.italic === 'italic' &&
          shown.muted === muted;
        return right ? [] : [`${player} (${side}): ${JSON.stringify(shown)}`];
      });
    },
    [...evil],
  );
  check(
    wrong.length === 0,
    `${label}: every player's line has their seat square, before the name if evil and after the role if good, the player in their side's color and the role in muted italics${wrong.map((line) => `; ${line}`).join('')}`,
  );
}

/** Screenshots the game as it stands, light and dark, and goes back to light. */
async function screenshots(page, hash) {
  await page.screenshot({ path: join(shots, `${hash}-light.png`), fullPage: true });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForTimeout(SETTLE);
  await senderLines(page, `${hash} dark`);
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
  const tokens = await page.$$eval('.sd-quests .sd-quest', (quests) =>
    quests.map((quest) =>
      quest.classList.contains('sd-succeeded') || quest.classList.contains('sd-failed')
        ? quest.querySelector('.sd-token').textContent
        : null,
    ),
  );
  const played = tokens.filter((token) => token !== null);
  check(
    // Five rejected proposals can end the game before any quest.
    JSON.stringify(played) === JSON.stringify(quests) && tokens.slice(0, played.length).every((token) => token !== null),
    `avalon: the quest squares agree with the narrated quests (${quests.join(' ')})`,
  );
  const rows = await page.$$eval('.sd-votes', ([table]) => {
    const seats = table.querySelectorAll('thead th').length - 2;
    return [...table.querySelectorAll('tbody tr')].map((row) => {
      const cells = [...row.querySelectorAll('td')].slice(0, -1);
      return { seats, marks: cells.map((cell) => cell.textContent) };
    });
  });
  check(
    rows.length > 0 && rows.every((row) => row.marks.length === row.seats && row.marks.every((mark) => ['✓', '×', ''].includes(mark))),
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
  const dead = await page.$$eval('.sd-players li.sd-dead', (rows) =>
    rows.map((row) => ({
      name: row.querySelector('.sd-name').textContent,
      struck: getComputedStyle(row.querySelector('.sd-name')).textDecorationLine,
      faded: getComputedStyle(row.querySelector('.sd-seat.sd-out')).opacity,
    })),
  );
  const names = dead.map((row) => row.name).sort();
  check(
    killed.length > 0 && JSON.stringify(names) === JSON.stringify(killed),
    `werewolf: the dead in the panel are those the narration kills (${killed.join(', ')})`,
  );
  check(
    dead.every((row) => row.struck === 'line-through' && row.faded === '0.35'),
    `werewolf: the dead are struck through and their squares faded (${JSON.stringify(dead)})`,
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
    // Ink and paper as they stand, which a landing row takes inverted.
    const probe = document.createElement('div');
    probe.style.cssText = 'color: var(--sd-paper); background: var(--sd-ink)';
    document.body.append(probe);
    const inverted = getComputedStyle(probe);
    const snapshot = () => {
      const phase = document.querySelector('.sd-phase');
      const clock = phase?.querySelector('.sd-clock')?.textContent ?? '';
      const closing = phase?.querySelector('.sd-closing')?.textContent ?? '';
      const messages = [...document.querySelectorAll('.rc-message')];
      window.timeline.push({
        at: performance.now(),
        theme: document.documentElement.getAttribute('data-theme'),
        phase: (phase?.textContent ?? '').replace(clock, '').replace(closing, '').trim(),
        closing,
        over: document.querySelector('.sd-outcome') !== null,
        landed: [...document.querySelectorAll('.sd-vote.sd-landed')].map((row) => {
          const style = getComputedStyle(row);
          return {
            inverted: style.backgroundColor === inverted.backgroundColor && style.color === inverted.color,
            tag: getComputedStyle(row, '::after').content,
          };
        }),
        thinking: messages
          .filter((message) => message.dataset.messageId.startsWith('think-') && message.querySelector('.rc-thinking'))
          .map((message) => message.dataset.name),
        spoken: messages
          .filter((message) => !message.dataset.messageId.startsWith('think-') && message.dataset.name !== 'Narrator')
          .map((message) => message.dataset.name),
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
  const landed = playing.flatMap((shot) => shot.landed);
  check(
    landed.length > 0 && landed.every((row) => row.inverted && row.tag === '"New"'),
    `werewolf-prompted: a landed vote's row is inverted and tagged New while it flashes (${landed.length} seen)`,
  );
  // The theme as each phase ends, before the next heading or the outcome.
  const ends = [];
  for (let i = 0; i < playing.length; i += 1) {
    const shot = playing[i];
    const next = playing[i + 1];
    if (!shot.over && /^(Night|Day) \d+$/.test(shot.phase) && (!next || next.phase !== shot.phase || next.over)) {
      ends.push(shot);
    }
  }
  check(
    ends.some((shot) => shot.phase.startsWith('Night')) &&
      ends.some((shot) => shot.phase.startsWith('Day')) &&
      ends.every((shot) => shot.theme === (shot.phase.startsWith('Night') ? 'dark' : 'light')),
    `werewolf-prompted: data-theme is dark by night and light by day (${ends.map((shot) => `${shot.phase} ${shot.theme}`).join(', ')})`,
  );
  // Each day that closes and then gives way to the night, and whether a
  // player spoke while it closed.
  const closings = [];
  for (let i = 1; i < playing.length; i += 1) {
    const shot = playing[i];
    if (!shot.closing || !shot.phase.startsWith('Day')) continue;
    const before = playing[i - 1];
    if (before.closing && before.phase === shot.phase) continue;
    let j = i;
    while (j + 1 < playing.length && playing[j + 1].closing && playing[j + 1].phase === shot.phase) j += 1;
    const next = playing[j + 1];
    if (!next || next.over || !next.phase.startsWith('Night')) continue;
    closings.push({ phase: shot.phase, spoke: playing[j].spoken.length > before.spoken.length });
  }
  check(
    closings.some((closing) => closing.spoke),
    `werewolf-prompted: a day shows Closing… until the night, and speech lands while it does (${closings.map((closing) => `${closing.phase} ${closing.spoke ? 'spoke' : 'silent'}`).join(', ')})`,
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

await section('favicon', async () => {
  const link = await page.$eval('link[rel="icon"]', (icon) => ({ href: icon.href, type: icon.type }));
  const served = await page.request.get(link.href);
  const mark = await served.text();
  check(
    served.ok() && link.type === 'image/svg+xml' && mark.includes('viewBox="6 20 76 56"'),
    `favicon: the tab shows the mark (${link.href})`,
  );
});

// The desktop app, its backend stood in for by an init script that
// answers each command with what the app would start with.
const desktop = {
  settings: {
    general: { appearance: 'system', game: 'Werewolf' },
    providers: [
      { name: 'Broke', base_url: 'https://api.broke.example/v1', api_key_env: 'BROKE_API_KEY' },
      { name: 'Mute', base_url: 'https://api.mute.example/v1', api_key_env: 'MUTE_API_KEY' },
    ],
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
      strategies: { everyone: { policy: 'prompted', model: 'qwen', system: '{{ rules }}\nYou are {{ name }}, a {{ role }}.' } },
      prompt_parts: { rules: 'Play well.' },
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
      prompt_parts: {},
    },
  },
  errors: [],
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

/** The shape of each element `selector` matches on `page`: its box and
 * the computed styles the controls are drawn with. */
function shapes(page, selector) {
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

/** The sides' colors, which no control on `page` uses. */
async function sideColored(page) {
  return page.$$eval('button, input, select, textarea, .sd-chip', (controls) => {
    const sides = ['--sd-good', '--sd-good-text', '--sd-evil', '--sd-evil-text'].map((name) => {
      const probe = document.createElement('span');
      probe.style.color = `var(${name})`;
      document.body.append(probe);
      const color = getComputedStyle(probe).color;
      probe.remove();
      return color;
    });
    return controls
      .filter((control) => !control.closest('.rc-chat'))
      .filter((control) => {
        const style = getComputedStyle(control);
        return [style.color, style.backgroundColor, style.borderTopColor].some((color) => sides.includes(color));
      })
      .map((control) => control.outerHTML.slice(0, 80));
  });
}

/** What has the focus on `page` after each of `count` presses of Tab,
 * and whether it shows the ink focus ring. */
async function ringed(page, count) {
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

/** Check the Models tab of the Settings window `settings`, then go back
 * to the Werewolf tab. */
async function models(settings) {
  await settings.click('#tab-models');
  await settings.waitForSelector('.sd-offered');
  const providers = await settings.$$eval('.sd-provider h4', (names) => names.map((name) => name.textContent.trim()));
  check(
    JSON.stringify(providers) === JSON.stringify(['OpenAI', 'Anthropic', 'Local (localhost:1234)', 'Local (localhost:11434)', 'Broke', 'Mute']),
    `desktop: each provider is shown by its name (${providers.join(', ')})`,
  );
  const card = (name) => `.sd-provider[aria-label="${name}"]`;
  const text = (selector) => settings.$eval(selector, (element) => element.textContent.trim());
  const ids = (selector) => settings.$$eval(selector, (found) => found.map((id) => id.textContent.trim()));
  check(
    (await text(`${card('Local (localhost:11434)')} .sd-note`)) === 'The server is running, but no model is loaded.',
    'desktop: a local server with no model loaded says so',
  );
  check((await text(`${card('OpenAI')} .sd-field-error`)).includes('401'), 'desktop: a provider that cannot list its models says why');
  const tip = (selector) => settings.$eval(selector, (element) => element.dataset.tip);
  check(
    (await text(`${card('Broke')} .sd-field-error`)) ===
      "Broke's account has no credits left. Add credits at billing.broke.example, then reopen Settings." &&
      (await tip(`${card('Broke')} .sd-field-error`)).startsWith('You have no credits remaining.') &&
      (await settings.$$(`${card('Broke')} .sd-note`)).length === 0 &&
      (await text('.sd-status')) === "Some models couldn't be checked. Their providers say why.",
    "desktop: a provider whose models couldn't be checked says why, and the provider's words are its help",
  );
  check(
    (await text(`${card('Mute')} .sd-note`)) === 'None of its models plays both games. Most often: no tools.' &&
      (await tip(`${card('Mute')} .sd-note`)) === 'mute-1: no tools\nmute-2: no tools',
    "desktop: a provider none of whose models play gives the most common reason, and each model's in its help",
  );
  check(
    (await text(`${card('OpenAI')} .sd-chip`)) === 'OPENAI_API_KEY from the environment' &&
      (await text(`${card('Anthropic')} .sd-chip`)) === 'ANTHROPIC_API_KEY in the keychain' &&
      (await text(`${card('Local (localhost:1234)')} .sd-chip`)) === 'No key needed',
    'desktop: each provider says where its key comes from',
  );
  const offered = await ids(`${card('Anthropic')} .sd-offered .sd-id`);
  check(
    JSON.stringify(offered) === JSON.stringify(['claude-opus-5-5', 'claude-sonnet-5-5']),
    `desktop: only models that play are available, in the listing's order (${offered.join(', ')})`,
  );
  const local = await ids(`${card('Local (localhost:1234)')} .sd-id`);
  check(
    JSON.stringify(local) === JSON.stringify(['qwen2.5-7b-instruct-1m', 'llama-3.2-3b-instruct']),
    `desktop: a model added is under its provider and not available again (${local.join(', ')})`,
  );
  const untipped = await settings.$$eval('.sd-settings button, .sd-settings input', (all) =>
    all.filter((element) => !element.closest('[data-tip]')).map((element) => element.outerHTML.slice(0, 80)),
  );
  check(untipped.length === 0, `desktop: everything on the Models tab has help (${untipped.join(' ')})`);
  const [chip] = await shapes(settings, `${card('OpenAI')} .sd-chip`);
  check(
    chip.height === 26 && chip.radius === '13px' && chip.background === 'rgb(240, 240, 240)' && chip.weight === '600',
    `desktop: where a key comes from is a gray chip (${JSON.stringify(chip)})`,
  );
  const [add] = await shapes(settings, `${card('Anthropic')} .sd-offered button`);
  check(add.height === 32 && add.radius === '8px', `desktop: a model's Add is a small secondary button (${JSON.stringify(add)})`);
  const sided = await sideColored(settings);
  check(sided.length === 0, `desktop: no control on the Models tab is red or blue (${sided.join(' ')})`);
  await settings.screenshot({ path: join(shots, 'settings-models.png'), fullPage: true });
  // Adding a model asks for its name, filled in with its id.
  await settings.click(`${card('Anthropic')} .sd-offered button`);
  const name = `${card('Anthropic')} .sd-adding input`;
  await settings.waitForSelector(name);
  check((await settings.$eval(name, (input) => input.value)) === 'claude-opus-5-5', 'desktop: a model added is first named for its id');
  await settings.fill(name, 'qwen');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} .sd-field-error`);
  check(
    (await text(`${card('Anthropic')} .sd-field-error`)) === 'Another model already has that name.',
    'desktop: a name another model has is refused',
  );
  await settings.fill(name, 'opus');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} input.sd-name`);
  check((await settings.$eval(`${card('Anthropic')} input.sd-name`, (input) => input.value)) === 'opus', 'desktop: the model is added under the name given');
  // A name emptied is refused, and the old one is put back.
  const qwen = `${card('Local (localhost:1234)')} input.sd-name`;
  await settings.fill(qwen, '');
  await settings.press(qwen, 'Tab');
  check((await settings.$eval(qwen, (input) => input.value)) === 'qwen', 'desktop: an empty name is refused');
  // The context window is the provider's until a number is typed, and
  // Reset puts it back.
  const window = `${card('Local (localhost:1234)')} .sd-window input`;
  const shown = () => settings.$eval(window, (input) => [input.value, input.placeholder]);
  check(JSON.stringify(await shown()) === JSON.stringify(['', '1,010,000']), "desktop: an empty context window shows the provider's figure");
  await settings.fill(window, '32768');
  await settings.press(window, 'Tab');
  await settings.waitForSelector(`${card('Local (localhost:1234)')} .sd-window button`);
  check((await shown())[0] === '32,768', 'desktop: a context window typed overrides it');
  await settings.screenshot({ path: join(shots, 'settings-models-added.png'), fullPage: true });
  await settings.click(`${card('Local (localhost:1234)')} .sd-window button`);
  check((await shown())[0] === '', 'desktop: Reset goes back to the provider\'s figure');
  await settings.click('#tab-werewolf');
  await settings.waitForSelector('.sd-stepper input');
}

/** `opened`, its console errors and page errors recorded. */
function watch(opened) {
  opened.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  opened.on('pageerror', (error) => errors.push(error.message));
  return opened;
}

await section('desktop', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend, desktop);
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.sd-start .sd-game-summary:not(:empty)');
  const games = await main.$$eval('.sd-game-list > .sd-game', (rows) =>
    rows.map((row) => ({
      name: row.querySelector('h2').textContent,
      described: row.querySelector('.sd-description').textContent.length > 0,
      seats: [...row.querySelectorAll('.sd-game-seats > span')].map((seat) => seat.className).join(' '),
      summary: row.querySelector('.sd-game-summary').textContent,
      play: row.querySelector('.sd-play').textContent.trim(),
      enabled: !row.querySelector('.sd-play').disabled,
      warned: row.querySelector('.sd-warning') !== null,
    })),
  );
  check(
    JSON.stringify(games) ===
      JSON.stringify([
        {
          name: 'Werewolf',
          described: true,
          seats: 'sd-evil sd-evil sd-good sd-good sd-good sd-good sd-good',
          summary: '7 players · 1 strategy · qwen2.5-7b-instruct-1m',
          play: 'Play Werewolf',
          enabled: true,
          warned: false,
        },
        { name: 'Avalon', described: true, seats: 'sd-evil sd-evil sd-good sd-good sd-good', summary: '5 players · 1 strategy', play: 'Play Avalon', enabled: true, warned: false },
      ]),
    `desktop: the start state lists each game with what it is, its seats, its summary and its own Play (${JSON.stringify(games)})`,
  );
  check((await main.$$('.sd-start .sd-segments')).length === 0, 'desktop: the start state has no game chooser');
  const look = await main.evaluate(() => {
    const style = (selector) => getComputedStyle(document.querySelector(selector));
    return {
      title: [style('.sd-start-head h1').fontSize, style('.sd-start-head h1').fontWeight],
      tagline: style('.sd-tagline').fontStyle,
      name: style('.sd-game h2').fontSize,
      settings: document.querySelector('.sd-open-settings').textContent,
    };
  });
  check(
    JSON.stringify(look) === JSON.stringify({ title: ['22px', '700'], tagline: 'italic', name: '44px', settings: look.settings }) &&
      /^Settings (⌘|Ctrl\+),$/.test(look.settings),
    `desktop: the start state is drawn as designed (${JSON.stringify(look)})`,
  );
  const squares = await shapes(main, '.sd-game-seats > span');
  const colors = new Set(squares.map((square) => square.background));
  check(
    squares.every((square) => square.width === 14 && square.height === 14) && colors.size === 2 && colors.has('rgb(210, 0, 31)') && colors.has('rgb(36, 56, 232)'),
    `desktop: each seat is a 14px square, red or blue by side (${JSON.stringify([...colors])})`,
  );
  const logo = await main.$eval('.sd-logo svg', (svg) => ({
    tiles: svg.querySelectorAll('rect:not(mask rect)').length,
    evil: getComputedStyle(svg.querySelector(':scope > path')).fill,
    good: getComputedStyle(svg.querySelector(':scope > use')).fill,
    gap: getComputedStyle(svg.querySelector(':scope > path')).mask,
  }));
  check(
    logo.tiles === 0 && logo.evil === 'rgb(210, 0, 31)' && logo.good === 'rgb(36, 56, 232)' && logo.gap !== 'none',
    `desktop: the start state shows the red and blue balloons, a gap cut between them (${JSON.stringify(logo)})`,
  );
  await main.screenshot({ path: join(shots, 'desktop-start.png'), fullPage: true });
  const [big] = await shapes(main, '.sd-start .sd-play');
  check(
    big.height === 48 && big.radius === '12px' && big.background === 'rgb(17, 17, 17)' && big.color === 'rgb(255, 255, 255)' && big.shadow.includes('inset'),
    `desktop: Play is a large primary button, filled with ink and lifted (${JSON.stringify(big)})`,
  );
  const startSided = await sideColored(main);
  check(startSided.length === 0, `desktop: no control on the start state is red or blue (${startSided.join(' ')})`);
  const startUnringed = await ringed(main, 3);
  check(startUnringed.length === 0, `desktop: every control on the start state shows a focus ring (${startUnringed.join(' ')})`);
  await main.hover('.sd-play');
  const played = await main.$eval('.sd-tip', (element) => !element.hidden && element.textContent);
  check(typeof played === 'string' && played.startsWith('Start a game of Werewolf'), `desktop: Play has help (${played})`);
  const settings = watch(await app.newPage());
  await settings.setViewportSize({ width: 760, height: 560 });
  await settings.goto(`${origin}#settings`);
  await settings.waitForSelector('.sd-sidebar [role="tab"]');
  const tabs = await settings.$$eval('.sd-sidebar[role="tablist"][aria-orientation="vertical"] [role="tab"]', (buttons) =>
    buttons.map((button) => button.textContent.trim()),
  );
  check(
    JSON.stringify(tabs) === JSON.stringify(['General', 'Models', 'Werewolf', 'Avalon']),
    `desktop: #settings renders the four pages as a vertical tablist (${tabs.join(', ')})`,
  );
  // The arrows move between the pages, and the focus goes with them.
  const current = () =>
    settings.evaluate(() => ({
      selected: document.querySelector('[role="tab"][aria-selected="true"]').textContent,
      focused: document.activeElement.textContent,
    }));
  await settings.focus('#tab-general');
  await settings.keyboard.press('ArrowDown');
  const down = await current();
  await settings.keyboard.press('ArrowUp');
  const up = await current();
  check(
    down.selected === 'Models' && down.focused === 'Models' && up.selected === 'General' && up.focused === 'General',
    `desktop: the arrow keys move between the pages (${JSON.stringify([down, up])})`,
  );
  await settings.screenshot({ path: join(shots, 'settings-general.png'), fullPage: true });
  const headings = await settings.$$eval('.sd-settings h2', (elements) => elements.map((element) => element.textContent.trim()));
  check(
    JSON.stringify(headings) === JSON.stringify(['Appearance']),
    `desktop: General has the look alone (${headings.join(', ')})`,
  );
  // Help shows under what the pointer is over, and under what has the
  // keyboard's focus.
  const tip = () =>
    settings.$eval('.sd-tip', (element) => ({ text: element.textContent, shown: !element.hidden, top: element.getBoundingClientRect().top }));
  await settings.hover('.sd-segments button:has-text("Light")');
  const hovered = await tip();
  check(hovered.shown && hovered.text.startsWith('Always light'), `desktop: hovering shows help (${JSON.stringify(hovered)})`);
  await settings.mouse.move(0, 0);
  await settings.focus('.sd-segments button:has-text("Dark")');
  const focused = await tip();
  check(focused.shown && focused.text.startsWith('Always dark'), `desktop: focus shows help (${JSON.stringify(focused)})`);
  await settings.screenshot({ path: join(shots, 'settings-tip.png') });
  await settings.keyboard.press('Escape');
  check(!(await tip()).shown, 'desktop: Escape hides help');
  await settings.click('#tab-werewolf');
  await settings.waitForSelector('.sd-page-head h1');
  await settings.screenshot({ path: join(shots, 'settings-werewolf.png'), fullPage: true });
  await models(settings);
  const players = await settings.$$eval('.sd-stepper input', (inputs) => inputs.map((input) => input.value));
  check(JSON.stringify(players) === JSON.stringify(['2', '3', '1', '1']), `desktop: the Werewolf tab shows each role's count (${players.join(', ')})`);
  const notes = await settings.$$eval('.sd-role, .sd-limit', (rows) =>
    rows.map((row) => {
      const note = row.querySelector('.sd-note');
      return `${row.querySelector('.sd-item-name').textContent}: ${note.checkVisibility() ? note.textContent : ''}`;
    }),
  );
  check(
    notes[0] === 'Werewolf: Knows the other werewolves. Each night they kill a villager.' &&
      notes[4] === 'Night: How long the werewolves, the doctor and the seer have to choose.' &&
      notes.every((note) => !note.endsWith(': ')),
    `desktop: what each role and phase does is written under its name (${notes.join('; ')})`,
  );
  const [restore] = await shapes(settings, '.sd-page-head > button');
  check(
    restore.text === 'Restore defaults…' && restore.height === 40 && restore.radius === '10px' && restore.border === '1px rgb(196, 196, 196)' &&
      restore.background === 'rgb(255, 255, 255)' && restore.shadow !== 'none',
    `desktop: Restore defaults… is a secondary button, white with a hairline (${JSON.stringify(restore)})`,
  );
  const [stepper] = await shapes(settings, '.sd-stepper');
  const [fewer, count] = await shapes(settings, '.sd-stepper > *');
  check(
    stepper.height === 36 && stepper.radius === '10px' && stepper.border === '1px rgb(196, 196, 196)' && fewer.width === 36 && count.width === 40,
    `desktop: a stepper is one joined control (${JSON.stringify([stepper, fewer, count])})`,
  );
  const [select] = await shapes(settings, '.sd-select');
  check(
    select.appearance === 'none' && select.icon && select.height === 36 && select.radius === '8px',
    `desktop: a select is a field with a chevron drawn beside it (${JSON.stringify(select)})`,
  );
  const [field] = await shapes(settings, '.sd-adder-strategy input');
  check(field.height === 36 && field.radius === '8px' && field.shadow.includes('inset'), `desktop: a text field is sunk into the page (${JSON.stringify(field)})`);
  const [prompt] = await shapes(settings, '.sd-strategy textarea');
  check(prompt.font.includes('monospace'), `desktop: a prompt is monospace (${prompt.font})`);
  const [remove] = await shapes(settings, '.sd-remove');
  check(
    remove.width === 32 && remove.height === 32 && remove.radius === '8px' && remove.icon && remove.shadow === 'none',
    `desktop: a remove button is a square drawn with an icon (${JSON.stringify(remove)})`,
  );
  const [quiet] = await shapes(settings, '.sd-assigned .sd-link');
  check(
    quiet.background === 'rgba(0, 0, 0, 0)' && quiet.shadow === 'none' && quiet.border.startsWith('0px'),
    `desktop: Split seats… is a quiet button, with no box (${JSON.stringify(quiet)})`,
  );
  const sided = await sideColored(settings);
  check(sided.length === 0, `desktop: no control on the Werewolf tab is red or blue (${sided.join(' ')})`);
  const unringed = await ringed(settings, 30);
  check(unringed.length === 0, `desktop: every control on the Werewolf tab shows a focus ring (${unringed.join(' ')})`);
  // Each section is grouped lists, rows on hairlines, with no box inside
  // a list but its controls.
  await settings.fill('.sd-adder-strategy input', 'bold');
  await settings.click('.sd-adder-strategy button');
  await settings.waitForSelector('.sd-strategy >> nth=1');
  const page = await settings.evaluate(() => {
    const controls = 'button, input, select, textarea, .sd-stepper, .sd-segments, .sd-select-box';
    const boxed = (element) => {
      const style = getComputedStyle(element);
      return ['Top', 'Right', 'Bottom', 'Left'].every((side) => style[`border${side}Width`] !== '0px' && style[`border${side}Style`] !== 'none');
    };
    const lists = [...document.querySelectorAll('.sd-list')];
    return {
      sidebar: document.querySelector('.sd-sidebar').getBoundingClientRect().width,
      lists: lists.length,
      nested: lists.flatMap((list) =>
        [...list.querySelectorAll('*')].filter((inner) => !inner.closest(controls) && boxed(inner)).map((inner) => inner.outerHTML.slice(0, 60)),
      ),
      limit: document.querySelector('.sd-limit input').getBoundingClientRect().width,
      uses: [...document.querySelectorAll('.sd-part .sd-uses')].map((uses) => uses.textContent),
      pulled: document.querySelector('.sd-part code').textContent,
    };
  });
  check(page.sidebar === 220 && page.lists >= 6, `desktop: the Werewolf page is a sidebar beside grouped lists (${JSON.stringify(page)})`);
  check(page.nested.length === 0, `desktop: no grouped list holds another box (${page.nested.join(' ')})`);
  check(page.limit === 80, `desktop: a phase limit's field is short (${page.limit}px)`);
  check(JSON.stringify(page.uses) === JSON.stringify(['used by 1 strategy']), `desktop: a prompt part shows its uses (${page.uses})`);
  check(page.pulled === '{{ rules }}', `desktop: a prompt part shows how to pull it in (${page.pulled})`);
  await settings.fill('.sd-adder-part input', 'the-tone');
  await settings.click('.sd-adder-part button');
  const refused = await settings.$eval('.sd-adder-part', (adder) => adder.textContent);
  check(refused.includes('Use letters, digits, underscores and spaces'), `desktop: a prompt part's name is letters, digits, underscores and spaces (${refused})`);
  await settings.screenshot({ path: join(shots, 'settings-werewolf-wide.png'), fullPage: true });
  // Every control on the General, Werewolf and Avalon pages has help.
  const unhelped = [];
  for (const tab of ['general', 'werewolf', 'avalon']) {
    await settings.click(`#tab-${tab}`);
    await settings.waitForTimeout(100);
    unhelped.push(
      ...(await settings.$$eval('.sd-settings button, .sd-settings input, .sd-settings select, .sd-settings textarea, .sd-settings h2, .sd-settings h3', (elements) =>
        elements.filter((element) => !element.closest('[data-tip]:not([data-tip=""])') && !element.querySelector('[data-tip]:not([data-tip=""])')).map((element) => element.outerHTML.slice(0, 80)),
      )),
    );
  }
  check(unhelped.length === 0, `desktop: everything on General, Werewolf and Avalon has help${unhelped.map((element) => `; ${element}`).join('')}`);
  await settings.screenshot({ path: join(shots, 'settings-avalon.png'), fullPage: true });
  // On a narrow window the sidebar stacks above the page.
  await settings.click('#tab-werewolf');
  await settings.setViewportSize({ width: 520, height: 700 });
  await settings.waitForTimeout(100);
  const narrowSettings = await settings.evaluate(() => {
    const box = (selector) => document.querySelector(selector).getBoundingClientRect();
    const pane = document.scrollingElement;
    return {
      stacked: box('.sd-page').top >= box('.sd-sidebar').bottom,
      wide: Math.max(pane.scrollWidth, document.querySelector('.sd-settings').scrollWidth),
      window: pane.clientWidth,
    };
  });
  check(narrowSettings.stacked, 'desktop: a narrow Settings window stacks the sidebar above the page');
  check(narrowSettings.wide <= narrowSettings.window, `desktop: a narrow Settings window scrolls only down (${narrowSettings.wide} in ${narrowSettings.window})`);
  await settings.screenshot({ path: join(shots, 'settings-werewolf-narrow.png'), fullPage: true });
  await settings.setViewportSize({ width: 760, height: 560 });
  // Typing goes on into a new prompt part and a phase limit, each change
  // stored as it is made.
  await settings.fill('.sd-adder-part input', 'tone');
  await settings.click('.sd-adder-part button');
  const part = '[id="field-werewolf.prompt_parts.tone"]';
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

await section('desktop with an error', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend, { ...desktop, errors: [{ path: ['avalon', 'phases', 'vote_limit'], message: 'Give a duration, as 1m or 30s.' }] });
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.sd-start .sd-game-summary:not(:empty)');
  const rows = await main.$$eval('.sd-game-list > .sd-game', (games) =>
    games.map((game) => ({
      enabled: !game.querySelector('.sd-play').disabled,
      warning: game.querySelector('.sd-warning')?.textContent.trim() ?? null,
    })),
  );
  check(
    JSON.stringify(rows) === JSON.stringify([{ enabled: true, warning: null }, { enabled: false, warning: 'Give a duration, as 1m or 30s. Fix it in Settings' }]),
    `desktop: a game with a settings error can't be played and says why, and the others can (${JSON.stringify(rows)})`,
  );
  const opened = await main.evaluate(async () => {
    const asked = [];
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      asked.push([command, args]);
      return invoke(command, args);
    };
    document.querySelector('.sd-warning .sd-link').click();
    return asked;
  });
  check(
    JSON.stringify(opened) === JSON.stringify([['settings_open', { path: ['avalon', 'phases', 'vote_limit'] }]]),
    `desktop: the error links to its field in Settings (${JSON.stringify(opened)})`,
  );
  await main.screenshot({ path: join(shots, 'desktop-start-error.png'), fullPage: true });
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
