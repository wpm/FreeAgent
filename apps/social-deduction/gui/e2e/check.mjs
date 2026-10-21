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
// it speaks, turns dark by night and light by day, washes a vote's row with ink
// and tags it New as it lands, and shows a day closing, with speech
// landing while it does, before the night falls. The prompted Avalon board names the Assassin's target.
// In each game, dragging the handle on the line between the chat and the
// board resizes them by as far as the pointer moves, selecting no text,
// the board to no less than 240 px and the chat to no less than 320 px,
// the arrow keys step it 16 px, Home and End take it to its limits, and a
// double click puts it back. The handle tells its width and limits, keeps
// clear of the chat's scrollbar, and in a window narrowed under a wide
// board, moves on from the width shown. At 420 px wide the board stacks under the chat, the handle is
// hidden, and nothing scrolls sideways. The handle is the one edge that
// resizes anything, and the page and the board stay put past their ends. The tab shows the mark. With a stand-in for the desktop app's backend,
// the main window opens on its start state: the mark, then one row per game
// with what it is, its seats, its summary and its own Play, which a
// settings error disables, linked to its field. #settings opens the
// Settings window's four pages in a vertical tablist the arrows move
// through, each page a few grouped lists with no box inside them and what
// each role and phase does written under its name, help shows on hover and
// on focus, under the part of an element that places it, and not while
// pressed, and typing goes on in a field while each change is stored. Its Models tab
// shows each provider by name with where its key comes from, why it lists
// nothing as a marigold warning, or the models that play; adding one asks for its name, and a
// name taken or empty is refused; a context window typed overrides the
// provider's, and Reset puts it back; and everything on it has help. Its
// controls are ink only, each with a focus ring: Play a large primary
// button, the others secondary, quiet or icon buttons, a segmented control
// raised out of a sunken track, a joined stepper, fields and selects sunk
// into the page, and chips in gray. A log opened in review starts on its
// first bubble with every later one dimmed; the arrow keys, Home and End,
// ⌘↑ and ⌘↓, ⌥↓ and a click move the cursor, and the board, the outcome
// and the position follow it; ⌘] and the list of games open another game
// on its first bubble, and Close goes back to the start state. A game
// just played goes into review on its last bubble under the game's name,
// offering Save Log… beside New game; Save Game Log… is enabled there and
// nowhere else, and ⌘↑ goes to the first bubble; a log opened from there
// offers Close. On the web page a game at rest is in review on its
// last bubble, the keys and a click move the cursor, and Play, then Play
// again, leaves review until every cue is shown. The console stays clean
// throughout.
// What any app's chrome must pass comes from free-agent-gui's
// crates/gui/e2e/chrome.mjs.
// Screenshots land in ./screenshots for a human to look at.
//
//   npm test                 # against ../dist, from this directory
//   node check.mjs <dist>    # against another build

import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { backend, check, finish, ringed, section, serve, shapes, tablist, tips, watch } from '../../../../crates/gui/e2e/chrome.mjs';

const dist = resolve(process.argv[2] ?? '../dist');
const shots = resolve('screenshots');
await mkdir(shots, { recursive: true });

// build-page.sh builds for the site's /FreeAgent/viewer/, so the server
// serves `dist` there.
const { server, origin } = await serve(dist, '/FreeAgent/viewer/');

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

/** Where the review cursor is on `page`: the position shown, the
 * selected bubble, how many bubbles there are and how many are dimmed,
 * the file named, and the outcome shown. */
