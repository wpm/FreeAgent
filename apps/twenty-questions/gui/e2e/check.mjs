// End-to-end check of Twenty Questions' built page in headless Chromium,
// with a stand-in for the desktop app's backend.
//
// The main window opens on its start: the mark, the name, the link to
// Settings, the tagline, a pip for each guess, a line summing up the
// settings, and Play, which a settings error turns off, linked to its
// field. Play plays the recorded prompted game at ten times its pace. Each
// player speaks from the left in a bubble filled with its own color, the
// sender line holding its name and a square of its color, and the host from
// the right in ink; the narrator is centered and unboxed. A guess is the
// player's bubble bordered in its color and tagged Guess n, and an answer
// the host's, tagged Answer n and quoting the guess it answers; the answer
// that names the object and its guess are ringed. The secret stays in the
// header, with the guesses used as n / 20 over a pip each, the one waiting
// for its answer pulsing and the one that won in its guesser's color, and
// the countdown under them. Thinking shows as dots in the thinker's bubble.
// At the end the game goes into review on its last bubble, the header
// saying who won and offering Save Log… and New game, which goes back to
// the start, as File ▸ New Game does; Save Game Log… is enabled there and
// nowhere else. In review the arrow keys, Home and End, ⌘↑ and ⌘↓, ⌥↑
// and ⌥↓ from guess to guess, and a click move the cursor, every later
// bubble dimmed, and the secret, the count, the pips and the countdown
// show the game as it stood there. A log opened, of three games, starts
// on its first bubble under the file's name; ⌘] and the list of games
// open another game on its first bubble, and Close goes back to the
// start. In a plain browser the page opens game/prompted.json in review
// on its last bubble, Play plays it again and it is back in review once
// every cue is shown, and a game that can't be fetched says why. Every player hue is clear of
// marigold and holds ink at 4.5:1 or more, light and dark. At 420 px wide
// nothing scrolls sideways. #settings opens the Settings window: General,
// Models and Game. Game holds the whole game: the players, named from
// their count, the guesses and the time limit; the host's model, secret
// object, prompt, pick, temperature and categories; the strategies, two
// across where they fit, with Add strategy; who plays what, with Split
// seats…; and the prompt parts, each counting the prompts that use it.
// Each error shows in marigold beneath its own field. The console stays
// clean throughout.
// What any app's chrome and Settings window must pass comes from
// free-agent-gui's crates/gui/e2e/chrome.mjs.
// Screenshots land in ./screenshots for a human to look at.
//
//   npm test                 # against ../dist, from this directory
//   node check.mjs <dist>    # against another build

import { mkdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import {
  backend,
  check,
  finish,
  groupedLists,
  helped,
  models,
  narrowSettings,
  refusedPart,
  ringed,
  section,
  serve,
  settingsWindow,
  shapes,
  standInModels,
  tips,
  typing,
  watch,
} from '../../../../crates/gui/e2e/chrome.mjs';

const dist = resolve(process.argv[2] ?? '../dist');
const shots = resolve('screenshots');
await mkdir(shots, { recursive: true });
const { server, origin } = await serve(dist);

/** The recorded prompted game's cues, which build-page.sh folds into game/. */
const cues = JSON.parse(await readFile(join(dist, 'game', 'prompted.json'), 'utf8'));

/** Whether `cue` is a beat of `kind`, such as `Guessed`. */
const is = (cue, kind) => cue.beat === kind || (typeof cue.beat === 'object' && kind in cue.beat);

/** The guess of the recorded game that named the object. */
const named = cues.find((cue) => is(cue, 'Answered') && cue.beat.Answered.named)?.beat.Answered.guess;

/** How many times its pace the recorded game plays at. */
const PACE = 10;

const browser = await chromium.launch();

/** The settings the stand-in starts with: four players and a host on the
 * local model, which picks its object. */
const desktop = {
  settings: {
    general: { appearance: 'system' },
    ...standInModels,
    game: {
      players: 4,
      guesses: 20,
      time_limit: '10m',
      strategy: 'everyone',
      host: {
        model: 'qwen',
        object: '',
        temperature: 1.2,
        categories: ['an animal'],
        pick: 'Pick {{ category }}.',
        system: '{{ rules }}\nYou are the host. The object is {{ object }}.',
      },
      strategies: { everyone: { policy: 'prompted', model: 'qwen', system: '{{ rules }}\nYou are {{ name }}.' } },
      prompt_parts: { rules: 'Twenty Questions.' },
    },
  },
  errors: [],
};

/** Twenty Questions' answers to the stand-in backend, run on the page:
 * `play` plays the cues the page holds as `window.recorded` at `pace`
 * times their pace, `stop` ends the game stopped, `menus` keeps the menus
 * the main window enables and `save_log` counts the logs saved. The
 * native menu's Go shortcuts take their keys before the page sees them. */
function answers(standIn) {
  standIn.played = 0;
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
    menus: (enabled) => {
      standIn.menus = enabled;
      return null;
    },
    save_log: () => {
      standIn.saved += 1;
      return null;
    },
    play: ({ cues: channel }) => {
      standIn.played += 1;
      const recorded = window.recorded;
      const pace = window.pace;
      const first = recorded[0].at;
      standIn.timers = recorded.map((cue) => {
        const paced = typeof cue.beat === 'object' && 'Clock' in cue.beat ? { ...cue, beat: { Clock: { rate: pace } } } : cue;
        return setTimeout(() => channel.handler(paced), (cue.at - first) / pace);
      });
      return null;
    },
    stop: () => {
      standIn.timers?.forEach(clearTimeout);
      return null;
    },
  };
}

/** A main window with the stand-in backend in a context of `options`,
 * the recorded game ready to play. */
async function mainWindow(options = {}, snapshot = desktop) {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 1000, height: 760 }, ...options });
  await app.addInitScript(backend(snapshot, answers));
  await app.addInitScript(({ recorded, pace }) => {
    window.recorded = recorded;
    window.pace = pace;
  }, { recorded: cues, pace: PACE });
  const main = watch(await app.newPage());
  await main.goto(origin);
  await main.waitForSelector('.fa-start .tq-summary:not(:empty)');
  return { app, main };
}

