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
// At the end the header says who won and offers New game, which goes back
// to the start, as File ▸ New Game does. Every player hue is clear of
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

/** The recorded prompted game's cues, which build-page.sh folds. */
const cues = JSON.parse(await readFile(resolve('prompted.json'), 'utf8'));

/** Whether `cue` is a beat of `kind`, such as `Guessed`. */
const is = (cue, kind) => cue.beat === kind || (typeof cue.beat === 'object' && kind in cue.beat);

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
 * times their pace, and `stop` ends the game stopped. */
function answers(standIn) {
  standIn.played = 0;
  return {
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
 * color, tag, quote and border, and whether it is ringed. */
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
  const named = answered.find((cue) => cue.beat.Answered.named)?.beat.Answered.guess;
  const ringedTags = shown.filter((bubble) => bubble.ringed).map((bubble) => bubble.tag);
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
  check(over.buttons.join() === 'New game', `play: New game shows once the game is over (${over.buttons})`);
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

  await main.click('.tq-header button');
  await main.waitForSelector('.fa-start');
  check((await main.$$('.rc-message')).length === 0, 'play: New game goes back to the start');
  await app.close();
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