const cursor = (page) =>
  page.evaluate(() => {
    const messages = [...document.querySelectorAll('.rc-message')];
    return {
      position: document.querySelector('.fa-review-position')?.textContent ?? null,
      selected: messages.findIndex((message) => message.classList.contains('rc-selected')),
      count: messages.length,
      later: messages.filter((message) => message.classList.contains('rc-later')).length,
      name: document.querySelector('.fa-review-name')?.textContent ?? null,
      outcome: document.querySelector('.sd-outcome')?.textContent ?? null,
    };
  });

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
      const muted = color('--fa-muted');
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
const page = watch(await context.newPage());

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
  const leaders = await page.$$eval('.sd-votes', ([table]) => {
    const seats = [...table.querySelectorAll('thead .sd-seat')].map((square) => square.textContent);
    return [...table.querySelectorAll('tbody th')].map((header) => {
      const squares = [...header.querySelectorAll('.sd-seat')].map((square) => square.textContent);
      return { squares, seated: squares.every((number) => seats.includes(number)) };
    });
  });
  check(
    leaders.length > 0 && leaders.every((row) => row.squares.length === 1 && row.seated),
    `avalon: every vote row names its leader by seat square (${JSON.stringify(leaders.map((row) => row.squares))})`,
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

// A game at rest is in review on its last bubble, and the keys and clicks
// move the cursor.
await section('web review', async () => {
  await open(page, 'avalon-prompted');
  const rest = await cursor(page);
  check(
    rest.selected === rest.count - 1 && rest.later === 0 && rest.position === `${rest.count} / ${rest.count}` && rest.name === null,
    `web review: a game at rest is in review on its last bubble, with nothing dimmed (${JSON.stringify(rest)})`,
  );
  await page.keyboard.press('ArrowUp');
  const back = await cursor(page);
  check(
    back.selected === rest.count - 2 && back.later === 1 && back.position === `${rest.count - 1} / ${rest.count}`,
    `web review: the arrow keys step back, dimming the bubble after (${JSON.stringify(back)})`,
  );
  await page.keyboard.press('Home');
  const first = await cursor(page);
  check(first.selected === 0 && first.outcome === null, `web review: Home goes to the first bubble, before the outcome (${JSON.stringify(first)})`);
  await page.keyboard.press('ArrowRight');
  check((await cursor(page)).selected === 1, 'web review: the arrow keys step forward');
  await (await page.$$('.rc-message'))[3].click();
  check((await cursor(page)).selected === 3, 'web review: a click on a bubble puts the cursor there');
  await page.keyboard.press('End');
  const end = await cursor(page);
  check(end.selected === rest.count - 1 && end.outcome !== null, 'web review: End goes back to the last bubble and its outcome');
  await page.screenshot({ path: join(shots, 'web-review.png'), fullPage: true });
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
    // The wash of ink that a landing row takes.
    const probe = document.createElement('div');
    probe.style.cssText = 'background: var(--sd-wash)';
    document.body.append(probe);
    const washed = getComputedStyle(probe);
    const snapshot = () => {
      const phase = document.querySelector('.sd-phase');
      const clock = phase?.querySelector('.fa-clock')?.textContent ?? '';
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
            washed: style.backgroundColor === washed.backgroundColor,
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
  const play = () => page.$eval('.fa-header button.fa-primary', (button) => button.textContent);
  const label = await play();
  check(label === 'Play', `werewolf-prompted: a game at rest offers Play (${label})`);
  await page.click('.fa-header button.fa-primary');
  const again = await play();
  check(
    again === 'Play again' && (await page.$('.fa-review-position')) === null,
    `werewolf-prompted: Play leaves review and becomes Play again (${again})`,
  );
  await page.waitForSelector('.rc-thinking');
  await page.screenshot({ path: join(shots, 'werewolf-prompted-thinking.png'), fullPage: true });
  await page.waitForSelector('.fa-review-position', { timeout: 60_000 });
  await page.waitForTimeout(SETTLE);
  const played = await cursor(page);
  check(
    played.selected === played.count - 1 && played.later === 0 && played.outcome !== null,
    `werewolf-prompted: once Play has shown every cue, the game is back in review on its last bubble (${JSON.stringify(played)})`,
  );
  check(
    !(await page.evaluate(() => document.body.innerText.includes('Replay'))),
    'werewolf-prompted: no control is labeled Replay',
  );
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
    landed.length > 0 && landed.every((row) => row.washed && row.tag === '"New"'),
    `werewolf-prompted: a landed vote's row is washed with ink and tagged New while it flashes (${landed.length} seen)`,
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

// The handle on the line between the chat and the board drags the board
// wider or narrower, within limits, and the keys move it.
await section('splitter', async () => {
  const widths = () =>
    page.evaluate(() => ({
      chat: document.querySelector('.sd-chat').getBoundingClientRect().width,
      board: document.querySelector('.sd-board').getBoundingClientRect().width,
    }));
  const value = () =>
    page.$eval('.fa-splitter', (element) => ({
      now: Number(element.getAttribute('aria-valuenow')),
      min: Number(element.getAttribute('aria-valuemin')),
      max: Number(element.getAttribute('aria-valuemax')),
    }));
  /** Drags the handle `by` pixels sideways, from the middle of its strip. */
  const drag = async (by) => {
    const handle = await page.$eval('.fa-splitter', (element) => {
      const box = element.getBoundingClientRect();
      return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
    });
    await page.mouse.move(handle.x, handle.y);
    await page.mouse.down();
    await page.mouse.move(handle.x + by / 2, handle.y);
    await page.mouse.move(handle.x + by, handle.y);
    await page.mouse.up();
  };
  /** Presses `key` on the handle. */
  const press = async (key) => {
    await page.focus('.fa-splitter');
    await page.keyboard.press(key);
  };
  for (const game of ['avalon', 'werewolf']) {
    await open(page, game);
    const separator = await page.$eval('.fa-splitter', (element) => {
      const chat = document.querySelector('.sd-chat').getBoundingClientRect();
      const board = document.querySelector('.sd-board').getBoundingClientRect();
      const box = element.getBoundingClientRect();
      const grip = element.querySelector('.fa-grip').getBoundingClientRect();
      return {
        role: element.getAttribute('role'),
        orientation: element.getAttribute('aria-orientation'),
        cursor: getComputedStyle(element).cursor,
        onBoard: Math.abs(box.left - board.left) <= 1 && box.right <= board.right,
        gripOnLine: Math.abs(grip.left + grip.width / 2 - board.left) <= 1,
        // The chat's own scrollbar, at its right edge, takes the pointer.
        scrollbar: document.elementFromPoint(chat.right - 2, chat.top + chat.height / 2)?.closest('.sd-chat') !== null,
      };
    });
    check(
      separator.role === 'separator' &&
        separator.orientation === 'vertical' &&
        separator.cursor === 'col-resize' &&
        separator.onBoard &&
        separator.gripOnLine &&
        separator.scrollbar,
      `${game}: a handle sits on the line between the chat and the board, clear of the chat (${JSON.stringify(separator)})`,
    );
    const before = await widths();
    const limits = await value();
    check(
      JSON.stringify(limits) === JSON.stringify({ now: before.board, min: 240, max: 960 }),
      `${game}: the handle tells its width and limits (${JSON.stringify(limits)})`,
    );
    await drag(-100);
    const wider = await widths();
    check(
      Math.abs(wider.board - before.board - 100) <= 1 && Math.abs(before.chat - wider.chat - 100) <= 1,
      `${game}: dragging the handle left widens the board by as much, with no jump (${before.board} to ${wider.board})`,
    );
    check(
      await page.evaluate(() => getSelection().toString() === ''),
      `${game}: dragging the handle selects no text`,
    );
    await drag(1000);
    check((await widths()).board === 240, `${game}: the board narrows no further than 240 px`);
    await drag(-2000);
    check((await widths()).chat === 320, `${game}: the chat narrows no further than 320 px`);
    await page.dblclick('.fa-splitter');
    check((await widths()).board === before.board, `${game}: a double click puts the board back`);
    await press('ArrowLeft');
    const stepped = (await widths()).board;
    await press('ArrowRight');
    check(
      stepped === before.board + 16 && (await widths()).board === before.board,
      `${game}: the arrow keys step the board 16 px (${stepped})`,
    );
    await press('Home');
    const home = await widths();
    await press('End');
    const end = await widths();
    check(
      home.board === 240 && end.chat === 320,
      `${game}: Home takes the board to its narrowest and End to its widest (${home.board}, ${end.board})`,
    );
    // A window narrowed under a wide board takes the board's width
    // down, and the keys move on from the width shown.
    await page.setViewportSize({ width: 1000, height: 900 });
    // The page measures the narrower window a moment later.
    await page.waitForTimeout(SETTLE);
    const narrowed = await widths();
    const told = await value();
    await press('ArrowRight');
    const moved = (await widths()).board;
    check(
      narrowed.board === 680 && told.now === 680 && told.max === 680 && moved === 664,
      `${game}: in a narrower window the board keeps the chat's width, and the keys move from what shows (${narrowed.board}, ${JSON.stringify(told)}, ${moved})`,
    );
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.waitForTimeout(SETTLE);
    await page.dblclick('.fa-splitter');
  }
  await page.screenshot({ path: join(shots, 'werewolf-splitter.png'), fullPage: true });
});

// The handle is the window's one draggable edge: nothing else resizes or
// shows a resize cursor, and the page itself stays put under a drag or a
// scroll past its end, as on the bottom bar.
await section('edges', async () => {
  for (const game of ['avalon', 'werewolf']) {
    await open(page, game);
    const edges = await page.evaluate(() => ({
      resizers: [...document.querySelectorAll('body *')]
        .filter((element) => {
          const style = getComputedStyle(element);
          // A resize cursor counts where it is set, not where it is inherited.
          const inherited = getComputedStyle(element.parentElement).cursor === style.cursor;
          return style.resize !== 'none' || (style.cursor.endsWith('resize') && !inherited);
        })
        .map((element) => element.className),
      overscroll: [document.documentElement, document.body, document.querySelector('.sd-board')].map(
        (element) => getComputedStyle(element).overscrollBehaviorY,
      ),
    }));
    check(
      JSON.stringify(edges.resizers) === JSON.stringify(['fa-splitter']),
      `${game}: the handle is the one draggable edge (${edges.resizers.join(', ')})`,
    );
    check(
      JSON.stringify(edges.overscroll) === JSON.stringify(['none', 'none', 'contain']),
      `${game}: the page stays put past its ends, and so does it past the board's (${edges.overscroll.join(', ')})`,
    );
  }
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
  check(!(await page.isVisible('.fa-splitter')), 'narrow: the handle is hidden');
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
    general: { appearance: 'system' },
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

/** Social Deduction's answers to the stand-in backend, run on the page:
 * the providers and what each lists, where each key is, the menus the
 * main window enables, and the logs saved. The native menu's Go
 * shortcuts take their keys before the page sees them. */
function answers(standIn) {
  // The menus as the main window last enabled them, and how many logs
  // were saved.
  standIn.menus = null;
  standIn.saved = 0;
  const shortcuts = {
    'Meta+ArrowUp': 'first',
    'Meta+ArrowDown': 'last',
    'Alt+ArrowUp': 'previous_chapter',
    'Alt+ArrowDown': 'next_chapter',
    'Meta+[': 'previous_game',
    'Meta+]': 'next_game',
  };
  window.addEventListener(
    'keydown',
    (event) => {
      const pressed = `${event.metaKey ? 'Meta+' : ''}${event.altKey ? 'Alt+' : ''}${event.key}`;
      const go = shortcuts[pressed];
      const games = standIn.menus?.games ?? 0;
      if (go && games > (go.endsWith('_game') ? 1 : 0)) {
        event.preventDefault();
        event.stopImmediatePropagation();
        standIn.emit('go', go, 'main');
      }
    },
    true,
  );
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
    menus: (enabled) => {
      standIn.menus = enabled;
      return null;
    },
    save_log: () => {
      standIn.saved += 1;
      return null;
    },
  };
}

/** The sides' colors, which no control on `page` uses. */
async function sideColored(page) {
  return page.$$eval('button, input, select, textarea, .fa-chip', (controls) => {
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

/** Check the Models tab of the Settings window `settings`, then go back
 * to the Werewolf tab. */
async function models(settings) {
  await settings.click('#tab-models');
  await settings.waitForSelector('.fa-offered');
  const providers = await settings.$$eval('.fa-provider h2', (names) => names.map((name) => name.textContent.trim()));
  check(
    JSON.stringify(providers) === JSON.stringify(['OpenAI', 'Anthropic', 'Local (localhost:1234)', 'Local (localhost:11434)', 'Broke', 'Mute']),
    `desktop: each provider is shown by its name (${providers.join(', ')})`,
  );
  const card = (name) => `.fa-provider[aria-label="${name}"]`;
  const text = (selector) => settings.$eval(selector, (element) => element.textContent.trim());
  const ids = (selector) => settings.$$eval(selector, (found) => found.map((id) => id.textContent.trim()));
  check(
    (await text(`${card('Local (localhost:11434)')} .fa-warning`)) === 'The server is running, but no model is loaded.',
    'desktop: a local server with no model loaded says so',
  );
  check((await text(`${card('OpenAI')} .fa-warning`)).includes('401'), 'desktop: a provider that cannot list its models says why');
  const tip = (selector) => settings.$eval(selector, (element) => element.dataset.tip);
  check(
    (await text(`${card('Broke')} .fa-warning`)) ===
      "Broke's account has no credits left. Add credits at billing.broke.example, then reopen Settings." &&
      (await tip(`${card('Broke')} .fa-warning`)).startsWith('You have no credits remaining.') &&
      (await settings.$$(`${card('Broke')} .fa-note`)).length === 0 &&
      (await text('.fa-status')) === "Some models couldn't be checked. Their providers say why.",
    "desktop: a provider whose models couldn't be checked says why, and the provider's words are its help",
  );
  check(
    (await text(`${card('Mute')} .fa-note`)) === 'None of its models plays both games. Most often: no tools.' &&
      (await tip(`${card('Mute')} .fa-note`)) === 'mute-1: no tools\nmute-2: no tools',
    "desktop: a provider none of whose models play gives the most common reason, and each model's in its help",
  );
  check(
    (await text(`${card('OpenAI')} .fa-chip`)) === 'OPENAI_API_KEY from the environment' &&
      (await text(`${card('Anthropic')} .fa-chip`)) === 'ANTHROPIC_API_KEY in the keychain' &&
      (await text(`${card('Local (localhost:1234)')} .fa-chip`)) === 'No key needed',
    'desktop: each provider says where its key comes from',
  );
  const offered = await ids(`${card('Anthropic')} .fa-offered .fa-model-id`);
  check(
    JSON.stringify(offered) === JSON.stringify(['claude-opus-5-5', 'claude-sonnet-5-5']),
    `desktop: only models that play are available, in the listing's order (${offered.join(', ')})`,
  );
  const local = await ids(`${card('Local (localhost:1234)')} .fa-model-id`);
  check(
    JSON.stringify(local) === JSON.stringify(['qwen2.5-7b-instruct-1m', 'llama-3.2-3b-instruct']),
    `desktop: a model added is under its provider and not available again (${local.join(', ')})`,
  );
  const untipped = await settings.$$eval('.fa-settings button, .fa-settings input', (all) =>
    all.filter((element) => !element.closest('[data-tip]')).map((element) => element.outerHTML.slice(0, 80)),
  );
  check(untipped.length === 0, `desktop: everything on the Models tab has help (${untipped.join(' ')})`);
  const [chip] = await shapes(settings, `${card('OpenAI')} .fa-chip`);
  check(
    chip.height === 26 && chip.radius === '13px' && chip.background === 'rgb(240, 240, 240)' && chip.weight === '600',
    `desktop: where a key comes from is a gray chip (${JSON.stringify(chip)})`,
  );
  const [add] = await shapes(settings, `${card('Anthropic')} .fa-offered button`);
  check(add.height === 32 && add.radius === '8px', `desktop: a model's Add is a small secondary button (${JSON.stringify(add)})`);
  const sided = await sideColored(settings);
  check(sided.length === 0, `desktop: no control on the Models tab is red or blue (${sided.join(' ')})`);
  const flags = await settings.$$eval('.fa-provider .fa-warning', (warnings) =>
    warnings.map((warning) => getComputedStyle(warning, '::before').backgroundColor),
  );
  check(
    flags.length >= 3 && flags.every((flag) => flag === 'rgb(242, 179, 0)'),
    `desktop: every problem on the Models tab is a marigold warning (${flags.join(', ')})`,
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
  check((await settings.$eval(name, (input) => input.value)) === 'claude-opus-5-5', 'desktop: a model added is first named for its id');
  await settings.fill(name, 'qwen');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} .fa-field-error`);
  check(
    (await text(`${card('Anthropic')} .fa-field-error`)) === 'Another model already has that name.',
    'desktop: a name another model has is refused',
  );
  await settings.fill(name, 'opus');
  await settings.press(name, 'Enter');
  await settings.waitForSelector(`${card('Anthropic')} input.fa-model-name`);
  check((await settings.$eval(`${card('Anthropic')} input.fa-model-name`, (input) => input.value)) === 'opus', 'desktop: the model is added under the name given');
  // A name emptied is refused, and the old one is put back.
  const qwen = `${card('Local (localhost:1234)')} input.fa-model-name`;
  await settings.fill(qwen, '');
  await settings.press(qwen, 'Tab');
  check((await settings.$eval(qwen, (input) => input.value)) === 'qwen', 'desktop: an empty name is refused');
  // The context window is the provider's until a number is typed, and
  // Reset puts it back.
  const window = `${card('Local (localhost:1234)')} .fa-context-window input`;
  const shown = () => settings.$eval(window, (input) => [input.value, input.placeholder]);
  check(JSON.stringify(await shown()) === JSON.stringify(['', '1,010,000']), "desktop: an empty context window shows the provider's figure");
  await settings.fill(window, '32768');
  await settings.press(window, 'Tab');
  await settings.waitForSelector(`${card('Local (localhost:1234)')} .fa-context-window button`);
  check((await shown())[0] === '32,768', 'desktop: a context window typed overrides it');
  await settings.screenshot({ path: join(shots, 'settings-models-added.png'), fullPage: true });
  await settings.click(`${card('Local (localhost:1234)')} .fa-context-window button`);
  check((await shown())[0] === '', 'desktop: Reset goes back to the provider\'s figure');
  await settings.click('#tab-werewolf');
  await settings.waitForSelector('.fa-stepper input');
}

await section('desktop', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend(desktop, answers));
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.fa-start .sd-game-summary:not(:empty)');
  const games = await main.$$eval('.sd-game-list > .sd-game', (rows) =>
    rows.map((row) => ({
      name: row.querySelector('h2').textContent,
      described: row.querySelector('.sd-description').textContent.length > 0,
      seats: [...row.querySelectorAll('.sd-game-seats > span')].map((seat) => seat.className).join(' '),
      said: [row.querySelector('.sd-game-seats').getAttribute('role'), row.querySelector('.sd-game-seats').getAttribute('aria-label')],
      summary: row.querySelector('.sd-game-summary').textContent,
      play: row.querySelector('.sd-play').textContent.trim(),
      enabled: !row.querySelector('.sd-play').disabled,
      warned: row.querySelector('.fa-warning') !== null,
    })),
  );
  check(
    JSON.stringify(games) ===
      JSON.stringify([
        {
          name: 'Werewolf',
          described: true,
          seats: 'sd-evil sd-evil sd-good sd-good sd-good sd-good sd-good',
          said: ['img', '2 evil, 5 good'],
          summary: '7 players · 1 strategy · qwen2.5-7b-instruct-1m',
          play: 'Play Werewolf',
          enabled: true,
          warned: false,
        },
        { name: 'Avalon', described: true, seats: 'sd-evil sd-evil sd-good sd-good sd-good', said: ['img', '2 evil, 3 good'], summary: '5 players · 1 strategy', play: 'Play Avalon', enabled: true, warned: false },
      ]),
    `desktop: the start state lists each game with what it is, its seats, its summary and its own Play (${JSON.stringify(games)})`,
  );
  check((await main.$$('.fa-start .fa-segments')).length === 0, 'desktop: the start state has no game chooser');
  const look = await main.evaluate(() => {
    const style = (selector) => getComputedStyle(document.querySelector(selector));
    return {
      title: [style('.fa-start-head h1').fontSize, style('.fa-start-head h1').fontWeight],
      tagline: style('.fa-tagline').fontStyle,
      name: style('.sd-game h2').fontSize,
      settings: document.querySelector('.fa-open-settings').textContent,
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
  const logo = await main.$eval('.fa-logo svg', (svg) => ({
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
  const [big] = await shapes(main, '.fa-start .sd-play');
  check(
    big.height === 48 && big.radius === '12px' && big.background === 'rgb(17, 17, 17)' && big.color === 'rgb(255, 255, 255)' && big.shadow.includes('inset'),
    `desktop: Play is a large primary button, filled with ink and lifted (${JSON.stringify(big)})`,
  );
  const startSided = await sideColored(main);
  check(startSided.length === 0, `desktop: no control on the start state is red or blue (${startSided.join(' ')})`);
  const startUnringed = await ringed(main, 3);
  check(startUnringed.length === 0, `desktop: every control on the start state shows a focus ring (${startUnringed.join(' ')})`);
  await main.hover('.sd-play');
  const played = await main.$eval('.fa-tip', (element) => !element.hidden && element.textContent);
  check(typeof played === 'string' && played.startsWith('Start a game of Werewolf'), `desktop: Play has help (${played})`);
  const settings = watch(await app.newPage());
  await settings.setViewportSize({ width: 760, height: 560 });
  await settings.goto(`${origin}#settings`);
  await settings.waitForSelector('.fa-sidebar [role="tab"]');
  await tablist(settings, 'desktop', '.fa-sidebar', ['General', 'Models', 'Werewolf', 'Avalon']);
  await settings.screenshot({ path: join(shots, 'settings-general.png'), fullPage: true });
  const headings = await settings.$$eval('.fa-settings h2', (elements) => elements.map((element) => element.textContent.trim()));
  check(
    JSON.stringify(headings) === JSON.stringify(['Appearance']),
    `desktop: General has the look alone (${headings.join(', ')})`,
  );
  // Help shows under what the pointer is over, and under what has the
  // keyboard's focus.
  await tips(settings, 'desktop', {
    hover: { selector: '.fa-segments button:has-text("Light")', text: 'Light, whatever' },
    focus: { selector: '.fa-segments button:has-text("Dark")', text: 'Dark, whatever' },
    press: '#tab-general',
    shot: join(shots, 'settings-tip.png'),
  });
  await settings.click('#tab-werewolf');
  await settings.waitForSelector('.fa-page-head h1');
  await settings.screenshot({ path: join(shots, 'settings-werewolf.png'), fullPage: true });
  const sideSaid = await settings.$$eval('.sd-square', (squares) =>
    squares.map((square) => [square.getAttribute('role'), square.getAttribute('aria-label'), getComputedStyle(square).backgroundColor]),
  );
  const sideOf = { 'rgb(210, 0, 31)': 'Evil', 'rgb(36, 56, 232)': 'Good' };
  check(
    sideSaid.length > 0 && sideSaid.every(([role, label, color]) => role === 'img' && label === sideOf[color]),
    `desktop: each role's square says its side (${JSON.stringify(sideSaid)})`,
  );
  const policy = watch(await app.newPage());
  await policy.goto(`${origin}#settings=werewolf/strategies/everyone/policy`);
  await policy.waitForSelector('.fa-segments');
  await policy.waitForFunction(() => document.activeElement !== document.body);
  const policyFocus = await policy.evaluate(() => document.activeElement.closest('.fa-segments')?.getAttribute('aria-label') ?? document.activeElement.outerHTML.slice(0, 60));
  check(policyFocus === 'Policy', `desktop: Settings opened at a policy error focuses the policy (${policyFocus})`);
  await policy.close();
  await models(settings);
  const players = await settings.$$eval('.fa-stepper input', (inputs) => inputs.map((input) => input.value));
  check(JSON.stringify(players) === JSON.stringify(['2', '3', '1', '1']), `desktop: the Werewolf tab shows each role's count (${players.join(', ')})`);
  const notes = await settings.$$eval('.sd-role, .sd-limit', (rows) =>
    rows.map((row) => {
      const note = row.querySelector('.fa-note');
      return `${row.querySelector('.fa-item-name').textContent}: ${note.checkVisibility() ? note.textContent : ''}`;
    }),
  );
  check(
    notes[0] === 'Werewolf: Knows the other werewolves. Each night they kill a villager.' &&
      notes[4] === 'Night: How long the werewolves, the doctor and the seer have to choose.' &&
      notes.every((note) => !note.endsWith(': ')),
    `desktop: what each role and phase does is written under its name (${notes.join('; ')})`,
  );
  const [restore] = await shapes(settings, '.fa-page-head > button');
  check(
    restore.text === 'Restore defaults…' && restore.height === 40 && restore.radius === '10px' && restore.border === '1px rgb(196, 196, 196)' &&
      restore.background === 'rgb(255, 255, 255)' && restore.shadow !== 'none',
    `desktop: Restore defaults… is a secondary button, white with a hairline (${JSON.stringify(restore)})`,
  );
  const [stepper] = await shapes(settings, '.fa-stepper');
  const [fewer, count] = await shapes(settings, '.fa-stepper > *');
  check(
    stepper.height === 36 && stepper.radius === '10px' && stepper.border === '1px rgb(196, 196, 196)' && fewer.width === 36 && count.width === 40,
    `desktop: a stepper is one joined control (${JSON.stringify([stepper, fewer, count])})`,
  );
  const [select] = await shapes(settings, '.fa-select');
  check(
    select.appearance === 'none' && select.icon && select.height === 36 && select.radius === '8px',
    `desktop: a select is a field with a chevron drawn beside it (${JSON.stringify(select)})`,
  );
  const [field] = await shapes(settings, '.fa-adder-strategy input');
  check(field.height === 36 && field.radius === '8px' && field.shadow.includes('inset'), `desktop: a text field is sunk into the page (${JSON.stringify(field)})`);
  const [prompt] = await shapes(settings, '.fa-strategy textarea');
  check(prompt.font.includes('monospace'), `desktop: a prompt is monospace (${prompt.font})`);
  const [remove] = await shapes(settings, '.fa-remove');
  check(
    remove.width === 32 && remove.height === 32 && remove.radius === '8px' && remove.icon && remove.shadow === 'none',
    `desktop: a remove button is a square drawn with an icon (${JSON.stringify(remove)})`,
  );
  const [quiet] = await shapes(settings, '.sd-assigned .fa-link');
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
  await settings.fill('.fa-adder-strategy input', 'bold');
  await settings.click('.fa-adder-strategy button');
  await settings.waitForSelector('.fa-strategy >> nth=1');
  const page = await settings.evaluate(() => {
    const controls = 'button, input, select, textarea, .fa-stepper, .fa-segments, .fa-select-box';
    const boxed = (element) => {
      const style = getComputedStyle(element);
      return ['Top', 'Right', 'Bottom', 'Left'].every((side) => style[`border${side}Width`] !== '0px' && style[`border${side}Style`] !== 'none');
    };
    const lists = [...document.querySelectorAll('.fa-list')];
    return {
      sidebar: document.querySelector('.fa-sidebar').getBoundingClientRect().width,
      lists: lists.length,
      nested: lists.flatMap((list) =>
        [...list.querySelectorAll('*')].filter((inner) => !inner.closest(controls) && boxed(inner)).map((inner) => inner.outerHTML.slice(0, 60)),
      ),
      limit: document.querySelector('.sd-limit input').getBoundingClientRect().width,
      uses: [...document.querySelectorAll('.fa-part .fa-uses')].map((uses) => uses.textContent),
      pulled: document.querySelector('.fa-part code').textContent,
    };
  });
  check(page.sidebar === 220 && page.lists >= 6, `desktop: the Werewolf page is a sidebar beside grouped lists (${JSON.stringify(page)})`);
  check(page.nested.length === 0, `desktop: no grouped list holds another box (${page.nested.join(' ')})`);
  check(page.limit === 80, `desktop: a phase limit's field is short (${page.limit}px)`);
  check(JSON.stringify(page.uses) === JSON.stringify(['used by 1 strategy']), `desktop: a prompt part shows its uses (${page.uses})`);
  check(page.pulled === '{{ rules }}', `desktop: a prompt part shows how to pull it in (${page.pulled})`);
  await settings.fill('.fa-adder-part input', 'the-tone');
  await settings.click('.fa-adder-part button');
  const refused = await settings.$eval('.fa-adder-part', (adder) => adder.textContent);
  check(refused.includes('Use letters, digits, underscores and spaces'), `desktop: a prompt part's name is letters, digits, underscores and spaces (${refused})`);
  await settings.screenshot({ path: join(shots, 'settings-werewolf-wide.png'), fullPage: true });
  // Every control on the General, Werewolf and Avalon pages has help.
  const unhelped = [];
  for (const tab of ['general', 'werewolf', 'avalon']) {
    await settings.click(`#tab-${tab}`);
    await settings.waitForTimeout(100);
    unhelped.push(
      ...(await settings.$$eval('.fa-settings button, .fa-settings input, .fa-settings select, .fa-settings textarea, .fa-settings h2, .fa-settings h3', (elements) =>
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
      stacked: box('.fa-page').top >= box('.fa-sidebar').bottom,
      wide: Math.max(pane.scrollWidth, document.querySelector('.fa-settings').scrollWidth),
      window: pane.clientWidth,
    };
  });
  check(narrowSettings.stacked, 'desktop: a narrow Settings window stacks the sidebar above the page');
  check(narrowSettings.wide <= narrowSettings.window, `desktop: a narrow Settings window scrolls only down (${narrowSettings.wide} in ${narrowSettings.window})`);
  await settings.screenshot({ path: join(shots, 'settings-werewolf-narrow.png'), fullPage: true });
  await settings.setViewportSize({ width: 760, height: 560 });
  // Typing goes on into a new prompt part and a phase limit, each change
  // stored as it is made.
  await settings.fill('.fa-adder-part input', 'tone');
  await settings.click('.fa-adder-part button');
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

// A log opened in review: the backend sends the recorded prompted games,
// Werewolf then Avalon, as Open Game Log… would.
await section('desktop review', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend(desktop, answers));
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.fa-start .sd-game-summary:not(:empty)');
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 0, open: true, save: false }),
    'desktop review: Open Game Log… is enabled on the start screen, and Go and Save Game Log… are not',
  );
  await main.evaluate(async () => {
    const cues = async (name) => (await fetch(`game/${name}.json`)).json();
    const games = [
      { episode: '00000000-0000-0000-0000-000000000001', game: 'Werewolf', cues: await cues('werewolf-prompted') },
      { episode: '00000000-0000-0000-0000-000000000002', game: 'Avalon', cues: await cues('avalon-prompted') },
    ];
    window.standIn.emit('review', { name: 'two.jsonl', recordings: { games, problems: [] } }, 'main');
  });
  await main.waitForSelector('.fa-review-position');
  await main.waitForTimeout(SETTLE);
  /** Where the cursor is: the position shown, the selected bubble, the
   * dimmed ones, the game, and the board and outcome shown. */
  const where = () =>
    main.evaluate(() => {
      const messages = [...document.querySelectorAll('.rc-message')];
      const later = messages.filter((message) => message.classList.contains('rc-later'));
      return {
        position: document.querySelector('.fa-review-position').textContent,
        selected: messages.findIndex((message) => message.classList.contains('rc-selected')),
        current: messages.filter((message) => message.getAttribute('aria-current') === 'true').length,
        count: messages.length,
        later: later.length,
        laterFirst: messages.indexOf(later[0]),
        dimmed: later.length > 0 && Number(getComputedStyle(later[0]).opacity) < 1,
        game: document.querySelector('.fa-header h1').textContent,
        name: document.querySelector('.fa-review-name').textContent,
        of: document.querySelector('.fa-review-game')?.textContent ?? null,
        outcome: document.querySelector('.sd-outcome')?.textContent ?? null,
        board: document.querySelector('.sd-board').innerText,
        theme: document.documentElement.dataset.theme ?? null,
        inView: (() => {
          const selected = document.querySelector('.rc-message.rc-selected');
          const pane = document.querySelector('.rc-messages').getBoundingClientRect();
          const box = selected?.getBoundingClientRect();
          return box !== undefined && box.top >= pane.top - 1 && box.bottom <= pane.bottom + 1;
        })(),
      };
    });
  const first = await where();
  check(
    first.name === 'two.jsonl' && first.game === 'Werewolf' && first.of === 'Game 1 of 2' && first.position === `1 / ${first.count}`,
    `desktop review: a log opens on the first bubble of its first game (${JSON.stringify({ ...first, board: undefined })})`,
  );
  check(
    first.selected === 0 && first.current === 1 && first.later === first.count - 1 && first.laterFirst === 1 && first.dimmed,
    `desktop review: the first bubble is selected and every later one is dimmed (${JSON.stringify({ ...first, board: undefined })})`,
  );
  check(first.outcome === null, `desktop review: the first bubble shows no outcome (${first.outcome})`);
  const arrows = await main.$$eval('.fa-review-games button', (buttons) => buttons.map((button) => [button.textContent, button.disabled]));
  check(
    JSON.stringify(arrows) === JSON.stringify([['‹', true], ['›', false]]),
    `desktop review: ‹ and › step between games, ‹ off on the first (${JSON.stringify(arrows)})`,
  );
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 2, open: true, save: false }),
    'desktop review: Open Game Log… and Go are enabled in review, Previous and Next Game with them',
  );
  await main.screenshot({ path: join(shots, 'desktop-review-first.png') });
  await main.keyboard.press('ArrowDown');
  await main.keyboard.press('ArrowRight');
  const stepped = await where();
  check(
    stepped.selected === 2 && stepped.position === `3 / ${first.count}` && stepped.later === first.count - 3,
    `desktop review: the arrow keys step forward (${JSON.stringify({ ...stepped, board: undefined })})`,
  );
  await main.keyboard.press('ArrowUp');
  check((await where()).selected === 1, 'desktop review: the arrow keys step back');
  await main.keyboard.press('Meta+ArrowDown');
  await main.waitForTimeout(SETTLE);
  const last = await where();
  check(
    last.selected === first.count - 1 && last.later === 0 && last.position === `${first.count} / ${first.count}` && last.inView,
    `desktop review: ⌘↓ jumps to the last bubble, in view, with nothing dimmed (${JSON.stringify({ ...last, board: undefined })})`,
  );
  check(last.outcome !== null && last.board !== first.board, `desktop review: the board and outcome follow the cursor (${last.outcome})`);
  await main.screenshot({ path: join(shots, 'desktop-review-last.png') });
  await main.keyboard.press('ArrowDown');
  check((await where()).selected === first.count - 1, 'desktop review: the cursor stops at the last bubble');
  await main.keyboard.press('Home');
  check((await where()).selected === 0, 'desktop review: Home goes to the first bubble');
  await main.keyboard.press('End');
  await main.keyboard.press('Meta+ArrowUp');
  await main.waitForTimeout(SETTLE);
  const top = await where();
  check(top.selected === 0 && top.board === first.board && top.inView, 'desktop review: ⌘↑ goes back to the first bubble and its board');
  await main.keyboard.press('Alt+ArrowDown');
  const chapter = await where();
  const heading = await main.$eval('.rc-message.rc-selected', (message) => message.querySelector('.sd-heading')?.textContent ?? null);
  check(chapter.selected > 0 && heading !== null, `desktop review: ⌥↓ goes to the next chapter heading (${heading})`);
  await (await main.$$('.rc-message'))[4].click();
  check((await where()).selected === 4, 'desktop review: a click on a bubble puts the cursor there');
  await main.keyboard.press('Meta+]');
  await main.waitForTimeout(SETTLE);
  const second = await where();
  check(
    second.game === 'Avalon' && second.of === 'Game 2 of 2' && second.selected === 0 && second.position === `1 / ${second.count}`,
    `desktop review: ⌘] opens the next game on its first bubble (${JSON.stringify({ ...second, board: undefined })})`,
  );
  await main.selectOption('.fa-review-games select', '0');
  await main.waitForTimeout(100);
  check((await where()).game === 'Werewolf', 'desktop review: the list of games opens the one chosen');
  await main.click('.fa-header button:text("Close")');
  await main.waitForSelector('.fa-start');
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 0, open: true, save: false }),
    'desktop review: Close goes back to the start screen, and Go is disabled again',
  );
  await app.close();
});