/** Ends every change of theme on `page` at once, so that what is measured
 * and shot is the theme changed to. */
async function still(page) {
  await page.addStyleTag({ content: '*, *::before, *::after { transition: none !important; }' });
  await page.waitForTimeout(100);
}

/** How long the recorded game takes at `PACE`, in milliseconds. */
const playing = (cues.at(-1).at - cues[0].at) / PACE;

/** WCAG's contrast ratio between two `rgb(…)` colors. */
function contrast(a, b) {
  const luminance = (color) => {
    const [r, g, b] = color.match(/[\d.]+/g).slice(0, 3).map((channel) => {
      const c = Number(channel) / 255;
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [dark, light] = [luminance(a), luminance(b)].sort((x, y) => x - y);
  return (light + 0.05) / (dark + 0.05);
}

/** The twelve player hues on `page`: each fill and mark, the ink and the
 * marigold, as computed colors. */
const hues = (page) =>
  page.evaluate(() => {
    const color = (value) => {
      const probe = document.createElement('span');
      probe.style.color = value;
      document.body.append(probe);
      const computed = getComputedStyle(probe).color;
      probe.remove();
      return computed;
    };
    return {
      players: Array.from({ length: 12 }, (_, k) => ({
        fill: color(`var(--tq-player-${k + 1})`),
        mark: color(`var(--tq-player-${k + 1}-mark)`),
      })),
      ink: color('var(--fa-ink)'),
      paper: color('var(--fa-paper)'),
      warn: color('var(--fa-warn)'),
    };
  });

/** Checks the hues on `page`, as `label`: twelve fills, each holding ink
 * at 4.5:1 or more, each mark standing out of the paper at 3:1 or more,
 * none of them alike, and none marigold. */
async function checkHues(page, label) {
  const { players, ink, paper, warn } = await hues(page);
  const fills = players.map((player) => player.fill);
  const low = players.map((player) => contrast(player.fill, ink)).filter((ratio) => ratio < 4.5);
  check(new Set(fills).size === 12 && low.length === 0, `${label}: the twelve player fills hold ink at 4.5:1 or more (${low.join(', ')})`);
  const faint = players.map((player) => contrast(player.mark, paper)).filter((ratio) => ratio < 3);
  check(new Set(players.map((player) => player.mark)).size === 12 && faint.length === 0, `${label}: the twelve marks stand out of the paper (${faint.join(', ')})`);
  check(!fills.includes(warn) && !players.some((player) => player.mark === warn), `${label}: no player is marigold`);
}

/** What the chat shows: each bubble's sender, side, background, text
 * color, tag, quote and border, and whether it is ringed or selected. */
const bubbles = (page) =>
  page.$$eval('.rc-message', (messages) => {
    const pane = document.querySelector('.rc-messages').getBoundingClientRect();
    return messages
      .filter((message) => message.querySelector(':scope > .rc-bubble'))
      .map((message) => {
        const element = message.querySelector(':scope > .rc-bubble');
        const box = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        const middle = box.left + box.width / 2 - pane.left;
        return {
          name: message.dataset.name,
          side: Math.abs(middle - pane.width / 2) < 4 ? 'center' : middle < pane.width / 2 ? 'left' : 'right',
          background: style.backgroundColor,
          color: style.color,
          border: style.borderTopColor,
          borderWidth: style.borderTopWidth,
          ringed: style.outlineStyle !== 'none' && style.outlineWidth !== '0px',
          selected: message.classList.contains('rc-selected'),
          tag: element.querySelector('.tq-tag')?.textContent ?? null,
          tagCaps: element.querySelector('.tq-tag') ? getComputedStyle(element.querySelector('.tq-tag')).fontVariantCaps : null,
          quote: element.querySelector('.tq-quote')?.textContent ?? null,
          sender: message.querySelector('.rc-sender')?.textContent ?? null,
          square: message.querySelector('.rc-sender .tq-square') ? getComputedStyle(message.querySelector('.rc-sender .tq-square')).backgroundColor : null,
          thinking: !!element.querySelector('.rc-thinking'),
        };
      });
  });

/** The header: the secret, the outcome, the tally, each pip's class and
 * color, and the clock. */
const header = (page) =>
  page.evaluate(() => ({
    secret: document.querySelector('.tq-secret')?.textContent ?? null,
    outcome: document.querySelector('.tq-outcome')?.textContent ?? null,
    tally: document.querySelector('.tq-tally')?.textContent ?? null,
    pips: [...document.querySelectorAll('.tq-header .tq-pip')].map((pip) => ({
      class: pip.className,
      background: getComputedStyle(pip).backgroundColor,
      animated: getComputedStyle(pip).animationName !== 'none',
    })),
    clock: document.querySelector('.tq-header .fa-clock')?.textContent ?? null,
    buttons: [...document.querySelectorAll('.tq-header button')].map((button) => button.textContent),
  }));

/** The players' fills and marks on `page`, by player, from the cast. */
async function playerColors(page) {
  const { players } = await hues(page);
  const seats = cues[0].beat.Cast.seats.filter((seat) => seat.part !== 'Host');
  return Object.fromEntries(seats.map((seat) => [seat.name, players[seat.part.Player.color]]));
}

await section('start', async () => {
  const { app, main } = await mainWindow();
  const start = await main.evaluate(() => ({
    logo: !!document.querySelector('.fa-start-head .fa-logo svg'),
    name: document.querySelector('.fa-start-head h1')?.textContent,
    settings: document.querySelector('.fa-open-settings')?.textContent,
    tagline: document.querySelector('.fa-tagline')?.textContent,
    pips: document.querySelectorAll('.tq-start .tq-pip').length,
    summary: document.querySelector('.tq-summary')?.textContent,
    play: document.querySelector('.tq-play')?.textContent,
    enabled: !document.querySelector('.tq-play')?.disabled,
  }));
  check(
    start.logo && start.name === 'Twenty Questions' && start.settings.startsWith('Settings') &&
      start.tagline === 'Language models put twenty yes-or-no questions to a host who is thinking of something.' &&
      start.pips === 20 && start.summary === '4 players · qwen2.5-7b-instruct-1m · host picks' && start.play === 'Play' && start.enabled,
    `start: the mark, the name, Settings, the tagline, a pip per guess, the summary and Play (${JSON.stringify(start)})`,
  );
  const [play] = await shapes(main, '.tq-play');
  check(play.height === 48 && play.background === 'rgb(17, 17, 17)', `start: Play is a large primary button (${JSON.stringify(play)})`);
  const unringed = await ringed(main, 2);
  check(unringed.length === 0, `start: every control shows a focus ring (${unringed.join(' ')})`);
  await checkHues(main, 'light');
  await main.evaluate(() => document.activeElement.blur());
  await main.mouse.move(0, 0);
  await main.screenshot({ path: join(shots, 'start.png') });
  await main.emulateMedia({ colorScheme: 'dark' });
  await checkHues(main, 'dark');
  await still(main);
  await main.screenshot({ path: join(shots, 'start-dark.png') });
  await app.close();
});

await section('start with an error', async () => {
  const error = { path: ['game', 'host', 'model'], message: "the host names model 'ghost', which doesn't exist" };
  const { app, main } = await mainWindow({}, { ...desktop, errors: [error, { path: ['models', 'unused', 'id'], message: 'no id' }] });
  const shown = await main.evaluate(() => ({
    enabled: !document.querySelector('.tq-play').disabled,
    warning: document.querySelector('.tq-start-error')?.textContent.trim() ?? null,
  }));
  check(
    !shown.enabled && shown.warning === "the host names model 'ghost', which doesn't exist Fix it in Settings",
    `start: an error in the game's settings turns Play off and says why (${JSON.stringify(shown)})`,
  );
  const opened = await main.evaluate(async () => {
    const asked = [];
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      asked.push([command, args]);
      return invoke(command, args);
    };
    document.querySelector('.tq-start-error .fa-link').click();
    return asked;
  });
  check(
    JSON.stringify(opened) === JSON.stringify([['settings_open', { path: ['game', 'host', 'model'] }]]),
    `start: the error links to its field in Settings (${JSON.stringify(opened)})`,
  );
  await main.screenshot({ path: join(shots, 'start-error.png') });
  await app.close();
});

await section('a game that cannot start', async () => {
  const { app, main } = await mainWindow();
  await main.evaluate(() => {
    const invoke = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (command === 'play') throw 'qwen did not answer';
      return invoke(command, args);
    };
  });
  await main.click('.tq-play');
  await main.waitForSelector('.tq-failed');
  const failed = await main.$eval('.tq-failed', (failed) => failed.textContent.trim());
  check(failed === 'qwen did not answer' && (await main.$('.fa-start')) !== null, `start: why a game could not start shows under Play (${failed})`);
  await app.close();
});

await section('play', async () => {
  const { app, main } = await mainWindow();
  await main.click('.tq-play');
  await main.waitForSelector('.tq-header');
  const colors = await playerColors(main);
  const { ink, paper } = await hues(main);
  await main.waitForSelector('.tq-secret strong');
  const secret = await main.$eval('.tq-secret', (secret) => secret.textContent);
  const object = cues.find((cue) => is(cue, 'Secret')).beat.Secret.object;
  check(secret === `Secret: ${object}`, `play: the secret is in the header (${secret})`);
  await main.waitForSelector('.tq-header .fa-clock');
  check((await header(main)).buttons.join() === 'Stop', 'play: Stop shows while the game plays');
  // A thinker's dots, in its own bubble.
  await main.waitForSelector('.rc-thinking', { timeout: playing });
  const dots = (await bubbles(main)).filter((bubble) => bubble.thinking);
  check(dots.length > 0 && dots.every((bubble) => bubble.name === 'Host' ? bubble.background === ink : bubble.background === colors[bubble.name]?.fill), `play: thinking shows as dots in the thinker's bubble (${JSON.stringify(dots.map((d) => d.name))})`);
  // A guess waiting for its answer pulses.
  await main.waitForSelector('.tq-header .tq-waiting', { timeout: playing });
  const waiting = (await header(main)).pips.filter((pip) => pip.class.includes('tq-waiting'));
  check(waiting.length > 0 && waiting.every((pip) => pip.animated), `play: a guess waiting for its answer pulses (${waiting.length})`);
  await main.screenshot({ path: join(shots, 'playing.png') });
  await main.waitForSelector('.tq-outcome', { timeout: playing * 2 });
  await main.waitForTimeout(300);
  await main.screenshot({ path: join(shots, 'over.png') });

  const shown = await bubbles(main);
  const misplaced = shown.filter((bubble) => {
    if (bubble.name === 'Narrator') return bubble.side !== 'center';
    if (bubble.name === 'Host') return bubble.side !== 'right' || bubble.background !== ink || bubble.color !== paper;
    return bubble.side !== 'left' || bubble.background !== colors[bubble.name]?.fill || bubble.color !== ink;
  });
  check(misplaced.length === 0, `play: players speak from the left in their own colors, the host from the right in ink, the narrator centered (${JSON.stringify(misplaced.slice(0, 3))})`);
  const senders = shown.filter((bubble) => bubble.name !== 'Narrator' && bubble.name !== 'Host');
  const unsquared = senders.filter((bubble) => bubble.sender !== bubble.name || bubble.square !== colors[bubble.name].mark);
  check(senders.length > 0 && unsquared.length === 0, `play: a player's sender line is its name and a square of its color (${JSON.stringify(unsquared.slice(0, 2))})`);
  const narrator = shown.find((bubble) => bubble.name === 'Narrator');
  check(narrator && narrator.background === 'rgba(0, 0, 0, 0)', `play: the narrator is unboxed (${narrator?.background})`);

  const guesses = shown.filter((bubble) => bubble.tag?.startsWith('Guess'));
  const counted = cues.filter((cue) => is(cue, 'Guessed'));
  const badGuesses = guesses.filter((bubble, k) =>
    bubble.tag !== `Guess ${counted[k].beat.Guessed.guess}` || bubble.border !== colors[bubble.name].mark || bubble.borderWidth !== '2px' || bubble.tagCaps !== 'all-small-caps');
  check(guesses.length === counted.length && badGuesses.length === 0, `play: a guess is its player's bubble, bordered in its color and tagged in small capitals (${JSON.stringify(badGuesses.slice(0, 2))})`);
  const answers = shown.filter((bubble) => bubble.tag?.startsWith('Answer'));
  const answered = cues.filter((cue) => is(cue, 'Answered'));
  const asked = Object.fromEntries(counted.map((cue) => [cue.beat.Guessed.guess, cue.beat.Guessed.text]));
  const badAnswers = answers.filter((bubble, k) => {
    const guess = answered[k].beat.Answered.guess;
    return bubble.name !== 'Host' || bubble.tag !== `Answer ${guess}` || bubble.quote !== asked[guess];
  });
  check(answers.length === answered.length && badAnswers.length === 0, `play: an answer is the host's bubble, tagged with its guess and quoting it (${JSON.stringify(badAnswers.slice(0, 2))})`);
  const ringedTags = shown.filter((bubble) => bubble.ringed && !bubble.selected).map((bubble) => bubble.tag);
  check(
    named !== undefined && JSON.stringify(ringedTags) === JSON.stringify([`Guess ${named}`, `Answer ${named}`]),
    `play: the answer that names the object and its guess are ringed (${JSON.stringify(ringedTags)})`,
  );

  const over = await header(main);
  const winner = counted.find((cue) => cue.beat.Guessed.guess === named).beat.Guessed.who;
  const won = over.pips[named - 1];
  check(over.tally === `${counted.length} / 20` && over.pips.length === 20, `play: the header counts the guesses used (${over.tally}, ${over.pips.length} pips)`);
  check(won.class.includes('tq-won-pip') && won.background === colors[winner].mark, `play: the pip that won is in the guesser's color (${JSON.stringify(won)})`);
  const used = over.pips.slice(0, counted.length).filter((pip, k) => k !== named - 1);
  check(used.every((pip) => pip.background === ink && !pip.animated), 'play: every other guess used is an ink pip, still once the game is over');
  check(over.pips.slice(counted.length).every((pip) => pip.class === 'tq-pip'), 'play: the guesses left are open pips');
  check(/^\d+:\d\d$/.test(over.clock), `play: the countdown shows the time left, stopped (${over.clock})`);
  check(over.outcome === `The players won in ${named}`, `play: the header says who won (${over.outcome})`);
  check(over.buttons.join() === 'Save Log…,New game', `play: Save Log… and New game show once the game is over (${over.buttons})`);
  check((await main.$$('.rc-thinking')).length === 0, 'play: no dots once the game is over');

  await main.emulateMedia({ colorScheme: 'dark' });
  await still(main);
  const dark = await hues(main);
  const darkColors = await playerColors(main);
  const darkShown = await bubbles(main);
  const darkWrong = darkShown.filter((bubble) =>
    bubble.name === 'Host' ? bubble.background !== dark.ink || bubble.color !== dark.paper
      : bubble.name !== 'Narrator' && (bubble.background !== darkColors[bubble.name].fill || bubble.color !== dark.ink));
  check(darkWrong.length === 0 && dark.ink !== ink, `dark: players keep their colors, deep, and the host's ink turns light (${JSON.stringify(darkWrong.slice(0, 2))})`);
  await main.screenshot({ path: join(shots, 'over-dark.png') });

  await main.click('.tq-header button:text("New game")');
  await main.waitForSelector('.fa-start');
  check((await main.$$('.rc-message')).length === 0, 'play: New game goes back to the start');
  await app.close();
});

/** Where the review cursor is on `page`: the position shown, the
 * selected bubble and its tag, how many bubbles there are, how many are
 * dimmed and whether the first of them looks it, whether the selected
 * one is in view, the file named and the game of how many, and the
 * header. */
const cursor = (page) =>
  page.evaluate(() => {
    const messages = [...document.querySelectorAll('.rc-message')];
    const later = messages.filter((message) => message.classList.contains('rc-later'));
    const selected = document.querySelector('.rc-message.rc-selected');
    const pane = document.querySelector('.rc-messages').getBoundingClientRect();
    const box = selected?.getBoundingClientRect();
    return {
      position: document.querySelector('.fa-review-position')?.textContent ?? null,
      selected: messages.indexOf(selected),
      tag: selected?.querySelector('.tq-tag')?.textContent ?? null,
      count: messages.length,
      later: later.length,
      laterFirst: messages.indexOf(later[0]),
      dimmed: later.length > 0 && Number(getComputedStyle(later[0]).opacity) < 1,
      inView: box !== undefined && box.top >= pane.top - 1 && box.bottom <= pane.bottom + 1,
      name: document.querySelector('.fa-review-name')?.textContent ?? null,
      of: document.querySelector('.fa-review-game')?.textContent ?? null,
      secret: document.querySelector('.tq-secret')?.textContent ?? null,
      outcome: document.querySelector('.tq-outcome')?.textContent ?? null,
      tally: document.querySelector('.tq-tally')?.textContent ?? null,
      waiting: [...document.querySelectorAll('.tq-header .tq-waiting')].map((pip) => getComputedStyle(pip).animationName),
      clock: document.querySelector('.tq-header .fa-clock')?.textContent ?? null,
    };
  });

/** The menus the main window last enabled, as JSON. */
const menus = async (page) => JSON.stringify(await page.evaluate(() => window.standIn.menus));

// The game just played goes into review on its last bubble.
await section('review of a game just played', async () => {
  const { app, main } = await mainWindow();
  check((await menus(main)) === JSON.stringify({ games: 0, open: true, save: false }), 'review: Open Game Log… is enabled at the start, and Go and Save Game Log… are not');
  await main.click('.tq-play');
  await main.waitForSelector('.tq-secret strong');
  check((await menus(main)) === JSON.stringify({ games: 0, open: false, save: false }), 'review: Open Game Log… and Save Game Log… are disabled while the game plays');
  await main.waitForSelector('.fa-review-position', { timeout: playing * 2 });
  await main.waitForTimeout(300);
  const last = await cursor(main);
  check(
    last.count > 0 && last.selected === last.count - 1 && last.later === 0 && last.position === `${last.count} / ${last.count}` && last.inView,
    `review: a game over goes into review on its last bubble, where it left off, with nothing dimmed (${JSON.stringify(last)})`,
  );
  check(last.name === null && last.outcome === `The players won in ${named}`, `review: the header says who won and names no file (${last.name}, ${last.outcome})`);
  check((await menus(main)) === JSON.stringify({ games: 1, open: true, save: true }), 'review: Open Game Log…, Save Game Log… and Go are enabled in the review of a game just played');
  await main.click('.tq-header button:text("Save Log…")');
  await main.waitForTimeout(100);
  check((await main.evaluate(() => window.standIn.saved)) === 1, "review: Save Log… saves the game's log");

  await main.keyboard.press('Meta+ArrowUp');
  await main.waitForTimeout(300);
  const first = await cursor(main);
  check(
    first.selected === 0 && first.later === last.count - 1 && first.laterFirst === 1 && first.dimmed && first.inView,
    `review: ⌘↑ goes to the first bubble, every later one dimmed (${JSON.stringify(first)})`,
  );
  check(
    first.outcome === null && first.tally === '0 / 20' && first.waiting.length === 0,
    `review: the first bubble shows the game as it stood, before any guess and the end (${JSON.stringify(first)})`,
  );
  await main.screenshot({ path: join(shots, 'review-first.png') });
  await main.keyboard.press('Alt+ArrowDown');
  const guess = await cursor(main);
  check(
    guess.tag === 'Guess 1' && guess.tally === '1 / 20' && guess.waiting.length === 1 && guess.waiting[0] === 'none' && /^\d+:\d\d$/.test(guess.clock),
    `review: ⌥↓ goes to the first guess, waiting for its answer and holding still, with the time left then (${JSON.stringify(guess)})`,
  );
  await main.keyboard.press('Alt+ArrowDown');
  check((await cursor(main)).tag === 'Guess 2', 'review: ⌥↓ goes on to the next guess');
  await main.keyboard.press('Alt+ArrowUp');
  check((await cursor(main)).tag === 'Guess 1', 'review: ⌥↑ goes back a guess');
  await main.screenshot({ path: join(shots, 'review-guess.png') });
  await main.keyboard.press('ArrowDown');
  await main.keyboard.press('ArrowRight');
  check((await cursor(main)).selected === guess.selected + 2, 'review: the arrow keys step forward');
  await main.keyboard.press('ArrowUp');
  check((await cursor(main)).selected === guess.selected + 1, 'review: the arrow keys step back');
  await main.keyboard.press('End');
  check((await cursor(main)).selected === last.count - 1, 'review: End goes to the last bubble');
  await main.keyboard.press('ArrowDown');
  check((await cursor(main)).selected === last.count - 1, 'review: the cursor stops at the last bubble');
  await main.keyboard.press('Home');
  check((await cursor(main)).selected === 0, 'review: Home goes to the first bubble');
  await (await main.$$('.rc-message'))[4].click();
  check((await cursor(main)).selected === 4, 'review: a click on a bubble puts the cursor there');
  await main.keyboard.press('Meta+ArrowDown');
  await main.waitForTimeout(300);
  const end = await cursor(main);
  check(end.selected === last.count - 1 && end.outcome === last.outcome && end.clock === last.clock && end.inView, `review: ⌘↓ goes back to the last bubble and the end (${JSON.stringify(end)})`);

  // A log opened from here offers Close.
  await main.evaluate((cues) => {
    const games = [{ episode: '00000000-0000-0000-0000-000000000001', cues }];
    window.standIn.emit('review', { name: 'one.jsonl', recordings: { games, problems: [] } }, 'main');
  }, cues);
  await main.waitForSelector('.fa-review-name');
  check((await header(main)).buttons.join() === 'Close', 'review: a log opened from the review of a game just played offers Close');
  await main.evaluate(() => window.standIn.emit('new-game', null, 'main'));
  await main.waitForSelector('.fa-start');
  check((await menus(main)) === JSON.stringify({ games: 0, open: true, save: false }), 'review: New Game goes from a review to the start, and Save Game Log… is disabled');
  await app.close();
});

// A log of three games opened in review: the backend sends them as Open
// Game Log… would, the second cut off half way, its players' words marked.
await section('review of a log', async () => {
  const { app, main } = await mainWindow();
  await main.evaluate((cues) => {
    const game = (episode, cues) => ({ episode: `00000000-0000-0000-0000-00000000000${episode}`, cues });
    const marked = cues.slice(0, cues.length / 2).map((cue) => (typeof cue.beat === 'object' && 'Said' in cue.beat ? { ...cue, beat: { Said: { ...cue.beat.Said, text: `Second: ${cue.beat.Said.text}` } } } : cue));
    const games = [game(1, cues), game(2, marked), game(3, cues)];
    window.standIn.emit('review', { name: 'three.jsonl', recordings: { games, problems: ['the log is bad at line 9'] } }, 'main');
  }, cues);
  await main.waitForSelector('.fa-review-position');
  await main.waitForTimeout(300);
  const first = await cursor(main);
  check(
    first.name === 'three.jsonl' && first.of === 'Game 1 of 3' && first.position === `1 / ${first.count}` && first.selected === 0 && first.later === first.count - 1,
    `log: a log opens on the first bubble of its first game, every later one dimmed (${JSON.stringify(first)})`,
  );
  const warning = await main.$eval('.fa-review-warning', (warning) => warning.textContent);
  check(warning === "Some of this log couldn't be read: the log is bad at line 9", `log: what couldn't be read shows above the chat (${warning})`);
  const titles = await main.$$eval('.fa-review-games option', (options) => options.map((option) => option.textContent));
  const object = cues.find((cue) => is(cue, 'Secret')).beat.Secret.object;
  check(
    JSON.stringify(titles) === JSON.stringify([`1. ${object}, the players won in ${named}`, `2. ${object}, unfinished`, `3. ${object}, the players won in ${named}`]),
    `log: each game is listed by its secret and how it ended (${JSON.stringify(titles)})`,
  );
  check((await menus(main)) === JSON.stringify({ games: 3, open: true, save: false }), 'log: Open Game Log… and Go are enabled, Previous and Next Game with them, and Save Game Log… is not');
  check((await header(main)).buttons.join() === '‹,›,Close', `log: ‹ › and Close control the log (${(await header(main)).buttons})`);
  await main.screenshot({ path: join(shots, 'review-log.png') });
  const crowded = await main.evaluate(() => {
    const box = (selector) => document.querySelector(selector).getBoundingClientRect();
    const [title, count, controls] = [box('.tq-title'), box('.tq-count'), box('.tq-controls')];
    return title.right > count.left || count.bottom > controls.top;
  });
  check(!crowded, 'log: the review of several games has a row of its own under the title and the count');
  await main.keyboard.press('Meta+ArrowDown');
  await main.keyboard.press('Meta+]');
  await main.waitForTimeout(300);
  const second = await cursor(main);
  check(second.of === 'Game 2 of 3' && second.selected === 0 && second.count < first.count, `log: ⌘] opens the next game on its first bubble (${JSON.stringify(second)})`);
  /** What the players say in the chat. */
  const said = () => main.$$eval('.rc-message', (messages) => messages.filter((message) => !['Host', 'Narrator'].includes(message.dataset.name) && !message.querySelector('.tq-tag')).map((message) => message.querySelector('.rc-bubble').textContent));
  const secondSaid = await said();
  check(secondSaid.length > 0 && secondSaid.every((text) => text.startsWith('Second: ')), `log: the chat shows the game opened (${JSON.stringify(secondSaid.slice(0, 2))})`);
  await main.keyboard.press('End');
  check((await cursor(main)).outcome === null, 'log: a game cut off ends with no outcome');
  await main.selectOption('.fa-review-games select', '2');
  await main.waitForTimeout(100);
  check((await cursor(main)).of === 'Game 3 of 3', 'log: the list of games opens the one chosen');
  check((await said()).every((text) => !text.startsWith('Second: ')), 'log: the chat shows the game chosen');
  await main.keyboard.press('Meta+[');
  check((await cursor(main)).of === 'Game 2 of 3', 'log: ⌘[ opens the game before');
  await main.emulateMedia({ colorScheme: 'dark' });
  await still(main);
  await main.screenshot({ path: join(shots, 'review-log-dark.png') });
  await main.setViewportSize({ width: 420, height: 760 });
  await main.waitForTimeout(200);
  const wide = await main.evaluate(() => document.documentElement.scrollWidth);
  check(wide <= 420, `log: nothing scrolls sideways at 420 px (${wide})`);
  await main.screenshot({ path: join(shots, 'review-log-narrow-dark.png') });
  await main.click('.tq-header button:text("Close")');
  await main.waitForSelector('.fa-start');
  check((await menus(main)) === JSON.stringify({ games: 0, open: true, save: false }), 'log: Close goes back to the start, and Go is disabled again');
  await main.evaluate(() => window.standIn.emit('review', { name: 'empty.jsonl', recordings: { games: [], problems: [] } }, 'main'));
  await main.waitForSelector('.fa-start .fa-error');
  const empty = await main.$eval('.fa-start .fa-error', (error) => error.textContent);
  check(empty === 'empty.jsonl holds no games.', `log: a log with no game to show stays at the start and says why (${empty})`);
  await app.close();
});

// The web page, in a plain browser, with no backend.
await section('web', async () => {
  const web = await browser.newContext({ colorScheme: 'light', viewport: { width: 1000, height: 760 } });
  const page = watch(await web.newPage());
  await page.goto(`${origin}#prompted-fast`);
  await page.waitForSelector('.fa-review-position');
  await page.waitForTimeout(300);
  const rest = await cursor(page);
  check(
    rest.selected === rest.count - 1 && rest.later === 0 && rest.position === `${rest.count} / ${rest.count}` && rest.name === null && rest.outcome === `The players won in ${named}`,
    `web: the recorded game opens in review on its last bubble (${JSON.stringify(rest)})`,
  );
  check((await header(page)).buttons.join() === 'Play', 'web: Play is the only control');
  await page.screenshot({ path: join(shots, 'web.png') });
  await page.keyboard.press('ArrowUp');
  const back = await cursor(page);
  check(back.selected === rest.count - 2 && back.later === 1, `web: the arrow keys step back, dimming the bubble after (${JSON.stringify(back)})`);
  await page.keyboard.press('Home');
  check((await cursor(page)).selected === 0, 'web: Home goes to the first bubble');
  await (await page.$$('.rc-message'))[3].click();
  check((await cursor(page)).selected === 3, 'web: a click on a bubble puts the cursor there');
  await page.click('.tq-header button');
  await page.waitForTimeout(500);
  const played = await cursor(page);
  check(played.position === null && played.count < rest.count && (await header(page)).buttons.join() === 'Play again', `web: Play leaves review and plays the game again (${JSON.stringify(played)})`);
  await page.waitForSelector('.fa-review-position', { timeout: (cues.length * 250) / 10 + 10_000 });
  await page.waitForTimeout(300);
  const again = await cursor(page);
  check(again.selected === again.count - 1 && again.count === rest.count, `web: once every cue is shown the game is back in review on its last bubble (${JSON.stringify(again)})`);
  await page.emulateMedia({ colorScheme: 'dark' });
  await still(page);
  await page.screenshot({ path: join(shots, 'web-dark.png') });
  await page.setViewportSize({ width: 420, height: 760 });
  await page.waitForTimeout(200);
  const wide = await page.evaluate(() => document.documentElement.scrollWidth);
  check(wide <= 420, `web: nothing scrolls sideways at 420 px (${wide})`);
  await page.screenshot({ path: join(shots, 'web-narrow-dark.png') });
  await web.close();
});

await section('web without the game', async () => {
  const web = await browser.newContext({ colorScheme: 'light', viewport: { width: 1000, height: 760 } });
  const page = await web.newPage();
  await page.goto(`${origin}#missing`);
  await page.waitForSelector('.tq-main .fa-error');
  const error = await page.$eval('.tq-main .fa-error', (error) => error.textContent);
  check(error === 'game/missing.json: 404', `web: a game that can't be fetched says why (${error})`);
  await web.close();
});

await section('stop and new game', async () => {
  const { app, main } = await mainWindow();
  await main.click('.tq-play');
  await main.waitForSelector('.tq-secret strong');
  await main.click('.tq-header button');
  await main.evaluate(() => window.standIn.emit('new-game', null, 'main'));
  await main.waitForSelector('.fa-start');
  check((await main.$('.tq-header')) === null, 'File ▸ New Game goes back to the start from a game playing');
  await app.close();
});

await section('narrow', async () => {
  const { app, main } = await mainWindow({ viewport: { width: 420, height: 760 } });
  await main.screenshot({ path: join(shots, 'start-narrow.png') });
  await main.click('.tq-play');
  await main.waitForSelector('.tq-outcome', { timeout: playing * 2 });
  await main.waitForTimeout(300);
  const wide = await main.evaluate(() => document.documentElement.scrollWidth);
  check(wide <= 420, `narrow: nothing scrolls sideways at 420 px (${wide})`);
  const count = await main.$eval('.tq-count', (count) => count.getBoundingClientRect().right);
  check(count <= 420, `narrow: the count stays in the window (${count})`);
  await main.screenshot({ path: join(shots, 'over-narrow.png') });
  await main.emulateMedia({ colorScheme: 'dark' });
  await still(main);
  await main.screenshot({ path: join(shots, 'over-narrow-dark.png') });
  await app.close();
});

await section('favicon', async () => {
  const { app, main } = await mainWindow();
  const icon = await main.$eval('link[rel="icon"]', (link) => link.getAttribute('href'));
  check(icon === 'mark.svg', `favicon: the tab shows the mark (${icon})`);
  await app.close();
});

await section('settings', async () => {
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 760, height: 560 } });
  await app.addInitScript(backend(desktop, answers));
  const settings = watch(await app.newPage());
  await settings.goto(`${origin}#settings`);
  await settingsWindow(settings, 'settings', { tabs: ['General', 'Models', 'Game'], shots });
  await models(settings, 'settings', { plays: 'Twenty Questions', shots });
  await settings.click('#tab-game');
  await settings.waitForSelector('.fa-strategy');
  const page = await settings.evaluate(() => {
    const at = (path) => document.querySelector(`[id="field-game.${path}"]`)?.value;
    return {
      sections: [...document.querySelectorAll('.fa-section-head h2')].map((h2) => h2.textContent),
      players: at('players'),
      names: document.querySelector('.tq-names')?.textContent,
      guesses: at('guesses'),
      time: at('time_limit'),
      model: at('host.model'),
      object: document.querySelector('[id="field-game.host.object"]')?.placeholder,
      system: at('host.system')?.startsWith('{{ rules }}'),
      pick: at('host.pick'),
      temperature: at('host.temperature'),
      categories: at('host.categories'),
      strategy: document.querySelector('.fa-strategy .fa-item-name')?.textContent,
      everyone: at('strategy'),
      uses: [...document.querySelectorAll('.fa-part .fa-uses')].map((uses) => uses.textContent),
      head: document.querySelectorAll('.fa-page > .fa-field-error').length,
    };
  });
  check(
    JSON.stringify(page) ===
      JSON.stringify({
        sections: ['Players', 'Host', 'Strategies', 'Who plays what', 'Prompt parts'],
        players: '4',
        names: 'Alice, Bob, Carol and Dave',
        guesses: '20',
        time: '10m',
        model: 'qwen',
        object: 'The host picks',
        system: true,
        pick: 'Pick {{ category }}.',
        temperature: '1.2',
        categories: 'an animal',
        strategy: 'everyone',
        everyone: 'everyone',
        uses: ['used by 2 prompts'],
        head: 0,
      }),
    `settings: the Game page holds the whole game, the players named from their count and the prompt part's uses counted in prompts (${JSON.stringify(page)})`,
  );
  await settings.screenshot({ path: join(shots, 'settings-game.png'), fullPage: true });
  const [select] = await shapes(settings, '.fa-select');
  check(select.appearance === 'none' && select.icon && select.height === 36, `settings: a select is a field with a chevron (${JSON.stringify(select)})`);
  const [field] = await shapes(settings, '[id="field-game.host.object"]');
  check(field.height === 36 && field.radius === '8px' && field.shadow.includes('inset'), `settings: the secret's field is sunk into the page (${JSON.stringify(field)})`);
  const [prompt] = await shapes(settings, '.fa-strategy textarea');
  const [hostPrompt] = await shapes(settings, '[id="field-game.host.system"]');
  check(prompt.font.includes('monospace') && hostPrompt.font.includes('monospace'), `settings: a prompt is monospace (${prompt.font})`);
  const unringed = await ringed(settings, 20);
  check(unringed.length === 0, `settings: every control on the page shows a focus ring (${unringed.join(' ')})`);
  await typing(settings, 'settings', { selector: '[id="field-game.host.object"]', text: 'a teapot', what: 'the secret object' });
  await typing(settings, 'settings', { selector: '[id="field-game.time_limit"]', text: '5m', what: 'the time limit', empty: true });
  await typing(settings, 'settings', { selector: '[id="field-game.host.categories"]', text: 'a bird\na tree', what: 'the categories, one per line', empty: true });
  await settings.click('[aria-label="More players"]', { timeout: 5000 });
  await settings.click('[aria-label="Fewer guesses"]', { timeout: 5000 });
  await settings.waitForTimeout(100);
  const stepped = await settings.evaluate(() => ({
    players: document.querySelector('[id="field-game.players"]').value,
    names: document.querySelector('.tq-names').textContent,
    guesses: document.querySelector('[id="field-game.guesses"]').value,
  }));
  check(
    JSON.stringify(stepped) === JSON.stringify({ players: '5', names: 'Alice, Bob, Carol, Dave and Eve', guesses: '19' }),
    `settings: + adds a player, named next, and − takes a guess (${JSON.stringify(stepped)})`,
  );
  // A second strategy, then a seat split off to play it.
  await settings.fill('.fa-adder-strategy input', 'skeptic');
  await settings.click('.fa-adder-strategy button');
  await settings.waitForSelector('.fa-strategy >> nth=1');
  await settings.click('text=Split seats…');
  await settings.fill('[id="field-game.allotments.skeptic"]', '1');
  await settings.press('[id="field-game.allotments.skeptic"]', 'Tab');
  const split = await settings.evaluate(() => ({
    seats: [...document.querySelectorAll('.tq-seats')].map((seats) => `${seats.querySelector('input').value} ${seats.textContent}`),
    expanded: document.querySelector('.fa-section .fa-link[aria-expanded]').getAttribute('aria-expanded'),
  }));
  check(
    JSON.stringify(split) === JSON.stringify({ seats: ['0 everyone', '1 skeptic'], expanded: 'true' }),
    `settings: Split seats… gives seats to each strategy (${JSON.stringify(split)})`,
  );
  await groupedLists(settings, 'settings', { lists: 7 });
  await refusedPart(settings, 'settings');
  await helped(settings, 'settings', ['general', 'game']);
  // Scrolling hides help, so the field is in view before it is hovered.
  await settings.$eval('[id="field-game.host.object"]', (field) => field.scrollIntoView({ block: 'center' }));
  await settings.waitForTimeout(100);
  await tips(settings, 'settings', {
    hover: { selector: '[id="field-game.host.object"]', text: 'What the host is thinking of' },
    focus: { selector: '[id="field-game.host.object"]', text: 'What the host is thinking of' },
    press: '.fa-stepper button',
    shot: join(shots, 'settings-tip-game.png'),
  });
  // A window wide enough sets the strategies two across, shot tall enough
  // to hold the whole page.
  await settings.setViewportSize({ width: 1100, height: 2000 });
  await settings.waitForTimeout(100);
  const across = await settings.$$eval('.fa-strategy', (cards) => cards.map((card) => Math.round(card.getBoundingClientRect().top)));
  check(across.length === 2 && across[0] === across[1], `settings: the strategies sit two across (${across})`);
  await settings.screenshot({ path: join(shots, 'settings-game-wide.png'), fullPage: true });
  await settings.emulateMedia({ colorScheme: 'dark' });
  await still(settings);
  await settings.screenshot({ path: join(shots, 'settings-game-wide-dark.png'), fullPage: true });
  await settings.emulateMedia({ colorScheme: 'light' });
  await settings.setViewportSize({ width: 760, height: 560 });
  await narrowSettings(settings, 'settings', join(shots, 'settings-game-narrow.png'));
  await settings.setViewportSize({ width: 520, height: 700 });
  await settings.emulateMedia({ colorScheme: 'dark' });
  await still(settings);
  const below = await settings.$$eval('.fa-strategy', (cards) => cards.map((card) => Math.round(card.getBoundingClientRect().left)));
  check(below.length === 2 && below[0] === below[1], `settings: on a narrow window the strategies stack (${below})`);
  await settings.screenshot({ path: join(shots, 'settings-game-narrow-dark.png'), fullPage: true });
  await settings.setViewportSize({ width: 760, height: 560 });
  await settings.screenshot({ path: join(shots, 'settings-game-dark.png'), fullPage: true });
  await settings.emulateMedia({ colorScheme: 'light' });
  await app.close();
});

await section('settings errors', async () => {
  // Each error shows flagged in marigold beneath its own field, and none
  // at the page's head.
  const errors = [
    { path: ['game', 'time_limit'], message: "'soon' isn't a duration, such as 10m" },
    { path: ['game', 'host', 'temperature'], message: 'a temperature is from 0 to 2' },
    { path: ['game', 'strategies', 'everyone', 'system'], message: 'rendering the prompt of strategy everyone for Alice' },
    { path: ['game', 'prompt_parts', 'rules'], message: '[prompt_parts] has a part named "rules"' },
    { path: ['game', 'allotments'], message: '[players] strategies allots 5 seats, more than the 4 players' },
  ];
  const app = await browser.newContext({ colorScheme: 'light', viewport: { width: 760, height: 560 } });
  const split = { ...desktop.settings, game: { ...desktop.settings.game, allotments: { everyone: 5 } } };
  await app.addInitScript(backend({ settings: split, errors }, answers));
  const settings = watch(await app.newPage());
  await settings.goto(`${origin}#settings`);
  await settings.click('#tab-game');
  await settings.waitForSelector('.fa-strategy');
  const shown = await settings.evaluate((errors) => {
    const warn = getComputedStyle(document.documentElement).getPropertyValue('--fa-warn').trim();
    return {
      placed: errors.map(({ path, message }) => {
        const field = document.getElementById(`field-${path.join('.')}`);
        const error = [...(field?.closest('.fa-item')?.querySelectorAll('.fa-field-error') ?? [])].find((p) => p.textContent === message);
        return error ? getComputedStyle(error, '::before').backgroundColor : `${path.join('.')} not beneath its field`;
      }),
      warn,
      head: document.querySelectorAll('.fa-page > .fa-field-error').length,
    };
  }, errors);
  const marigold = await settings.evaluate((warn) => {
    const probe = document.createElement('p');
    probe.style.color = warn;
    document.body.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  }, shown.warn);
  check(shown.placed.every((color) => color === marigold) && shown.head === 0, `settings: each error shows flagged in marigold beneath its field (${JSON.stringify(shown)})`);
  await settings.screenshot({ path: join(shots, 'settings-game-errors.png'), fullPage: true });
  await app.close();
});

await browser.close();
server.close();
finish();