// A game played live, shown cue by cue down the stand-in's channel, and
// in review once it is over.
await section('desktop review of a game just played', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend(desktop, answers));
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.fa-start .sd-game-summary:not(:empty)');
  await main.evaluate(() => {
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (command === 'play') {
        window.playing = args.cues;
        return null;
      }
      return invoke(command, args);
    };
  });
  await main.click('.sd-game:first-child .sd-play');
  await main.evaluate(async () => {
    window.cues = await (await fetch('game/werewolf-prompted.json')).json();
    window.playing.handler(window.cues[0]);
  });
  await main.waitForTimeout(100);
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 0, open: false, save: false }),
    'desktop review: Open Game Log… and Save Game Log… are disabled while the game plays',
  );
  await main.evaluate(() => {
    for (const cue of window.cues.slice(1)) window.playing.handler(cue);
  });
  await main.waitForSelector('.fa-review-position');
  await main.waitForTimeout(SETTLE);
  const buttons = await main.$$eval('.fa-header button', (buttons) => buttons.map((button) => button.textContent));
  check(
    JSON.stringify(buttons) === JSON.stringify(['Save Log…', 'New game']),
    `desktop review: the game just played offers Save Log… beside New game (${JSON.stringify(buttons)})`,
  );
  const last = await cursor(main);
  const game = await main.$eval('.fa-header h1', (heading) => heading.textContent);
  check(
    last.count > 0 && last.selected === last.count - 1 && last.later === 0 && last.position === `${last.count} / ${last.count}` && last.outcome !== null,
    `desktop review: a game over goes into review on its last bubble, where it left off, with nothing dimmed (${JSON.stringify(last)})`,
  );
  check(last.name === null && game === 'Werewolf', `desktop review: the header names the game, not a file (${game}, ${last.name})`);
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 1, open: true, save: true }),
    'desktop review: Open Game Log…, Save Game Log… and Go are enabled in the review of a game just played',
  );
  await main.click('.fa-header button:text("Save Log…")');
  await main.waitForTimeout(100);
  check((await main.evaluate(() => window.standIn.saved)) === 1, 'desktop review: Save Log… saves the game\'s log');
  await main.screenshot({ path: join(shots, 'desktop-review-played.png') });
  await main.keyboard.press('Meta+ArrowUp');
  await main.waitForTimeout(SETTLE);
  const first = await cursor(main);
  check(
    first.selected === 0 && first.later === last.count - 1 && first.outcome === null,
    `desktop review: ⌘↑ goes to the first bubble (${JSON.stringify(first)})`,
  );
  await main.evaluate(() => {
    const games = [{ episode: '00000000-0000-0000-0000-000000000001', game: 'Werewolf', cues: window.cues }];
    window.standIn.emit('review', { name: 'one.jsonl', recordings: { games, problems: [] } }, 'main');
  });
  await main.waitForSelector('.fa-review-name');
  const logged = await main.$$eval('.fa-header button', (buttons) => buttons.map((button) => button.textContent));
  check(
    JSON.stringify(logged) === JSON.stringify(['Close']),
    `desktop review: a log opened from the review of a game just played offers Close (${JSON.stringify(logged)})`,
  );
  await main.evaluate(() => window.standIn.emit('new-game', null, 'main'));
  await main.waitForSelector('.fa-start');
  check(
    JSON.stringify(await main.evaluate(() => window.standIn.menus)) === JSON.stringify({ games: 0, open: true, save: false }),
    'desktop review: New Game goes from the review to the start screen, and Save Game Log… is disabled',
  );
  await app.close();
});

await section('desktop with an error', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1100, height: 760 } });
  await app.addInitScript(backend({ ...desktop, errors: [{ path: ['avalon', 'phases', 'vote_limit'], message: 'Give a duration, as 1m or 30s.' }] }, answers));
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.fa-start .sd-game-summary:not(:empty)');
  const rows = await main.$$eval('.sd-game-list > .sd-game', (games) =>
    games.map((game) => ({
      enabled: !game.querySelector('.sd-play').disabled,
      warning: game.querySelector('.fa-warning')?.textContent.trim() ?? null,
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
    document.querySelector('.fa-warning .fa-link').click();
    return asked;
  });
  check(
    JSON.stringify(opened) === JSON.stringify([['settings_open', { path: ['avalon', 'phases', 'vote_limit'] }]]),
    `desktop: the error links to its field in Settings (${JSON.stringify(opened)})`,
  );
  await main.evaluate(() => {
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (command === 'play') throw 'qwen did not answer';
      return invoke(command, args);
    };
  });
  await main.click('.sd-game:first-child .sd-play');
  await main.waitForSelector('.sd-game-failed');
  const failed = await main.$$eval('.sd-game-list > .sd-game', (games) => games.map((game) => game.querySelector('.sd-game-failed')?.textContent.trim() ?? null));
  check(
    JSON.stringify(failed) === JSON.stringify(['qwen did not answer', null]) && (await main.$$('.fa-start-body > .fa-error')).length === 0,
    `desktop: why a game failed to start shows in its own row (${JSON.stringify(failed)})`,
  );
  await main.screenshot({ path: join(shots, 'desktop-start-error.png'), fullPage: true });
  await app.close();
});

await browser.close();
server.close();
finish();
